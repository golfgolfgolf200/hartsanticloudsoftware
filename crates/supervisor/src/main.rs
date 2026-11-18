use anyhow::{bail, Context, Result};
use axum::{
    body::Body,
    extract::{DefaultBodyLimit, Request, State},
    http::{header, HeaderValue, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use hartsanticloud_hw_scaler::{compute_budget, probe_hardware, RuntimeBudget};
use rand::RngCore;
use reqwest::Client;
use rusqlite::{params, types::ValueRef, Connection as SqliteConnection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    env,
    fs::{self, File, OpenOptions},
    io::{BufReader, Read, Write},
    net::TcpListener as StdTcpListener,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicU64, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use subtle::ConstantTimeEq;
use tokio::{
    io::AsyncWriteExt,
    net::UnixListener,
    process::{Child, Command},
};
use tracing::{error, info, warn};
mod doc_ingest;
mod mcp_bridge;
mod sandbox;
mod transcribe;
use sandbox::{SandboxExecRequest, SandboxManager};
use sqlite_vec::sqlite3_vec_init;
use zerocopy::IntoBytes;
use uuid::Uuid;

const MAX_REQUEST_BODY_BYTES: usize = 256 * 1024;
const MAX_CHAT_MESSAGES: usize = 64;
const MAX_TOKENS_CAP: u32 = 2048;
const MAX_SQL_LENGTH: usize = 4096;

const MODEL_STATE_READY: u8 = 0;
const MODEL_STATE_DOWNLOADING: u8 = 1;
const MODEL_STATE_NEEDS_BINARY: u8 = 2;
const MODEL_STATE_CHECKSUM_ERROR: u8 = 3;

struct AppState {
    data_dir: PathBuf,
    models_dir: PathBuf,
    bin_dir: PathBuf,
    sqlite_rw: Mutex<SqliteConnection>,
    sqlite_ro: Mutex<SqliteConnection>,
    initial_budget: RuntimeBudget,
    http_client: Client,
    inference_endpoint: Mutex<Option<String>>,
    embed_endpoint: Mutex<Option<String>>,
    embed_worker_handle: Mutex<Option<Child>>,
    internal_worker_api_key: String,
    cloud_burst_url: Option<String>,
    cloud_burst_key: Option<String>,
    auth_token: String,
    model_state: AtomicU8,
    model_downloaded_bytes: AtomicU64,
    model_total_bytes: AtomicU64,
    verified_model_sha256: Mutex<Option<String>>,
    worker_handle: Mutex<Option<Child>>,
    sandbox: Arc<SandboxManager>,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatCompletionRequest {
    #[serde(default)]
    model: Option<String>,
    messages: Vec<ChatMessage>,
    #[serde(default = "default_max_tokens")]
    max_tokens: u32,
    #[serde(default = "default_temperature")]
    temperature: f32,
    #[serde(default)]
    allow_cloud_burst: bool,
    #[serde(default)]
    use_vault_rag: bool,
}

fn default_max_tokens() -> u32 {
    512
}
fn default_temperature() -> f32 {
    0.2
}

#[derive(Debug, Serialize, Deserialize)]
struct AnalyticsQueryRequest {
    sql: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct AnalyticsQueryResponse {
    columns: Vec<String>,
    rows: Vec<Vec<serde_json::Value>>,
    elapsed_ms: u128,
}

#[derive(Debug, Serialize)]
struct SystemStatusResponse {
    status: String,
    timestamp_utc: String,
    budget: RuntimeBudget,
    live_telemetry: hartsanticloud_hw_scaler::HardwareTelemetry,
    inference_backend_healthy: bool,
    model_provisioning_state: String,
    model_download_pct: u8,
    verified_model_sha256: Option<String>,
    total_local_requests: i64,
    total_burst_requests: i64,
    estimated_cloud_savings_usd: f64,
    active_firecracker_vms: usize,
    kvm_assets_ready: bool,
}

fn random_hex_256() -> String {
    let mut raw = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut raw);
    raw.iter().map(|b| format!("{:02x}", b)).collect()
}

fn write_secret_file(path: &Path, secret: &str) -> Result<()> {
    if path.exists() {
        fs::remove_file(path)?;
    }
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("Failed to create secret file at {}", path.display()))?;
    file.write_all(secret.as_bytes())?;
    file.sync_all()?;
    Ok(())
}

fn generate_or_load_auth_token(run_dir: &Path) -> Result<String> {
    fs::create_dir_all(run_dir)?;
    let mut dir_perms = fs::metadata(run_dir)?.permissions();
    dir_perms.set_mode(0o700);
    fs::set_permissions(run_dir, dir_perms)?;

    let token_path = run_dir.join("supervisor.token");
    let token_hex = random_hex_256();
    write_secret_file(&token_path, &token_hex)?;
    Ok(token_hex)
}

fn allocate_ephemeral_loopback_port() -> Result<u16> {
    let listener = StdTcpListener::bind("127.0.0.1:0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

fn write_tmpfs_worker_key(key: &str) -> Result<PathBuf> {
    let base_dir = if Path::new("/dev/shm").is_dir() {
        PathBuf::from("/dev/shm")
    } else {
        env::temp_dir()
    };
    let path = base_dir.join(format!("hartsanticloud-worker-{}.key", std::process::id()));
    write_secret_file(&path, key)?;
    Ok(path)
}

async fn security_guard_middleware(
    State(state): State<Arc<AppState>>,
    req: Request<Body>,
    next: Next,
) -> Response {
    let provided_token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");

    let expected = state.auth_token.as_bytes();
    let provided = provided_token.as_bytes();
    let authorized = expected.len() == provided.len() && bool::from(expected.ct_eq(provided));

    if !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": "Missing or invalid supervisor bearer token" })),
        )
            .into_response();
    }

    let mut resp = next.run(req).await;
    let headers = resp.headers_mut();
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    resp
}

fn init_sqlite_connections(
    db_path: &Path,
    cache_size_kb: i64,
    mmap_size_bytes: i64,
) -> Result<(SqliteConnection, SqliteConnection)> {
    unsafe {
        rusqlite::ffi::sqlite3_auto_extension(Some(std::mem::transmute(
            sqlite3_vec_init as *const (),
        )));
    }
    let rw = SqliteConnection::open(db_path)
        .with_context(|| format!("Failed to open SQLite RW DB at {}", db_path.display()))?;

    rw.execute_batch(&format!(
        "PRAGMA journal_mode = WAL;
         PRAGMA synchronous = NORMAL;
         PRAGMA foreign_keys = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = {};
         PRAGMA mmap_size = {};
         PRAGMA busy_timeout = 5000;",
        cache_size_kb, mmap_size_bytes
    ))?;

    rw.execute_batch(
        "CREATE TABLE IF NOT EXISTS inference_audit (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            route_target TEXT NOT NULL,
            model_used TEXT NOT NULL,
            prompt_chars INTEGER NOT NULL,
            completion_chars INTEGER NOT NULL,
            latency_ms INTEGER NOT NULL,
            estimated_saved_usd REAL NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_inference_audit_created ON inference_audit(created_at);

        CREATE TABLE IF NOT EXISTS telemetry_rollups (
            recorded_at TEXT NOT NULL,
            tier TEXT NOT NULL,
            available_ram_mb INTEGER NOT NULL,
            available_disk_gb INTEGER NOT NULL,
            active_vms INTEGER NOT NULL
        );
        CREATE INDEX IF NOT EXISTS idx_telemetry_rollups_recorded ON telemetry_rollups(recorded_at);

        CREATE TABLE IF NOT EXISTS sandbox_audit (
            id TEXT PRIMARY KEY,
            created_at TEXT NOT NULL,
            runtime TEXT NOT NULL,
            workload_name TEXT NOT NULL,
            status TEXT NOT NULL,
            exit_code INTEGER NOT NULL,
            duration_ms INTEGER NOT NULL
        );

        CREATE TABLE IF NOT EXISTS vault_documents (
            id TEXT PRIMARY KEY,
            source_name TEXT NOT NULL,
            doc_type TEXT NOT NULL,
            chunk_count INTEGER NOT NULL,
            char_count INTEGER NOT NULL,
            sha256 TEXT NOT NULL,
            created_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS vault_chunks (
            chunk_rowid INTEGER PRIMARY KEY AUTOINCREMENT,
            doc_id TEXT NOT NULL,
            chunk_index INTEGER NOT NULL,
            source_name TEXT NOT NULL,
            content TEXT NOT NULL,
            FOREIGN KEY(doc_id) REFERENCES vault_documents(id) ON DELETE CASCADE
        );
        CREATE INDEX IF NOT EXISTS idx_vault_chunks_doc ON vault_chunks(doc_id);

        CREATE VIRTUAL TABLE IF NOT EXISTS vault_chunks_vec USING vec0(
            embedding float[384]
        );",
    )?;

    let ro = SqliteConnection::open_with_flags(
        db_path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("Failed to open SQLite RO DB at {}", db_path.display()))?;

    ro.execute_batch(&format!(
        "PRAGMA query_only = ON;
         PRAGMA temp_store = MEMORY;
         PRAGMA cache_size = {};
         PRAGMA mmap_size = {};
         PRAGMA busy_timeout = 3000;",
        cache_size_kb / 2,
        mmap_size_bytes
    ))?;

    Ok((rw, ro))
}

fn verify_artifact_permissions(path: &Path, base_dir: &Path) -> Result<PathBuf> {
    let canon_base = fs::canonicalize(base_dir)?;
    let canon_target = fs::canonicalize(path)?;
    if !canon_target.starts_with(&canon_base) {
        bail!(
            "Path traversal rejected: {} escapes {}",
            canon_target.display(),
            canon_base.display()
        );
    }
    let meta = fs::metadata(&canon_target)?;
    let mode = meta.permissions().mode();
    if mode & 0o002 != 0 {
        bail!(
            "Refusing to load world-writable artifact: {}",
            canon_target.display()
        );
    }
    Ok(canon_target)
}

fn compute_file_sha256(path: &Path) -> Result<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{:02x}", b)).collect())
}

async fn try_start_inference_worker(state: &Arc<AppState>) {
    let raw_model_path = state.models_dir.join(&state.initial_budget.ai_model_filename);
    let raw_binary = state.bin_dir.join("llama-server");

    if !raw_binary.exists() {
        warn!("llama-server binary not installed at {}", raw_binary.display());
        state.model_state.store(MODEL_STATE_NEEDS_BINARY, Ordering::Relaxed);
        return;
    }

    let binary = match verify_artifact_permissions(&raw_binary, &state.bin_dir) {
        Ok(p) => p,
        Err(e) => {
            error!("Binary security check failed: {}", e);
            state.model_state.store(MODEL_STATE_NEEDS_BINARY, Ordering::Relaxed);
            return;
        }
    };

    let model_path = match verify_artifact_permissions(&raw_model_path, &state.models_dir) {
        Ok(p) => p,
        Err(e) => {
            error!("Model security check failed: {}", e);
            return;
        }
    };

    let model_path_clone = model_path.clone();
    let actual_sha256 =
        match tokio::task::spawn_blocking(move || compute_file_sha256(&model_path_clone)).await {
            Ok(Ok(hash)) => hash,
            Ok(Err(e)) => {
                error!("Failed computing GGUF SHA-256 checksum: {}", e);
                state.model_state.store(MODEL_STATE_CHECKSUM_ERROR, Ordering::Relaxed);
                return;
            }
            Err(e) => {
                error!("SHA-256 worker panicked: {}", e);
                state.model_state.store(MODEL_STATE_CHECKSUM_ERROR, Ordering::Relaxed);
                return;
            }
        };

    if let Some(expected_sha256) = &state.initial_budget.ai_model_sha256 {
        let matched = bool::from(actual_sha256.as_bytes().ct_eq(expected_sha256.as_bytes()));
        if !matched {
            error!(
                "GGUF SHA-256 verification FAILED! Expected {}, got {}",
                expected_sha256, actual_sha256
            );
            state.model_state.store(MODEL_STATE_CHECKSUM_ERROR, Ordering::Relaxed);
            return;
        }
        info!("GGUF SHA-256 verified against pinned checksum: {}", actual_sha256);
    } else {
        info!(
            "GGUF SHA-256 recorded (set HARTSANTICLOUD_MODEL_SHA256 to enforce pin): {}",
            actual_sha256
        );
    }
    *state.verified_model_sha256.lock().unwrap() = Some(actual_sha256);

    let ephemeral_port = match allocate_ephemeral_loopback_port() {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to allocate ephemeral loopback port: {}", e);
            return;
        }
    };

    let tmpfs_key_path = match write_tmpfs_worker_key(&state.internal_worker_api_key) {
        Ok(p) => p,
        Err(e) => {
            error!("Failed to write tmpfs worker API key file: {}", e);
            return;
        }
    };

    let log_file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(state.data_dir.join("inference-worker.log"))
    {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open worker log: {}", e);
            return;
        }
    };

    match Command::new(&binary)
        .arg("-m")
        .arg(&model_path)
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(ephemeral_port.to_string())
        .arg("--api-key-file")
        .arg(&tmpfs_key_path)
        .arg("--no-webui")
        .arg("-t")
        .arg(state.initial_budget.ai_threads.to_string())
        .arg("-c")
        .arg(state.initial_budget.ai_context_tokens.to_string())
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => {
            let endpoint = format!("http://127.0.0.1:{}", ephemeral_port);
            info!(
                "Spawned {} on {} with tmpfs --api-key-file and --no-webui ({} threads, {} context)",
                binary.display(),
                endpoint,
                state.initial_budget.ai_threads,
                state.initial_budget.ai_context_tokens
            );
            *state.inference_endpoint.lock().unwrap() = Some(endpoint);
            *state.worker_handle.lock().unwrap() = Some(child);
            state.model_state.store(MODEL_STATE_READY, Ordering::Relaxed);
        }
        Err(e) => {
            error!("Failed to spawn llama-server: {}", e);
        }
    }
}

async fn ensure_model_provisioned(state: Arc<AppState>) {
    let target_path = state.models_dir.join(&state.initial_budget.ai_model_filename);
    if target_path.exists() {
        try_start_inference_worker(&state).await;
        return;
    }

    if let Err(e) = fs::create_dir_all(&state.models_dir) {
        error!(
            "Cannot create models directory {}: {}",
            state.models_dir.display(),
            e
        );
        return;
    }

    state.model_state.store(MODEL_STATE_DOWNLOADING, Ordering::Relaxed);
    info!(
        "First-run provisioning: downloading Liquid AI LFM2.5 model from {}",
        state.initial_budget.ai_model_url
    );

    let tmp_path = state
        .models_dir
        .join(format!("{}.part", state.initial_budget.ai_model_filename));
    let client = match Client::builder().timeout(Duration::from_secs(1800)).build() {
        Ok(c) => c,
        Err(e) => {
            error!("HTTP client error: {}", e);
            return;
        }
    };

    let mut resp = match client.get(&state.initial_budget.ai_model_url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            error!("Model download failed with HTTP {}", r.status());
            return;
        }
        Err(e) => {
            error!("Model download network error: {}", e);
            return;
        }
    };

    if let Some(len) = resp.content_length() {
        state.model_total_bytes.store(len, Ordering::Relaxed);
    }

    let mut out_file = match tokio::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o644)
        .open(&tmp_path)
        .await
    {
        Ok(f) => f,
        Err(e) => {
            error!("Failed to open temp model file {}: {}", tmp_path.display(), e);
            return;
        }
    };

    let mut downloaded: u64 = 0;
    let mut hasher = Sha256::new();
    while let Ok(Some(chunk)) = resp.chunk().await {
        if let Err(e) = out_file.write_all(&chunk).await {
            error!("Failed writing model chunk: {}", e);
            return;
        }
        hasher.update(&chunk);
        downloaded += chunk.len() as u64;
        state.model_downloaded_bytes.store(downloaded, Ordering::Relaxed);
    }

    if let Err(e) = out_file.flush().await {
        error!("Failed flushing model file: {}", e);
        return;
    }
    drop(out_file);

    let stream_sha256: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    if let Some(expected_sha256) = &state.initial_budget.ai_model_sha256 {
        let matched = bool::from(stream_sha256.as_bytes().ct_eq(expected_sha256.as_bytes()));
        if !matched {
            error!(
                "Downloaded GGUF SHA-256 mismatch! Expected {}, got {}. Removing untrusted file.",
                expected_sha256, stream_sha256
            );
            let _ = fs::remove_file(&tmp_path);
            state.model_state.store(MODEL_STATE_CHECKSUM_ERROR, Ordering::Relaxed);
            return;
        }
    }

    if let Err(e) = fs::rename(&tmp_path, &target_path) {
        error!("Failed finalizing model file: {}", e);
        return;
    }

    info!(
        "Liquid AI LFM2.5 model download complete ({} bytes, sha256={})",
        downloaded, stream_sha256
    );
    try_start_inference_worker(&state).await;
}

async fn health_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let live_telemetry = match probe_hardware(&state.data_dir) {
        Ok(t) => t,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let endpoint_opt = state.inference_endpoint.lock().unwrap().clone();
    let inference_backend_healthy = match endpoint_opt {
        Some(ep) => state
            .http_client
            .get(format!("{}/health", ep))
            .bearer_auth(&state.internal_worker_api_key)
            .timeout(Duration::from_millis(500))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false),
        None => false,
    };

    let m_state = state.model_state.load(Ordering::Relaxed);
    let downloaded = state.model_downloaded_bytes.load(Ordering::Relaxed);
    let total = state.model_total_bytes.load(Ordering::Relaxed);
    let pct = if m_state == MODEL_STATE_READY {
        100
    } else if total > 0 {
        ((downloaded * 100) / total).min(99) as u8
    } else {
        0
    };

    let state_label = match m_state {
        MODEL_STATE_READY => "ready",
        MODEL_STATE_DOWNLOADING => "downloading_model",
        MODEL_STATE_CHECKSUM_ERROR => "checksum_verification_failed",
        _ => "awaiting_llama_server_binary",
    };

    let verified_sha = state.verified_model_sha256.lock().unwrap().clone();

    let (total_local_requests, total_burst_requests, estimated_cloud_savings_usd) = {
        let conn = state.sqlite_ro.lock().unwrap();
        let local: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM inference_audit WHERE route_target = 'local'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let burst: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM inference_audit WHERE route_target = 'cloud_burst'",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0);
        let saved: f64 = conn
            .query_row(
                "SELECT COALESCE(SUM(estimated_saved_usd), 0.0) FROM inference_audit",
                [],
                |r| r.get(0),
            )
            .unwrap_or(0.0);
        (local, burst, saved)
    };

    Json(SystemStatusResponse {
        status: "online".to_string(),
        timestamp_utc: Utc::now().to_rfc3339(),
        budget: state.initial_budget.clone(),
        live_telemetry,
        inference_backend_healthy,
        model_provisioning_state: state_label.to_string(),
        model_download_pct: pct,
        verified_model_sha256: verified_sha,
        total_local_requests,
        total_burst_requests,
        estimated_cloud_savings_usd,
        active_firecracker_vms: state.sandbox.current_active(),
        kvm_assets_ready: state.data_dir.join("firecracker/vmlinux.bin").exists() && state.data_dir.join("firecracker/rootfs.ext4").exists(),
    })
    .into_response()
}

async fn chat_completions_handler(
    State(state): State<Arc<AppState>>,
    Json(mut payload): Json<ChatCompletionRequest>,
) -> impl IntoResponse {
    if payload.messages.is_empty() || payload.messages.len() > MAX_CHAT_MESSAGES {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "messages must contain between 1 and 64 entries" })),
        )
            .into_response();
    }

    for msg in &payload.messages {
        if !matches!(msg.role.as_str(), "system" | "user" | "assistant") {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Invalid message role" })),
            )
                .into_response();
        }
    }

    let clamped_max_tokens = payload.max_tokens.clamp(1, MAX_TOKENS_CAP);
    let clamped_temp = payload.temperature.clamp(0.0, 2.0);

    if payload.use_vault_rag {
        if let Some(last_user) = payload.messages.iter().rev().find(|m| m.role == "user") {
            if let Ok(hits) = search_vault_chunks(&state, &last_user.content, 3).await {
                if !hits.is_empty() {
                    let mut context_block = String::from(
                        "Answer using the following verified local vault excerpts when relevant. Cite the source filename.\n\n",
                    );
                    for (idx, hit) in hits.iter().enumerate() {
                        context_block.push_str(&format!(
                            "[Excerpt {} — {}]\n{}\n\n",
                            idx + 1,
                            hit.source_name,
                            hit.content
                        ));
                    }
                    payload.messages.insert(
                        0,
                        ChatMessage {
                            role: "system".to_string(),
                            content: context_block,
                        },
                    );
                }
            }
        }
    }

    let start = Instant::now();
    let prompt_chars: usize = payload.messages.iter().map(|m| m.content.len()).sum();
    let needs_burst = payload.allow_cloud_burst
        && prompt_chars > (state.initial_budget.ai_context_tokens as usize * 3);

    let (target_url, auth_header, route_target, model_used) =
        if needs_burst && state.cloud_burst_url.is_some() {
            (
                format!(
                    "{}/v1/chat/completions",
                    state.cloud_burst_url.as_ref().unwrap().trim_end_matches('/')
                ),
                state.cloud_burst_key.clone(),
                "cloud_burst",
                payload
                    .model
                    .clone()
                    .unwrap_or_else(|| "frontier-burst".to_string()),
            )
        } else {
            let ep = match state.inference_endpoint.lock().unwrap().clone() {
                Some(ep) => ep,
                None => {
                    return (
                        StatusCode::SERVICE_UNAVAILABLE,
                        Json(serde_json::json!({ "error": "Local Liquid AI inference worker is still provisioning" })),
                    )
                        .into_response();
                }
            };
            (
                format!("{}/v1/chat/completions", ep),
                Some(state.internal_worker_api_key.clone()),
                "local",
                state.initial_budget.ai_model_filename.clone(),
            )
        };

    let upstream_body = serde_json::json!({
        "model": model_used,
        "messages": payload.messages,
        "max_tokens": clamped_max_tokens,
        "temperature": clamped_temp,
        "stream": false
    });

    let mut req = state.http_client.post(&target_url).json(&upstream_body);
    if let Some(key) = auth_header {
        req = req.bearer_auth(key);
    }

    match req.send().await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status().as_u16())
                .unwrap_or(StatusCode::BAD_GATEWAY);
            let body_json: serde_json::Value = resp
                .json()
                .await
                .unwrap_or_else(|_| serde_json::json!({ "error": "Invalid upstream JSON" }));

            let msg_obj = &body_json["choices"][0]["message"];
            let completion_text = msg_obj["content"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| msg_obj["reasoning_content"].as_str().filter(|s| !s.trim().is_empty()))
                .unwrap_or("");
            let completion_chars = completion_text.len();

            let latency_ms = start.elapsed().as_millis() as i64;
            let saved_usd = if route_target == "local" {
                ((prompt_chars + completion_chars) as f64 / 4000.0) * 0.015
            } else {
                0.0
            };

            if let Ok(conn) = state.sqlite_rw.lock() {
                let _ = conn.execute(
                    "INSERT INTO inference_audit
                     (id, created_at, route_target, model_used, prompt_chars, completion_chars, latency_ms, estimated_saved_usd)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        Uuid::new_v4().to_string(),
                        Utc::now().to_rfc3339(),
                        route_target,
                        model_used,
                        prompt_chars as i64,
                        completion_chars as i64,
                        latency_ms,
                        saved_usd
                    ],
                );
            }

            (status, Json(body_json)).into_response()
        }
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({
                "error": "Local llama.cpp inference backend not ready yet",
                "details": e.to_string(),
                "model_path": state.models_dir.join(&state.initial_budget.ai_model_filename).display().to_string()
            })),
        )
            .into_response(),
    }
}

fn validate_readonly_sql(sql: &str) -> Result<String, &'static str> {
    if sql.len() > MAX_SQL_LENGTH {
        return Err("SQL query exceeds 4096 character limit");
    }
    let trimmed = sql.trim().trim_end_matches(';').trim();
    if trimmed.is_empty() {
        return Err("SQL query cannot be empty");
    }
    if trimmed.contains(';') || trimmed.contains("--") || trimmed.contains("/*") {
        return Err("Multi-statement queries and SQL comments are prohibited");
    }

    let upper = trimmed.to_uppercase();
    if !(upper.starts_with("SELECT ") || upper.starts_with("WITH ")) {
        return Err("Only single read-only SELECT or WITH queries are permitted");
    }

    const FORBIDDEN_TOKENS: &[&str] = &[
        "ATTACH",
        "DETACH",
        "PRAGMA",
        "VACUUM",
        "REINDEX",
        "ANALYZE",
        "CREATE",
        "DROP",
        "ALTER",
        "INSERT",
        "UPDATE",
        "DELETE",
        "REPLACE",
        "LOAD_EXTENSION",
    ];

    let tokens: Vec<&str> = upper
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|s| !s.is_empty())
        .collect();
    for forbidden in FORBIDDEN_TOKENS {
        if tokens.contains(forbidden) {
            return Err("Query contains restricted SQLite keyword");
        }
    }

    Ok(trimmed.to_string())
}

async fn sandbox_exec_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<SandboxExecRequest>,
) -> impl IntoResponse {
    match state.sandbox.execute(payload, &state.sqlite_rw).await {
        Ok(res) => (StatusCode::OK, Json(serde_json::json!(res))).into_response(),
        Err(e) => (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn analytics_query_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AnalyticsQueryRequest>,
) -> impl IntoResponse {
    let clean_sql = match validate_readonly_sql(&payload.sql) {
        Ok(s) => s,
        Err(msg) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": msg })),
            )
                .into_response();
        }
    };

    let start = Instant::now();
    let conn = state.sqlite_ro.lock().unwrap();
    let mut stmt = match conn.prepare(&clean_sql) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    let column_count = stmt.column_count();
    let columns: Vec<String> = (0..column_count)
        .map(|i| stmt.column_name(i).unwrap_or("col").to_string())
        .collect();

    let mut rows_out: Vec<Vec<serde_json::Value>> = Vec::new();
    let mut rows = match stmt.query([]) {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response();
        }
    };

    while let Ok(Some(row)) = rows.next() {
        if rows_out.len() >= 1000 {
            break;
        }
        let mut row_vals = Vec::with_capacity(column_count);
        for idx in 0..column_count {
            let val = match row.get_ref(idx) {
                Ok(ValueRef::Null) => serde_json::Value::Null,
                Ok(ValueRef::Integer(i)) => serde_json::json!(i),
                Ok(ValueRef::Real(f)) => serde_json::json!(f),
                Ok(ValueRef::Text(t)) => {
                    serde_json::json!(String::from_utf8_lossy(t).into_owned())
                }
                Ok(ValueRef::Blob(b)) => serde_json::json!(format!("<blob {} bytes>", b.len())),
                Err(_) => serde_json::Value::Null,
            };
            row_vals.push(val);
        }
        rows_out.push(row_vals);
    }

    Json(AnalyticsQueryResponse {
        columns,
        rows: rows_out,
        elapsed_ms: start.elapsed().as_millis(),
    })
    .into_response()
}

#[tokio::main]
async fn main() -> Result<()> {
    if std::env::args().nth(1).as_deref() == Some("mcp-bridge") {
        return mcp_bridge::run_mcp_stdio_bridge().await;
    }

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();

    let data_dir = PathBuf::from(
        env::var("HARTSANTICLOUD_DATA_DIR")
            .unwrap_or_else(|_| "/var/lib/hartsanticloud".to_string()),
    );
    let models_dir = PathBuf::from(
        env::var("HARTSANTICLOUD_MODELS_DIR")
            .unwrap_or_else(|_| "/opt/hartsanticloud/models".to_string()),
    );
    let bin_dir = PathBuf::from(
        env::var("HARTSANTICLOUD_BIN_DIR")
            .unwrap_or_else(|_| "/opt/hartsanticloud/bin".to_string()),
    );

    fs::create_dir_all(data_dir.join("sqlite"))?;
    let run_dir = data_dir.join("run");
    let auth_token = generate_or_load_auth_token(&run_dir)?;
    let internal_worker_api_key = random_hex_256();

    let enforce_min = env::var("HARTSANTICLOUD_ENFORCE_MIN_HW")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let budget = compute_budget(&data_dir, enforce_min)?;
    info!(
        "Hardware auto-scaler locked tier {:?}: {} cores, {} GiB RAM, engine={}, model={}, max_microvms={}",
        budget.tier,
        budget.telemetry.logical_cores,
        budget.telemetry.total_ram_bytes / (1024 * 1024 * 1024),
        budget.ai_engine_backend,
        budget.ai_model_filename,
        budget.max_firecracker_microvms
    );

    let sqlite_path = data_dir.join("sqlite/state.db");
    let (sqlite_rw, sqlite_ro) = init_sqlite_connections(
        &sqlite_path,
        budget.sqlite_cache_size_kb,
        budget.sqlite_mmap_size_bytes,
    )?;

    let sandbox = Arc::new(SandboxManager::new(
        bin_dir.clone(),
        data_dir.clone(),
        budget.telemetry.kvm_available,
        budget.max_firecracker_microvms,
        budget.firecracker_vcpu_per_vm,
        budget.firecracker_mem_mib_per_vm,
    ));

    let state = Arc::new(AppState {
        data_dir,
        models_dir,
        bin_dir,
        sqlite_rw: Mutex::new(sqlite_rw),
        sqlite_ro: Mutex::new(sqlite_ro),
        initial_budget: budget,
        http_client: Client::builder()
            .timeout(Duration::from_secs(120))
            .build()?,
        inference_endpoint: Mutex::new(None),
        embed_endpoint: Mutex::new(None),
        embed_worker_handle: Mutex::new(None),
        internal_worker_api_key,
        cloud_burst_url: env::var("HARTSANTICLOUD_BURST_URL").ok(),
        cloud_burst_key: env::var("HARTSANTICLOUD_BURST_KEY").ok(),
        auth_token,
        model_state: AtomicU8::new(MODEL_STATE_DOWNLOADING),
        model_downloaded_bytes: AtomicU64::new(0),
        model_total_bytes: AtomicU64::new(0),
        verified_model_sha256: Mutex::new(None),
        worker_handle: Mutex::new(None),
        sandbox,
    });

    let prov_state = state.clone();
    tokio::spawn(async move {
        ensure_model_provisioned(prov_state.clone()).await;
        ensure_embed_worker_provisioned(prov_state).await;
    });

    let bg_state = state.clone();
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(15));
        loop {
            ticker.tick().await;
            if let Ok(t) = probe_hardware(&bg_state.data_dir) {
                let tier_str = format!("{:?}", bg_state.initial_budget.tier);
                let avail_ram_mb = (t.available_ram_bytes / (1024 * 1024)) as i64;
                let avail_disk_gb = (t.available_disk_bytes / (1024 * 1024 * 1024)) as i64;
                let now_iso = Utc::now().to_rfc3339();
                if let Ok(conn) = bg_state.sqlite_rw.lock() {
                    let _ = conn.execute(
                        "INSERT INTO telemetry_rollups (recorded_at, tier, available_ram_mb, available_disk_gb, active_vms)
                         VALUES (?1, ?2, ?3, ?4, ?5)",
                        params![now_iso, tier_str, avail_ram_mb, avail_disk_gb, bg_state.sandbox.current_active() as i64],
                    );
                }
            }
        }
    });

    let app = Router::new()
        .route("/v1/system/status", get(health_handler))
        .route("/v1/chat/completions", post(chat_completions_handler))
        .route("/v1/analytics/query", post(analytics_query_handler))
        .route("/v1/sandbox/execute", post(sandbox_exec_handler))
        .route("/v1/sandbox/provision-kvm", post(handle_provision_kvm))
        .route("/v1/vault/ingest", post(vault_ingest_handler))
        .route("/v1/vault/search", post(vault_search_handler))
        .route("/v1/vault/documents", get(vault_list_documents_handler))
        .route("/v1/audio/transcriptions", post(audio_transcribe_handler))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BODY_BYTES))
        .layer(middleware::from_fn_with_state(
            state.clone(),
            security_guard_middleware,
        ))
        .with_state(state);

    let sock_path = run_dir.join("supervisor.sock");
    if sock_path.exists() {
        fs::remove_file(&sock_path)?;
    }

    let listener = UnixListener::bind(&sock_path)
        .with_context(|| format!("Failed to bind AF_UNIX socket at {}", sock_path.display()))?;
    let mut sock_perms = fs::metadata(&sock_path)?.permissions();
    sock_perms.set_mode(0o600);
    fs::set_permissions(&sock_path, sock_perms)?;

    info!(
        "Hardened AF_UNIX Supervisor listening on unix://{} (0600, zero TCP ports exposed)",
        sock_path.display()
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_provision_kvm(
    State(state): State<Arc<AppState>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let fc_dir = state.data_dir.join("firecracker");
    tokio::fs::create_dir_all(&fc_dir)
        .await
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let kernel_path = fc_dir.join("vmlinux.bin");
    let rootfs_path = fc_dir.join("rootfs.ext4");

    if kernel_path.exists() && rootfs_path.exists() {
        return Ok(Json(serde_json::json!({
            "status": "already_provisioned",
            "disk_used_mb": 425,
            "path": fc_dir.display().to_string()
        })));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(300))
        .build()
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let assets = [
        (
            "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.10/x86_64/vmlinux-5.10.223",
            &kernel_path,
        ),
        (
            "https://s3.amazonaws.com/spec.ccfc.min/firecracker-ci/v1.10/x86_64/ubuntu-22.04.ext4",
            &rootfs_path,
        ),
    ];

    for (url, dest) in assets {
        let resp = client
            .get(url)
            .send()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Download failed: {e}")))?
            .error_for_status()
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("HTTP error: {e}")))?;

        let bytes = resp
            .bytes()
            .await
            .map_err(|e| (StatusCode::BAD_GATEWAY, format!("Stream error: {e}")))?;

        let tmp = dest.with_extension("tmp");
        tokio::fs::write(&tmp, &bytes)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
        tokio::fs::rename(&tmp, dest)
            .await
            .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    }

    Ok(Json(serde_json::json!({
        "status": "provisioned",
        "disk_used_mb": 425,
        "path": fc_dir.display().to_string()
    })))
}

#[derive(Debug, Deserialize)]
struct VaultIngestRequest {
    #[serde(default)]
    file_path: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    markdown: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VaultSearchRequest {
    query: String,
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize {
    4
}

#[derive(Debug, Serialize)]
struct VaultSearchHit {
    doc_id: String,
    source_name: String,
    chunk_index: i64,
    distance: f64,
    content: String,
}

async fn ensure_embed_worker_provisioned(state: Arc<AppState>) {
    let target_path = state.models_dir.join(&state.initial_budget.embed_model_filename);
    if !target_path.exists() {
        info!(
            "Provisioning lightweight BGE-Small embedding GGUF from {}",
            state.initial_budget.embed_model_url
        );
        let tmp_path = state
            .models_dir
            .join(format!("{}.part", state.initial_budget.embed_model_filename));
        if let Ok(client) = Client::builder().timeout(Duration::from_secs(600)).build() {
            if let Ok(mut resp) = client.get(&state.initial_budget.embed_model_url).send().await {
                if resp.status().is_success() {
                    if let Ok(mut out) = tokio::fs::OpenOptions::new()
                        .create(true)
                        .write(true)
                        .truncate(true)
                        .mode(0o644)
                        .open(&tmp_path)
                        .await
                    {
                        let mut ok = true;
                        while let Ok(Some(chunk)) = resp.chunk().await {
                            if out.write_all(&chunk).await.is_err() {
                                ok = false;
                                break;
                            }
                        }
                        let _ = out.flush().await;
                        drop(out);
                        if ok {
                            let _ = fs::rename(&tmp_path, &target_path);
                        } else {
                            let _ = fs::remove_file(&tmp_path);
                        }
                    }
                }
            }
        }
    }

    let raw_binary = state.bin_dir.join("llama-server");
    if !raw_binary.exists() || !target_path.exists() {
        warn!("Embedding worker skipped: llama-server or embedding GGUF unavailable");
        return;
    }

    let binary = match verify_artifact_permissions(&raw_binary, &state.bin_dir) {
        Ok(p) => p,
        Err(e) => {
            error!("Embedding binary check failed: {}", e);
            return;
        }
    };
    let model_path = match verify_artifact_permissions(&target_path, &state.models_dir) {
        Ok(p) => p,
        Err(e) => {
            error!("Embedding model check failed: {}", e);
            return;
        }
    };

    let port = match allocate_ephemeral_loopback_port() {
        Ok(p) => p,
        Err(e) => {
            error!("Embedding port allocation failed: {}", e);
            return;
        }
    };

    let tmpfs_key_path = match write_tmpfs_worker_key(&state.internal_worker_api_key) {
        Ok(p) => p,
        Err(e) => {
            error!("Embedding key file failed: {}", e);
            return;
        }
    };

    let log_file = match OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(state.data_dir.join("embed-worker.log"))
    {
        Ok(f) => f,
        Err(e) => {
            error!("Embedding log open failed: {}", e);
            return;
        }
    };

    match Command::new(&binary)
        .arg("-m")
        .arg(&model_path)
        .arg("--embedding")
        .arg("--pooling")
        .arg("mean")
        .arg("--host")
        .arg("127.0.0.1")
        .arg("--port")
        .arg(port.to_string())
        .arg("--api-key-file")
        .arg(&tmpfs_key_path)
        .arg("--no-webui")
        .arg("-t")
        .arg("2")
        .arg("-c")
        .arg("512")
        .stdout(Stdio::null())
        .stderr(Stdio::from(log_file))
        .kill_on_drop(true)
        .spawn()
    {
        Ok(child) => {
            let endpoint = format!("http://127.0.0.1:{}", port);
            info!("Spawned BGE-Small embedding worker on {}", endpoint);
            *state.embed_endpoint.lock().unwrap() = Some(endpoint);
            *state.embed_worker_handle.lock().unwrap() = Some(child);
        }
        Err(e) => error!("Failed spawning embedding worker: {}", e),
    }
}

async fn compute_text_embedding(state: &Arc<AppState>, text: &str) -> Result<Vec<f32>> {
    let endpoint = state
        .embed_endpoint
        .lock()
        .unwrap()
        .clone()
        .context("Embedding worker not yet ready")?;

    let resp = state
        .http_client
        .post(format!("{}/v1/embeddings", endpoint))
        .bearer_auth(&state.internal_worker_api_key)
        .json(&serde_json::json!({ "input": text }))
        .send()
        .await
        .context("Failed calling embedding worker")?
        .error_for_status()
        .context("Embedding worker returned HTTP error")?;

    let body: serde_json::Value = resp.json().await?;
    let arr = body["data"][0]["embedding"]
        .as_array()
        .context("Invalid embedding JSON payload")?;

    let vec: Vec<f32> = arr
        .iter()
        .filter_map(|v| v.as_f64().map(|f| f as f32))
        .collect();

    if vec.len() != 384 {
        bail!("Expected 384-dim BGE-Small vector, received {}", vec.len());
    }
    Ok(vec)
}

async fn search_vault_chunks(
    state: &Arc<AppState>,
    query: &str,
    top_k: usize,
) -> Result<Vec<VaultSearchHit>> {
    let k = top_k.clamp(1, 20);
    let query_vec = compute_text_embedding(state, query).await?;
    let query_bytes = query_vec.as_bytes().to_vec();

    let conn = state.sqlite_ro.lock().unwrap();
    let mut stmt = conn.prepare(
        "SELECT c.doc_id, c.source_name, c.chunk_index, v.distance, c.content
         FROM vault_chunks_vec v
         JOIN vault_chunks c ON c.chunk_rowid = v.rowid
         WHERE v.embedding MATCH ?1 AND k = ?2
         ORDER BY v.distance ASC",
    )?;

    let rows = stmt.query_map(params![query_bytes, k as i64], |row| {
        Ok(VaultSearchHit {
            doc_id: row.get(0)?,
            source_name: row.get(1)?,
            chunk_index: row.get(2)?,
            distance: row.get(3)?,
            content: row.get(4)?,
        })
    })?;

    let mut hits = Vec::new();
    for r in rows {
        hits.push(r?);
    }
    Ok(hits)
}

async fn vault_ingest_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<VaultIngestRequest>,
) -> impl IntoResponse {
    let (source_name, doc_type, markdown) = if let Some(ref fp) = payload.file_path {
        let path = PathBuf::from(fp.trim());
        if !path.is_file() {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": format!("File not found: {}", path.display()) })),
            )
                .into_response();
        }
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("document")
            .to_string();
        match tokio::task::spawn_blocking(move || doc_ingest::extract_document_to_markdown(&path))
            .await
        {
            Ok(Ok((ext, md))) => (name, ext, md),
            Ok(Err(e)) => {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({ "error": e.to_string() })),
                )
                    .into_response()
            }
        }
    } else if let Some(md) = payload.markdown {
        let title = payload
            .title
            .unwrap_or_else(|| "inline-note.md".to_string());
        (title, "md".to_string(), md)
    } else {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Provide either 'file_path' or 'markdown'" })),
        )
            .into_response();
    };

    let chunks = doc_ingest::chunk_markdown(&markdown, 1100, 150);
    if chunks.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Document produced zero text chunks" })),
        )
            .into_response();
    }

    let mut embeddings: Vec<Vec<f32>> = Vec::with_capacity(chunks.len());
    for chunk in &chunks {
        match compute_text_embedding(&state, chunk).await {
            Ok(vec) => embeddings.push(vec),
            Err(e) => {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(serde_json::json!({ "error": format!("Embedding failed: {}", e) })),
                )
                    .into_response()
            }
        }
    }

    let mut hasher = Sha256::new();
    hasher.update(markdown.as_bytes());
    let doc_sha256: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{:02x}", b))
        .collect();

    let doc_id = Uuid::new_v4().to_string();
    let now_iso = Utc::now().to_rfc3339();
    let char_count = markdown.chars().count() as i64;
    let chunk_count = chunks.len() as i64;

    let write_res: Result<()> = (|| {
        let mut conn = state.sqlite_rw.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT INTO vault_documents (id, source_name, doc_type, chunk_count, char_count, sha256, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![doc_id, source_name, doc_type, chunk_count, char_count, doc_sha256, now_iso],
        )?;

        for (idx, (chunk, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
            tx.execute(
                "INSERT INTO vault_chunks (doc_id, chunk_index, source_name, content)
                 VALUES (?1, ?2, ?3, ?4)",
                params![doc_id, idx as i64, source_name, chunk],
            )?;
            let rowid = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO vault_chunks_vec (rowid, embedding) VALUES (?1, ?2)",
                params![rowid, emb.as_bytes()],
            )?;
        }
        tx.commit()?;
        Ok(())
    })();

    match write_res {
        Ok(()) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "status": "indexed",
                "doc_id": doc_id,
                "source_name": source_name,
                "doc_type": doc_type,
                "chunks_indexed": chunk_count,
                "chars_extracted": char_count,
                "sha256": doc_sha256
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("SQLite vector write failed: {}", e) })),
        )
            .into_response(),
    }
}

async fn vault_search_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<VaultSearchRequest>,
) -> impl IntoResponse {
    if payload.query.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query cannot be empty" })),
        )
            .into_response();
    }

    match search_vault_chunks(&state, &payload.query, payload.top_k).await {
        Ok(hits) => (
            StatusCode::OK,
            Json(serde_json::json!({
                "query": payload.query,
                "hits": hits
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": e.to_string() })),
        )
            .into_response(),
    }
}

async fn vault_list_documents_handler(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let conn = state.sqlite_ro.lock().unwrap();
    let mut stmt = match conn.prepare(
        "SELECT id, source_name, doc_type, chunk_count, char_count, sha256, created_at
         FROM vault_documents ORDER BY created_at DESC LIMIT 100",
    ) {
        Ok(s) => s,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    let rows = stmt.query_map([], |row| {
        Ok(serde_json::json!({
            "id": row.get::<_, String>(0)?,
            "source_name": row.get::<_, String>(1)?,
            "doc_type": row.get::<_, String>(2)?,
            "chunk_count": row.get::<_, i64>(3)?,
            "char_count": row.get::<_, i64>(4)?,
            "sha256": row.get::<_, String>(5)?,
            "created_at": row.get::<_, String>(6)?,
        }))
    });

    let mut docs = Vec::new();
    if let Ok(iter) = rows {
        for d in iter.flatten() {
            docs.push(d);
        }
    }

    (StatusCode::OK, Json(serde_json::json!({ "documents": docs }))).into_response()
}

#[derive(Debug, Deserialize)]
struct AudioTranscribeRequest {
    file_path: String,
    #[serde(default)]
    ingest_to_vault: bool,
}

async fn audio_transcribe_handler(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<AudioTranscribeRequest>,
) -> impl IntoResponse {
    let audio_path = PathBuf::from(payload.file_path.trim());
    let source_name = audio_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("audio-recording")
        .to_string();

    let res = transcribe::transcribe_audio_offline(
        &state.bin_dir,
        &state.models_dir,
        &state.initial_budget.whisper_model_filename,
        &state.initial_budget.whisper_model_url,
        &audio_path,
        state.initial_budget.ai_threads,
    )
    .await;

    let tr = match res {
        Ok(r) => r,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response()
        }
    };

    let mut ingested_doc_id: Option<String> = None;
    let mut chunks_indexed: i64 = 0;

    if payload.ingest_to_vault {
        let md = format!("# Transcript: {}\n\n{}", source_name, tr.transcript);
        let chunks = doc_ingest::chunk_markdown(&md, 1100, 150);
        if !chunks.is_empty() {
            let mut embeddings = Vec::with_capacity(chunks.len());
            let mut embed_ok = true;
            for c in &chunks {
                match compute_text_embedding(&state, c).await {
                    Ok(v) => embeddings.push(v),
                    Err(e) => {
                        warn!("Skipping transcript vault indexing (embedding error): {}", e);
                        embed_ok = false;
                        break;
                    }
                }
            }

            if embed_ok {
                let mut hasher = Sha256::new();
                hasher.update(md.as_bytes());
                let doc_sha256: String = hasher
                    .finalize()
                    .iter()
                    .map(|b| format!("{:02x}", b))
                    .collect();

                let doc_id = Uuid::new_v4().to_string();
                let now_iso = Utc::now().to_rfc3339();
                let char_count = md.chars().count() as i64;
                let count = chunks.len() as i64;

                let write_ok = (|| -> Result<()> {
                    let mut conn = state.sqlite_rw.lock().unwrap();
                    let tx = conn.transaction()?;
                    tx.execute(
                        "INSERT INTO vault_documents (id, source_name, doc_type, chunk_count, char_count, sha256, created_at)
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        params![doc_id, source_name, "transcript", count, char_count, doc_sha256, now_iso],
                    )?;
                    for (idx, (chunk, emb)) in chunks.iter().zip(embeddings.iter()).enumerate() {
                        tx.execute(
                            "INSERT INTO vault_chunks (doc_id, chunk_index, source_name, content)
                             VALUES (?1, ?2, ?3, ?4)",
                            params![doc_id, idx as i64, source_name, chunk],
                        )?;
                        let rowid = tx.last_insert_rowid();
                        tx.execute(
                            "INSERT INTO vault_chunks_vec (rowid, embedding) VALUES (?1, ?2)",
                            params![rowid, emb.as_bytes()],
                        )?;
                    }
                    tx.commit()?;
                    Ok(())
                })();

                if write_ok.is_ok() {
                    ingested_doc_id = Some(doc_id);
                    chunks_indexed = count;
                }
            }
        }
    }

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "status": "transcribed",
            "source_name": source_name,
            "model_used": tr.model_used,
            "elapsed_ms": tr.elapsed_ms,
            "text": tr.transcript,
            "ingested_doc_id": ingested_doc_id,
            "chunks_indexed": chunks_indexed
        })),
    )
        .into_response()
}
