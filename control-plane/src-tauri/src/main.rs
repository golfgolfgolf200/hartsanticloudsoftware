#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use hartsanticloud_hw_scaler::{compute_budget, probe_hardware, HardwareTelemetry, RuntimeBudget};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Serialize, Deserialize)]
struct SupervisorStatusPayload {
    budget: RuntimeBudget,
    live_telemetry: HardwareTelemetry,
    inference_backend_healthy: bool,
    model_provisioning_state: String,
    model_download_pct: u8,
    total_local_requests: i64,
    total_burst_requests: i64,
    estimated_cloud_savings_usd: f64,
    #[serde(default)]
    active_firecracker_vms: usize,
    #[serde(default)]
    kvm_assets_ready: bool,
}

#[derive(Debug, Serialize)]
struct NodeDashboardSnapshot {
    supervisor_online: bool,
    supervisor_url: String,
    local_budget: RuntimeBudget,
    live_telemetry: HardwareTelemetry,
    inference_backend_healthy: bool,
    model_provisioning_state: String,
    model_download_pct: u8,
    total_local_requests: i64,
    total_burst_requests: i64,
    estimated_cloud_savings_usd: f64,
    active_firecracker_vms: usize,
    kvm_assets_ready: bool,
    netbird_interface_up: bool,
}

fn get_run_dir() -> PathBuf {
    let data_dir = std::env::var("HARTSANTICLOUD_DATA_DIR")
        .unwrap_or_else(|_| "/var/lib/hartsanticloud".to_string());
    PathBuf::from(data_dir).join("run")
}

fn load_supervisor_token() -> Result<String, String> {
    if let Ok(env_token) = std::env::var("HARTSANTICLOUD_SUPERVISOR_TOKEN") {
        let trimmed = env_token.trim().to_string();
        if !trimmed.is_empty() {
            return Ok(trimmed);
        }
    }

    let token_path = get_run_dir().join("supervisor.token");
    fs::read_to_string(&token_path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("Unable to read supervisor auth token at {}: {}", token_path.display(), e))
}

fn build_uds_client(timeout: Duration) -> Result<(Client, String), String> {
    let sock_path = get_run_dir().join("supervisor.sock");
    if !sock_path.exists() {
        return Err(format!("Supervisor Unix socket not found at {}", sock_path.display()));
    }
    let display_uri = format!("unix://{}", sock_path.display());
    let sock_for_connector = sock_path.clone();

    let client = Client::builder()
        .timeout(timeout)
        .unix_socket(sock_for_connector)
        .build()
        .map_err(|e| e.to_string())?;

    Ok((client, display_uri))
}

#[tauri::command]
async fn get_node_snapshot() -> Result<NodeDashboardSnapshot, String> {
    let netbird_interface_up = Path::new("/sys/class/net/wt0").exists();
    let mut supervisor_uri = format!("unix://{}", get_run_dir().join("supervisor.sock").display());

    if let (Ok(token), Ok((client, uri))) = (
        load_supervisor_token(),
        build_uds_client(Duration::from_millis(800)),
    ) {
        supervisor_uri = uri;
        if let Ok(resp) = client
            .get("http://localhost/v1/system/status")
            .bearer_auth(&token)
            .send()
            .await
        {
            if let Ok(remote) = resp.json::<SupervisorStatusPayload>().await {
                return Ok(NodeDashboardSnapshot {
                    supervisor_online: true,
                    supervisor_url: supervisor_uri,
                    local_budget: remote.budget,
                    live_telemetry: remote.live_telemetry,
                    inference_backend_healthy: remote.inference_backend_healthy,
                    model_provisioning_state: remote.model_provisioning_state,
                    model_download_pct: remote.model_download_pct,
                    total_local_requests: remote.total_local_requests,
                    total_burst_requests: remote.total_burst_requests,
                    estimated_cloud_savings_usd: remote.estimated_cloud_savings_usd,
                    active_firecracker_vms: remote.active_firecracker_vms,
                    kvm_assets_ready: remote.kvm_assets_ready,
                    netbird_interface_up,
                });
            }
        }
    }

    let root = Path::new("/");
    let local_budget = compute_budget(root, false).map_err(|e| e.to_string())?;
    let live_telemetry = probe_hardware(root).map_err(|e| e.to_string())?;

    Ok(NodeDashboardSnapshot {
        supervisor_online: false,
        supervisor_url: supervisor_uri,
        local_budget,
        live_telemetry,
        inference_backend_healthy: false,
        model_provisioning_state: "supervisor_offline".to_string(),
        model_download_pct: 0,
        total_local_requests: 0,
        total_burst_requests: 0,
        estimated_cloud_savings_usd: 0.0,
        active_firecracker_vms: 0,
        kvm_assets_ready: false,
        netbird_interface_up,
    })
}

#[tauri::command]
async fn send_local_prompt(prompt: String) -> Result<String, String> {
    if prompt.trim().is_empty() || prompt.len() > 32_000 {
        return Err("Prompt must be between 1 and 32,000 characters".to_string());
    }

    let token = load_supervisor_token()?;
    let (client, _) = build_uds_client(Duration::from_secs(120))?;

    let resp = client
        .post("http://localhost/v1/chat/completions")
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "messages": [{ "role": "user", "content": prompt }],
            "max_tokens": 512,
            "temperature": 0.2,
            "allow_cloud_burst": false
        }))
        .send()
        .await
        .map_err(|e| format!("Supervisor UDS unreachable: {}", e))?;

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    let msg = &body["choices"][0]["message"];
    if let Some(content) = msg["content"].as_str().filter(|s| !s.trim().is_empty()) {
        Ok(content.to_string())
    } else if let Some(reasoning) = msg["reasoning_content"].as_str().filter(|s| !s.trim().is_empty()) {
        Ok(reasoning.to_string())
    } else if let Some(err_msg) = body["error"]["message"].as_str().or_else(|| body["error"].as_str()) {
        Err(format!("Inference engine error: {}", err_msg))
    } else {
        Ok(serde_json::to_string_pretty(&body).unwrap_or_default())
    }
}

#[tauri::command]
async fn execute_sqlite_query(sql: String) -> Result<serde_json::Value, String> {
    if sql.trim().is_empty() || sql.len() > 4096 {
        return Err("SQL query must be between 1 and 4096 characters".to_string());
    }

    let token = load_supervisor_token()?;
    let (client, _) = build_uds_client(Duration::from_secs(15))?;

    let resp = client
        .post("http://localhost/v1/analytics/query")
        .bearer_auth(&token)
        .json(&serde_json::json!({ "sql": sql }))
        .send()
        .await
        .map_err(|e| format!("Supervisor UDS unreachable: {}", e))?;

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn run_sandbox_task(runtime: String, workload_name: String, payload: String) -> Result<serde_json::Value, String> {
    let token = load_supervisor_token()?;
    let (client, _) = build_uds_client(Duration::from_secs(65))?;

    let resp = client
        .post("http://localhost/v1/sandbox/execute")
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "runtime": runtime,
            "workload_name": workload_name,
            "payload": payload,
            "timeout_secs": 15
        }))
        .send()
        .await
        .map_err(|e| format!("Supervisor UDS unreachable: {}", e))?;

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}


#[tauri::command]
async fn provision_kvm_assets() -> Result<serde_json::Value, String> {
    let token = load_supervisor_token()?;
    let (client, _) = build_uds_client(Duration::from_secs(300))?;

    let resp = client
        .post("http://localhost/v1/sandbox/provision-kvm")
        .bearer_auth(&token)
        .json(&serde_json::json!({}))
        .send()
        .await
        .map_err(|e| format!("Supervisor UDS unreachable: {}", e))?;

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}


#[tauri::command]
async fn ingest_vault_item(
    file_path: Option<String>,
    title: Option<String>,
    markdown: Option<String>,
) -> Result<serde_json::Value, String> {
    let token = load_supervisor_token()?;
    let (client, _) = build_uds_client(Duration::from_secs(120))?;

    let resp = client
        .post("http://localhost/v1/vault/ingest")
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "file_path": file_path.filter(|s| !s.trim().is_empty()),
            "title": title.filter(|s| !s.trim().is_empty()),
            "markdown": markdown.filter(|s| !s.trim().is_empty()),
        }))
        .send()
        .await
        .map_err(|e| format!("Supervisor UDS unreachable: {}", e))?;

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn search_vault_items(query: String, top_k: Option<usize>) -> Result<serde_json::Value, String> {
    if query.trim().is_empty() {
        return Err("Search query cannot be empty".to_string());
    }

    let token = load_supervisor_token()?;
    let (client, _) = build_uds_client(Duration::from_secs(30))?;

    let resp = client
        .post("http://localhost/v1/vault/search")
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "query": query,
            "top_k": top_k.unwrap_or(4)
        }))
        .send()
        .await
        .map_err(|e| format!("Supervisor UDS unreachable: {}", e))?;

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
async fn transcribe_audio_item(
    file_path: String,
    ingest_to_vault: Option<bool>,
) -> Result<serde_json::Value, String> {
    if file_path.trim().is_empty() {
        return Err("Audio file path cannot be empty".to_string());
    }

    let token = load_supervisor_token()?;
    let (client, _) = build_uds_client(Duration::from_secs(600))?;

    let resp = client
        .post("http://localhost/v1/audio/transcriptions")
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "file_path": file_path,
            "ingest_to_vault": ingest_to_vault.unwrap_or(true)
        }))
        .send()
        .await
        .map_err(|e| format!("Supervisor UDS unreachable: {}", e))?;

    resp.json::<serde_json::Value>()
        .await
        .map_err(|e| e.to_string())
}

fn main() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            get_node_snapshot,
            send_local_prompt,
            execute_sqlite_query,
            run_sandbox_task,
            provision_kvm_assets,
            ingest_vault_item,
            search_vault_items,
            transcribe_audio_item
        ])
        .run(tauri::generate_context!())
        .expect("error while running Hart's Anti-Cloud Control Plane");
}
