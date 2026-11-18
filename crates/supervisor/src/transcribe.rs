use anyhow::{bail, Context, Result};
use reqwest::Client;
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::{io::AsyncWriteExt, process::Command};
use tracing::info;
use uuid::Uuid;

pub struct TranscriptionResult {
    pub transcript: String,
    pub model_used: String,
    pub elapsed_ms: u128,
}

pub fn resolve_whisper_binary(bin_dir: &Path) -> Result<PathBuf> {
    let local_bin = bin_dir.join("whisper-cli");
    if local_bin.is_file() {
        let canon_base = fs::canonicalize(bin_dir)?;
        let canon_target = fs::canonicalize(&local_bin)?;
        if !canon_target.starts_with(&canon_base) {
            bail!("Path traversal rejected for whisper-cli");
        }
        let mode = fs::metadata(&canon_target)?.permissions().mode();
        if mode & 0o002 != 0 {
            bail!("Refusing to execute world-writable whisper-cli");
        }
        return Ok(canon_target);
    }

    if let Ok(path_env) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path_env) {
            let candidate = dir.join("whisper-cli");
            if candidate.is_file() {
                let mode = fs::metadata(&candidate)?.permissions().mode();
                if mode & 0o002 == 0 {
                    return Ok(candidate);
                }
            }
        }
    }

    bail!(
        "whisper-cli binary not installed in {} or PATH (build ggml-org/whisper.cpp and place whisper-cli in {})",
        bin_dir.display(),
        bin_dir.display()
    )
}

pub async fn ensure_whisper_model(
    models_dir: &Path,
    model_filename: &str,
    model_url: &str,
) -> Result<PathBuf> {
    fs::create_dir_all(models_dir)?;
    let target_path = models_dir.join(model_filename);
    if target_path.is_file() {
        return Ok(target_path);
    }

    info!(
        "On-demand provisioning Whisper GGML weights ({}) from {}",
        model_filename, model_url
    );

    let tmp_path = models_dir.join(format!("{}.part", model_filename));
    let client = Client::builder()
        .timeout(Duration::from_secs(900))
        .build()
        .context("Failed creating HTTP client for Whisper model download")?;

    let mut resp = client
        .get(model_url)
        .send()
        .await
        .context("Network error downloading Whisper GGML model")?
        .error_for_status()
        .context("HTTP error downloading Whisper GGML model")?;

    let mut out = tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o644)
        .open(&tmp_path)
        .await
        .with_context(|| format!("Failed opening {}", tmp_path.display()))?;

    while let Some(chunk) = resp.chunk().await? {
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    drop(out);

    fs::rename(&tmp_path, &target_path)?;
    Ok(target_path)
}

pub async fn transcribe_audio_offline(
    bin_dir: &Path,
    models_dir: &Path,
    model_filename: &str,
    model_url: &str,
    audio_path: &Path,
    threads: usize,
) -> Result<TranscriptionResult> {
    if !audio_path.is_file() {
        bail!("Audio file not found: {}", audio_path.display());
    }

    let ext = audio_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();

    if !matches!(ext.as_str(), "wav" | "mp3" | "ogg" | "flac" | "m4a") {
        bail!(
            "Unsupported audio format '.{}' (supported: wav, mp3, ogg, flac, m4a)",
            ext
        );
    }

    let whisper_bin = resolve_whisper_binary(bin_dir)?;
    let model_path = ensure_whisper_model(models_dir, model_filename, model_url).await?;

    let job_id = Uuid::new_v4().to_string();
    let out_prefix = std::env::temp_dir().join(format!("harts-whisper-{}", job_id));
    let expected_txt = PathBuf::from(format!("{}.txt", out_prefix.display()));

    let start = Instant::now();
    let output = Command::new(&whisper_bin)
        .arg("-m")
        .arg(&model_path)
        .arg("-f")
        .arg(audio_path)
        .arg("-t")
        .arg(threads.max(1).to_string())
        .arg("--no-prints")
        .arg("-otxt")
        .arg("-of")
        .arg(&out_prefix)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .await
        .with_context(|| format!("Failed spawning {}", whisper_bin.display()))?;

    let elapsed_ms = start.elapsed().as_millis();

    if !output.status.success() {
        let _ = fs::remove_file(&expected_txt);
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!(
            "whisper-cli failed (exit {:?}): {}",
            output.status.code(),
            stderr.trim()
        );
    }

    let transcript = if expected_txt.is_file() {
        let txt = fs::read_to_string(&expected_txt).unwrap_or_default();
        let _ = fs::remove_file(&expected_txt);
        txt
    } else {
        String::from_utf8_lossy(&output.stdout).to_string()
    };

    let cleaned = transcript
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");

    if cleaned.is_empty() {
        bail!("whisper-cli produced an empty transcript");
    }

    Ok(TranscriptionResult {
        transcript: cleaned,
        model_used: model_filename.to_string(),
        elapsed_ms,
    })
}
