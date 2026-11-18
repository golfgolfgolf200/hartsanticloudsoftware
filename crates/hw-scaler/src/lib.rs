use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;
use sysinfo::{Disks, System};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HardwareTier {
    Entry16G,
    Pro32G,
    Enterprise64GPlus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HardwareTelemetry {
    pub physical_cores: usize,
    pub logical_cores: usize,
    pub total_ram_bytes: u64,
    pub available_ram_bytes: u64,
    pub total_disk_bytes: u64,
    pub available_disk_bytes: u64,
    pub kvm_available: bool,
    pub cpu_brand: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeBudget {
    pub tier: HardwareTier,
    pub telemetry: HardwareTelemetry,
    pub ai_threads: usize,
    pub ai_context_tokens: u32,
    pub ai_model_filename: String,
    pub ai_model_url: String,
    pub ai_model_sha256: Option<String>,
    pub embed_model_filename: String,
    pub embed_model_url: String,
    pub whisper_model_filename: String,
    pub whisper_model_url: String,
    pub ai_engine_backend: String,
    pub max_firecracker_microvms: usize,
    pub firecracker_vcpu_per_vm: u8,
    pub firecracker_mem_mib_per_vm: u32,
    pub sqlite_cache_size_kb: i64,
    pub sqlite_mmap_size_bytes: i64,
    pub garage_cache_mb: u64,
}

pub fn probe_hardware(storage_mount: &Path) -> Result<HardwareTelemetry> {
    let mut sys = System::new_all();
    sys.refresh_all();

    let physical_cores = sys
        .physical_core_count()
        .unwrap_or_else(|| sys.cpus().len().max(1));
    let logical_cores = sys.cpus().len().max(1);
    let total_ram_bytes = sys.total_memory();
    let available_ram_bytes = sys.available_memory();
    let cpu_brand = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_string())
        .unwrap_or_else(|| "Unknown CPU".to_string());

    let disks = Disks::new_with_refreshed_list();
    let mut best_match_len = 0usize;
    let mut total_disk_bytes = 0u64;
    let mut available_disk_bytes = 0u64;

    for disk in disks.list() {
        let mount = disk.mount_point();
        if storage_mount.starts_with(mount) {
            let len = mount.as_os_str().len();
            if len >= best_match_len {
                best_match_len = len;
                total_disk_bytes = disk.total_space();
                available_disk_bytes = disk.available_space();
            }
        }
    }

    if total_disk_bytes == 0 {
        if let Some(root_disk) = disks.list().first() {
            total_disk_bytes = root_disk.total_space();
            available_disk_bytes = root_disk.available_space();
        }
    }

    let kvm_available = Path::new("/dev/kvm").exists();

    Ok(HardwareTelemetry {
        physical_cores,
        logical_cores,
        total_ram_bytes,
        available_ram_bytes,
        total_disk_bytes,
        available_disk_bytes,
        kvm_available,
        cpu_brand,
    })
}

pub fn compute_budget(storage_mount: &Path, enforce_minimums: bool) -> Result<RuntimeBudget> {
    let telemetry = probe_hardware(storage_mount)?;

    let min_ram_bytes: u64 = 14 * 1024 * 1024 * 1024;
    let min_disk_bytes: u64 = 200 * 1024 * 1024 * 1024;

    if enforce_minimums {
        if telemetry.logical_cores < 4 {
            bail!(
                "Insufficient CPU cores: detected {}, minimum required is 4",
                telemetry.logical_cores
            );
        }
        if telemetry.total_ram_bytes < min_ram_bytes {
            bail!(
                "Insufficient RAM: detected {} GiB, minimum required is 16 GiB",
                telemetry.total_ram_bytes / (1024 * 1024 * 1024)
            );
        }
        if telemetry.total_disk_bytes < min_disk_bytes {
            bail!(
                "Insufficient storage: detected {} GiB, minimum required is 256 GiB",
                telemetry.total_disk_bytes / (1024 * 1024 * 1024)
            );
        }
    }

    let total_gib = telemetry.total_ram_bytes / (1024 * 1024 * 1024);

    let tier = if total_gib >= 56 {
        HardwareTier::Enterprise64GPlus
    } else if total_gib >= 28 {
        HardwareTier::Pro32G
    } else {
        HardwareTier::Entry16G
    };

    let force_entry = std::env::var("HARTSANTICLOUD_FORCE_ENTRY_MODEL")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let (default_filename, default_url, default_sha256) = if force_entry || tier == HardwareTier::Entry16G {
        (
            "LFM2.5-1.2B-Instruct-Q4_K_M.gguf".to_string(),
            "https://huggingface.co/LiquidAI/LFM2.5-1.2B-Instruct-GGUF/resolve/main/LFM2.5-1.2B-Instruct-Q4_K_M.gguf".to_string(),
            Some("b1b3de114215d9507409a662a501a631095a479a419584e8a2ded6304b19b4f5".to_string()),
        )
    } else if tier == HardwareTier::Pro32G {
        (
            "LFM2.5-8B-A1B-Q4_K_M.gguf".to_string(),
            "https://huggingface.co/LiquidAI/LFM2.5-8B-A1B-GGUF/resolve/main/LFM2.5-8B-A1B-Q4_K_M.gguf".to_string(),
            None,
        )
    } else {
        (
            "LFM2.5-24B-A2B-Q4_K_M.gguf".to_string(),
            "https://huggingface.co/LiquidAI/LFM2.5-24B-A2B-GGUF/resolve/main/LFM2.5-24B-A2B-Q4_K_M.gguf".to_string(),
            None,
        )
    };

    let model_filename = std::env::var("HARTSANTICLOUD_MODEL_FILENAME")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_filename);

    let model_url = std::env::var("HARTSANTICLOUD_MODEL_URL")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or(default_url);

    let model_sha256 = std::env::var("HARTSANTICLOUD_MODEL_SHA256")
        .ok()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| s.len() == 64)
        .or(default_sha256);

    let (whisper_model_filename, whisper_model_url) = if force_entry || tier == HardwareTier::Entry16G {
        (
            "ggml-base.en-q5_1.bin".to_string(),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en-q5_1.bin".to_string(),
        )
    } else {
        (
            "ggml-small.en-q5_1.bin".to_string(),
            "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.en-q5_1.bin".to_string(),
        )
    };

    let embed_model_filename = "bge-small-en-v1.5-q8_0.gguf".to_string();
    let embed_model_url =
        "https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf/resolve/main/bge-small-en-v1.5-q8_0.gguf"
            .to_string();
    let engine_backend = "llama.cpp".to_string();

    let (
        ai_threads,
        ai_context_tokens,
        max_firecracker_microvms,
        firecracker_vcpu_per_vm,
        firecracker_mem_mib_per_vm,
        sqlite_cache_size_kb,
        sqlite_mmap_size_bytes,
        garage_cache_mb,
    ) = match tier {
        HardwareTier::Entry16G => (
            telemetry.physical_cores.saturating_sub(1).max(2),
            8192,
            4,
            1,
            256,
            -65536,
            134_217_728,
            512,
        ),
        HardwareTier::Pro32G => (
            telemetry.physical_cores.saturating_sub(2).max(4),
            16384,
            12,
            2,
            512,
            -262144,
            536_870_912,
            2048,
        ),
        HardwareTier::Enterprise64GPlus => (
            telemetry.physical_cores.saturating_sub(2).max(6),
            32768,
            32,
            2,
            768,
            -1048576,
            2_147_483_648,
            8192,
        ),
    };

    Ok(RuntimeBudget {
        tier,
        telemetry,
        ai_threads,
        ai_context_tokens,
        ai_model_filename: model_filename,
        ai_model_url: model_url,
        ai_model_sha256: model_sha256,
        embed_model_filename,
        embed_model_url,
        whisper_model_filename,
        whisper_model_url,
        ai_engine_backend: engine_backend,
        max_firecracker_microvms,
        firecracker_vcpu_per_vm,
        firecracker_mem_mib_per_vm,
        sqlite_cache_size_kb,
        sqlite_mmap_size_bytes,
        garage_cache_mb,
    })
}
