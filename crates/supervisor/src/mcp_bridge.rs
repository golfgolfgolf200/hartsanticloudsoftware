use anyhow::{Context, Result};
use rmcp::{
    handler::server::wrapper::Parameters,
    model::*,
    tool, tool_router,
    ServiceExt,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::{
    env, fs,
    path::PathBuf,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::UnixStream,
};

#[derive(Clone)]
pub struct AntiCloudMcpBridge {
    socket_path: PathBuf,
    token_path: PathBuf,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct LocalChatParams {
    #[schemars(description = "User prompt or question for the local Liquid AI model")]
    pub prompt: String,
    #[schemars(description = "Optional system instruction")]
    #[serde(default)]
    pub system_prompt: Option<String>,
    #[schemars(description = "Set true to ground the response using local sqlite-vec vault documents")]
    #[serde(default)]
    pub use_vault_rag: bool,
    #[schemars(description = "Maximum tokens to generate (default 512)")]
    #[serde(default)]
    pub max_tokens: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VaultIngestParams {
    #[schemars(description = "Absolute local path to a .pdf, .docx, .xlsx, .xls, .ods, .csv, .md, or .txt file")]
    #[serde(default)]
    pub file_path: Option<String>,
    #[schemars(description = "Document title when passing raw markdown instead of file_path")]
    #[serde(default)]
    pub title: Option<String>,
    #[schemars(description = "Raw markdown content to index into the local sqlite-vec vault")]
    #[serde(default)]
    pub markdown: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct VaultSearchParams {
    #[schemars(description = "Semantic search query across indexed local vault documents")]
    pub query: String,
    #[schemars(description = "Number of top matching chunks to return (1-20, default 4)")]
    #[serde(default)]
    pub top_k: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TranscribeAudioParams {
    #[schemars(description = "Absolute path to a local .wav, .mp3, .ogg, .flac, or .m4a audio file")]
    pub file_path: String,
    #[schemars(description = "If true, automatically index the resulting transcript into the local sqlite-vec vault")]
    #[serde(default)]
    pub ingest_to_vault: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SqliteReadonlyParams {
    #[schemars(description = "Read-only SELECT or WITH SQL query against state.db (inference_audit, telemetry_rollups, sandbox_audit, vault_documents)")]
    pub sql: String,
}

impl AntiCloudMcpBridge {
    pub fn from_env() -> Self {
        let data_dir = PathBuf::from(
            env::var("HARTSANTICLOUD_DATA_DIR")
                .unwrap_or_else(|_| "/var/lib/hartsanticloud".to_string()),
        );
        let run_dir = data_dir.join("run");
        Self {
            socket_path: run_dir.join("supervisor.sock"),
            token_path: run_dir.join("supervisor.token"),
        }
    }

    async fn post_unix_json(
        &self,
        route: &str,
        body: serde_json::Value,
    ) -> std::result::Result<serde_json::Value, String> {
        let token = fs::read_to_string(&self.token_path)
            .map_err(|e| format!("Failed reading {}: {}", self.token_path.display(), e))?;
        let token = token.trim();

        let mut stream = UnixStream::connect(&self.socket_path)
            .await
            .map_err(|e| format!("Failed connecting to {}: {}", self.socket_path.display(), e))?;

        let payload = serde_json::to_vec(&body).map_err(|e| e.to_string())?;
        let req = format!(
            "POST {} HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            route,
            token,
            payload.len()
        );

        stream
            .write_all(req.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        stream
            .write_all(&payload)
            .await
            .map_err(|e| e.to_string())?;
        stream.flush().await.map_err(|e| e.to_string())?;

        let mut raw_resp = Vec::new();
        stream
            .read_to_end(&mut raw_resp)
            .await
            .map_err(|e| e.to_string())?;

        let text = String::from_utf8_lossy(&raw_resp);
        let (_headers, json_body) = text
            .split_once("\r\n\r\n")
            .ok_or_else(|| "Malformed HTTP response from supervisor socket".to_string())?;

        serde_json::from_str(json_body.trim())
            .map_err(|e| format!("Invalid JSON from supervisor ({}): {}", e, json_body.trim()))
    }
}

#[tool_router(server_handler)]
impl AntiCloudMcpBridge {
    #[tool(description = "Run a local, zero-egress Liquid AI chat completion (optionally grounded by local sqlite-vec vault RAG)")]
    async fn local_ai_chat(
        &self,
        Parameters(params): Parameters<LocalChatParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let mut messages = Vec::new();
        if let Some(sys) = params.system_prompt {
            if !sys.trim().is_empty() {
                messages.push(serde_json::json!({ "role": "system", "content": sys }));
            }
        }
        messages.push(serde_json::json!({ "role": "user", "content": params.prompt }));

        let req = serde_json::json!({
            "messages": messages,
            "use_vault_rag": params.use_vault_rag,
            "max_tokens": params.max_tokens.unwrap_or(512),
            "allow_cloud_burst": false
        });

        match self.post_unix_json("/v1/chat/completions", req).await {
            Ok(resp) => {
                let text = resp["choices"][0]["message"]["content"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| resp.to_string());
                Ok(CallToolResult::success(vec![ContentBlock::text(text)]))
            }
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }

    #[tool(description = "Extract a local document (.pdf, .docx, .xlsx, .xls, .ods, .csv, .md, .txt) or raw Markdown and index it into the local sqlite-vec vault")]
    async fn vault_ingest_document(
        &self,
        Parameters(params): Parameters<VaultIngestParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let req = serde_json::json!({
            "file_path": params.file_path,
            "title": params.title,
            "markdown": params.markdown,
        });
        match self.post_unix_json("/v1/vault/ingest", req).await {
            Ok(resp) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&resp).unwrap_or_else(|_| resp.to_string()),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }

    #[tool(description = "Search the local sqlite-vec knowledge vault using 384-dimensional dense vector similarity")]
    async fn vault_semantic_search(
        &self,
        Parameters(params): Parameters<VaultSearchParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let req = serde_json::json!({
            "query": params.query,
            "top_k": params.top_k.unwrap_or(4),
        });
        match self.post_unix_json("/v1/vault/search", req).await {
            Ok(resp) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&resp).unwrap_or_else(|_| resp.to_string()),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }

    #[tool(description = "Transcribe a local audio file (.wav, .mp3, .ogg, .flac, .m4a) completely offline using whisper.cpp and optionally index into the vault")]
    async fn transcribe_audio_file(
        &self,
        Parameters(params): Parameters<TranscribeAudioParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let req = serde_json::json!({
            "file_path": params.file_path,
            "ingest_to_vault": params.ingest_to_vault,
        });
        match self.post_unix_json("/v1/audio/transcriptions", req).await {
            Ok(resp) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&resp).unwrap_or_else(|_| resp.to_string()),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }

    #[tool(description = "Execute a read-only SELECT query against the local SQLite state.db telemetry and audit database")]
    async fn sqlite_readonly_query(
        &self,
        Parameters(params): Parameters<SqliteReadonlyParams>,
    ) -> std::result::Result<CallToolResult, ErrorData> {
        let req = serde_json::json!({ "sql": params.sql });
        match self.post_unix_json("/v1/analytics/query", req).await {
            Ok(resp) => Ok(CallToolResult::success(vec![ContentBlock::text(
                serde_json::to_string_pretty(&resp).unwrap_or_else(|_| resp.to_string()),
            )])),
            Err(e) => Ok(CallToolResult::error(vec![ContentBlock::text(e)])),
        }
    }
}

pub async fn run_mcp_stdio_bridge() -> Result<()> {
    let service = AntiCloudMcpBridge::from_env();
    let running = service
        .serve(rmcp::transport::io::stdio())
        .await
        .context("Failed starting rmcp stdio server")?;
    running.waiting().await?;
    Ok(())
}
