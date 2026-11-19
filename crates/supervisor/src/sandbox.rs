use anyhow::{bail, Context, Result};
use chrono::Utc;
use rusqlite::{params, Connection as SqliteConnection};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
    process::Command,
};
use tracing::{info, warn};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxExecRequest {
    pub runtime: String,
    pub workload_name: String,
    #[serde(default)]
    pub payload: String,
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
}

fn default_timeout_secs() -> u64 {
    15
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SandboxExecResponse {
    pub job_id: String,
    pub runtime_used: String,
    pub status: String,
    pub exit_code: i32,
    pub duration_ms: u128,
    pub vcpu_allocated: u8,
    pub mem_mib_allocated: u32,
    pub active_vms: usize,
    pub max_vms: usize,
    pub output: String,
}

pub struct SandboxManager {
    pub bin_dir: PathBuf,
    pub data_dir: PathBuf,
    pub kvm_available: bool,
    pub max_vms: usize,
    pub vcpu_per_vm: u8,
    pub mem_mib_per_vm: u32,
    pub active_vms: AtomicUsize,
}

impl SandboxManager {
    pub fn new(
        bin_dir: PathBuf,
        data_dir: PathBuf,
        kvm_available: bool,
        max_vms: usize,
        vcpu_per_vm: u8,
        mem_mib_per_vm: u32,
    ) -> Self {
        Self {
            bin_dir,
            data_dir,
            kvm_available,
            max_vms,
            vcpu_per_vm,
            mem_mib_per_vm,
            active_vms: AtomicUsize::new(0),
        }
    }

    pub fn current_active(&self) -> usize {
        self.active_vms.load(Ordering::Relaxed)
    }

    async fn send_firecracker_api(sock_path: &Path, method: &str, route: &str, body_json: &str) -> Result<String> {
        let mut stream = UnixStream::connect(sock_path)
            .await
            .with_context(|| format!("Failed connecting to Firecracker API socket {}", sock_path.display()))?;

        let req = format!(
            "{} {} HTTP/1.1\r\nHost: localhost\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            method,
            route,
            body_json.len(),
            body_json
        );
        stream.write_all(req.as_bytes()).await?;
        stream.flush().await?;

        let mut resp = Vec::new();
        stream.read_to_end(&mut resp).await?;
        Ok(String::from_utf8_lossy(&resp).into_owned())
    }

    pub async fn execute(
        self: &Arc<Self>,
        req: SandboxExecRequest,
        sqlite_rw: &Mutex<SqliteConnection>,
    ) -> Result<SandboxExecResponse> {
        if req.workload_name.trim().is_empty()
            || req.workload_name.len() > 128
            || req.workload_name.contains("..")
            || req.workload_name.contains('/')
        {
            bail!("Invalid workload_name: must be a simple identifier without path separators");
        }
        if req.payload.len() > 65_536 {
            bail!("Sandbox payload exceeds 64 KiB limit");
        }

        let clamped_timeout = req.timeout_secs.clamp(1, 60);
        let runtime = req.runtime.trim().to_ascii_lowercase();
        if runtime != "firecracker" && runtime != "wasmtime" {
            bail!("Unsupported sandbox runtime '{}': expected 'firecracker' or 'wasmtime'", runtime);
        }

        let prev = self.active_vms.fetch_add(1, Ordering::SeqCst);
        if prev >= self.max_vms {
            self.active_vms.fetch_sub(1, Ordering::SeqCst);
            bail!(
                "Sandbox concurrency cap reached ({}/{} active instances)",
                prev,
                self.max_vms
            );
        }

        let job_id = Uuid::new_v4().to_string();
        let start = Instant::now();
        let result = if runtime == "firecracker" {
            self.run_firecracker_microvm(&job_id, &req.workload_name, clamped_timeout)
                .await
        } else {
            self.run_wasmtime_module(&job_id, &req.workload_name, &req.payload, clamped_timeout)
                .await
        };

        let remaining = self.active_vms.fetch_sub(1, Ordering::SeqCst).saturating_sub(1);
        let duration_ms = start.elapsed().as_millis();

        let (status_str, exit_code, output) = match result {
            Ok((code, out)) => ("completed".to_string(), code, out),
            Err(e) => ("unavailable_or_failed".to_string(), -1, e.to_string()),
        };

        if let Ok(conn) = sqlite_rw.lock() {
            let _ = conn.execute(
                "INSERT INTO sandbox_audit (id, created_at, runtime, workload_name, status, exit_code, duration_ms)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    job_id,
                    Utc::now().to_rfc3339(),
                    runtime,
                    req.workload_name,
                    status_str,
                    exit_code,
                    duration_ms as i64
                ],
            );
        }

        Ok(SandboxExecResponse {
            job_id,
            runtime_used: runtime,
            status: status_str,
            exit_code,
            duration_ms,
            vcpu_allocated: self.vcpu_per_vm,
            mem_mib_allocated: self.mem_mib_per_vm,
            active_vms: remaining,
            max_vms: self.max_vms,
            output,
        })
    }

    async fn run_firecracker_microvm(
        &self,
        job_id: &str,
        workload_name: &str,
        timeout_secs: u64,
    ) -> Result<(i32, String)> {
        if !self.kvm_available {
            bail!("Hardware virtualization /dev/kvm is not enabled on this host");
        }

        let fc_bin = self.bin_dir.join("firecracker");
        let jailer_bin = self.bin_dir.join("jailer");
        let fc_assets = self.data_dir.join("firecracker");
        let kernel_path = fc_assets.join("vmlinux.bin");
        let rootfs_path = fc_assets.join("rootfs.ext4");

        if !fc_bin.exists() || !kernel_path.exists() || !rootfs_path.exists() {
            bail!(
                "Firecracker assets not provisioned yet (requires {}, {}, and {})",
                fc_bin.display(),
                kernel_path.display(),
                rootfs_path.display()
            );
        }

        let job_dir = fc_assets.join("jobs").join(job_id);
        fs::create_dir_all(&job_dir)?;
        let mut perms = fs::metadata(&job_dir)?.permissions();
        perms.set_mode(0o700);
        fs::set_permissions(&job_dir, perms)?;

        let is_root = fs::metadata("/proc/self")
            .map(|m| std::os::unix::fs::MetadataExt::uid(&m) == 0)
            .unwrap_or(false);
        let use_jailer = jailer_bin.exists() && is_root;

        let (child, api_sock, guest_kernel, guest_rootfs, cleanup_root) = if use_jailer {
            let chroot_base = fc_assets.join("jail");
            let vm_root = chroot_base.join("firecracker").join(job_id).join("root");
            let run_dir = vm_root.join("run");
            fs::create_dir_all(&run_dir)?;

            let jailed_kernel = vm_root.join("vmlinux.bin");
            let jailed_rootfs = vm_root.join("rootfs.ext4");
            if fs::hard_link(&kernel_path, &jailed_kernel).is_err() {
                fs::copy(&kernel_path, &jailed_kernel)?;
            }
            fs::copy(&rootfs_path, &jailed_rootfs)?;

            let mem_bytes = (self.mem_mib_per_vm as u64 + 64) * 1024 * 1024;
            let mut cmd = Command::new(&jailer_bin);
            cmd.arg("--id")
                .arg(job_id)
                .arg("--exec-file")
                .arg(&fc_bin)
                .arg("--uid")
                .arg("65534")
                .arg("--gid")
                .arg("65534")
                .arg("--chroot-base-dir")
                .arg(&chroot_base)
                .arg("--new-pid-ns")
                .arg("--cgroup-version")
                .arg("2")
                .arg("--cgroup")
                .arg(format!("memory.max={}", mem_bytes))
                .arg("--")
                .arg("--api-sock")
                .arg("/run/firecracker.sock")
                .arg("--seccomp-level")
                .arg("2")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true);

            let child = cmd
                .spawn()
                .context("Failed to spawn Firecracker Jailer process")?;
            let host_sock = run_dir.join("firecracker.sock");
            let jail_job_dir = chroot_base.join("firecracker").join(job_id);
            (
                child,
                host_sock,
                PathBuf::from("/vmlinux.bin"),
                PathBuf::from("/rootfs.ext4"),
                jail_job_dir,
            )
        } else {
            let api_sock = job_dir.join("firecracker.sock");
            let ephem_rootfs = job_dir.join("rootfs.ext4");
            fs::copy(&rootfs_path, &ephem_rootfs)?;

            let child = Command::new(&fc_bin)
                .arg("--api-sock")
                .arg(&api_sock)
                .arg("--id")
                .arg(job_id)
                .arg("--seccomp-level")
                .arg("2")
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .kill_on_drop(true)
                .spawn()
                .context("Failed to spawn Firecracker VMM process")?;
            (child, api_sock, kernel_path, ephem_rootfs, job_dir.clone())
        };

        for _ in 0..50 {
            if api_sock.exists() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }

        let machine_cfg = serde_json::json!({
            "vcpu_count": self.vcpu_per_vm,
            "mem_size_mib": self.mem_mib_per_vm,
            "smt": false
        })
        .to_string();
        Self::send_firecracker_api(&api_sock, "PUT", "/machine-config", &machine_cfg).await?;

        let boot_src = serde_json::json!({
            "kernel_image_path": guest_kernel,
            "boot_args": format!("console=ttyS0 reboot=k panic=1 pci=off net.ifnames=0 harts_job={} harts_workload={}", job_id, workload_name)
        })
        .to_string();
        Self::send_firecracker_api(&api_sock, "PUT", "/boot-source", &boot_src).await?;

        let drive_cfg = serde_json::json!({
            "drive_id": "rootfs",
            "path_on_host": guest_rootfs,
            "is_root_device": true,
            "is_read_only": false
        })
        .to_string();
        Self::send_firecracker_api(&api_sock, "PUT", "/drives/rootfs", &drive_cfg).await?;

        let start_action = serde_json::json!({ "action_type": "InstanceStart" }).to_string();
        Self::send_firecracker_api(&api_sock, "PUT", "/actions", &start_action).await?;

        info!(
            "Booted Firecracker microVM {} (jailer={}, {} vCPU, {} MiB RAM, seccomp=2)",
            job_id, use_jailer, self.vcpu_per_vm, self.mem_mib_per_vm
        );

        let wait_res = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;
        let _ = fs::remove_dir_all(&cleanup_root);
        let _ = fs::remove_dir_all(&job_dir);

        match wait_res {
            Ok(Ok(out)) => Ok((
                out.status.code().unwrap_or(0),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )),
            Ok(Err(e)) => bail!("Firecracker VMM error: {}", e),
            Err(_) => {
                warn!("Firecracker microVM {} exceeded {}s timeout and was terminated", job_id, timeout_secs);
                bail!("MicroVM execution timed out after {}s", timeout_secs)
            }
        }
    }

    async fn run_wasmtime_module(
        &self,
        job_id: &str,
        workload_name: &str,
        payload: &str,
        timeout_secs: u64,
    ) -> Result<(i32, String)> {
        let wt_bin = self.bin_dir.join("wasmtime");
        let wasm_dir = self.data_dir.join("wasm");
        let wasm_file = wasm_dir.join(format!("{}.wasm", workload_name));

        if !wt_bin.exists() || !wasm_file.exists() {
            bail!(
                "Wasmtime runtime or module not provisioned yet (requires {} and {})",
                wt_bin.display(),
                wasm_file.display()
            );
        }

        let mut child = Command::new(&wt_bin)
            .arg("run")
            .arg("--max-memory-size")
            .arg(format!("{}", (self.mem_mib_per_vm as u64) * 1024 * 1024))
            .arg(&wasm_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .context("Failed to spawn Wasmtime WASI sandbox")?;

        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(payload.as_bytes()).await;
        }

        info!("Executed Wasmtime WASI sandbox job {} ({})", job_id, workload_name);
        let wait_res = tokio::time::timeout(Duration::from_secs(timeout_secs), child.wait_with_output()).await;

        match wait_res {
            Ok(Ok(out)) => Ok((
                out.status.code().unwrap_or(0),
                String::from_utf8_lossy(&out.stdout).into_owned(),
            )),
            Ok(Err(e)) => bail!("Wasmtime error: {}", e),
            Err(_) => bail!("Wasmtime execution timed out after {}s", timeout_secs),
        }
    }
}
