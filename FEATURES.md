# Hart's Anti-Cloud v0.2.0

## Complete Feature and Deliverable Specification

Hart's Anti-Cloud is a local-first, CPU-native AI and edge-computing platform.
It combines a hardened Rust supervisor, local inference and sandbox runtimes,
an MCP stdio bridge, and a native Tauri v2 and Svelte 5 Control Plane.

## Capability Summary

| Subsystem | Technology / Engine | Network Exposure | Key Capabilities |
| :--- | :--- | :--- | :--- |
| Hardware auto-scaler | `crates/hw-scaler` (Rust) | Local probe only | Automatic `Entry16G`, `Pro32G`, and `Enterprise64GPlus` sizing |
| Local LLM and vision | llama.cpp `llama-server` | Ephemeral `127.0.0.1` listener | Liquid AI LFM2.5 CPU inference and optional `--mmproj` vision projectors |
| Knowledge vault and OCR | sqlite-vec, `pdftoppm`, Tesseract | No cloud egress | 384-dimensional vector RAG and native PDF/Office parsing with offline OCR fallback |
| Voice transcription | whisper.cpp `whisper-cli` | No cloud egress | Offline `.wav`, `.mp3`, `.ogg`, `.flac`, and `.m4a` transcription |
| Agent sandboxes | Firecracker KVM or Wasmtime | Isolated | Firecracker microVMs with optional jailer isolation and WASI 1.0 fallback |
| Supervisor API and IPC | Rust and Axum | Owner-only `AF_UNIX` socket | 256-bit bearer authentication and no public supervisor TCP listener |
| IDE and agent bridge | `hartsanticloud-supervisor mcp-bridge` | `stdin` / `stdout` only | MCP tools for Claude Code, Cursor, Zed, Continue, and compatible clients |
| Desktop Control Plane | Tauri v2 and Svelte 5 | `AF_UNIX` only | Telemetry, inference, SQLite queries, vaults, transcription, and sandbox controls |

## 1. Hardware Auto-Scaler and Telemetry

The [`hw-scaler` crate](crates/hw-scaler) probes physical and logical CPU cores,
total and available RAM, storage, and `/dev/kvm` availability at startup.
It configures threads, context size, microVM limits, and SQLite memory budgets.

| Hardware Tier | Detected RAM | AI CPU Threads | Context Window | Max Concurrent MicroVMs | SQLite WAL Cache |
| :--- | :--- | :--- | :--- | :--- | :--- |
| `Entry16G` | `< 28 GiB` | Physical cores minus 1, minimum 2 | 8,192 tokens | 4 | 64 MB |
| `Pro32G` | `28 – 55 GiB` | Physical cores minus 2, minimum 4 | 16,384 tokens | 12 | 256 MB |
| `Enterprise64GPlus` | `>= 56 GiB` | Physical cores minus 2, minimum 6 | 32,768 tokens | 32 | 1 GB |

The supervisor records CPU and memory status, disk usage, active microVM
count, local versus cloud-burst request counts, and estimated cloud savings in
SQLite. Data is stored in `telemetry_rollups` and `inference_audit`.

## 2. Local CPU and Multimodal Vision AI

- Runs Liquid AI LFM2.5 models with llama.cpp `llama-server` on CPU.
- Uses `LFM2.5-1.2B-Instruct-Q4_K_M.gguf` on `Entry16G`; higher tiers select
  larger tier models unless `HARTSANTICLOUD_FORCE_ENTRY_MODEL=1` is set.
- Verifies the entry model against the configured SHA-256 digest before use.
- Detects an explicit `HARTSANTICLOUD_MMPROJ_FILENAME` or a `*mmproj*.gguf`
  file in the models directory and passes `--mmproj` to llama-server.
- Accepts OpenAI-compatible `image_url` content in chat-completion requests.
- Sends inference locally by default. Cloud routing requires both
  `HARTSANTICLOUD_BURST_URL` and `allow_cloud_burst: true`.

The internal worker listens only on `127.0.0.1` on an ephemeral port and
authenticates with a temporary key file under `/dev/shm`.

## 3. Local Knowledge Vault and Offline OCR

The [`doc_ingest` module](crates/supervisor/src/doc_ingest.rs) supports:

- Native text extraction from PDF, DOCX, XLSX, XLS, ODS, CSV, Markdown, text,
  JSON, YAML, TOML, and log files.
- Offline image OCR for PNG, JPEG, WebP, TIFF, and BMP files.
- Offline PDF OCR through `pdftoppm` and Tesseract when a PDF has no usable
  native text layer.
- 384-dimensional embeddings using
  `bge-small-en-v1.5-q8_0.gguf` and nearest-neighbor search in sqlite-vec.

The vault exposes document ingestion, semantic search, and document-listing
operations through the supervisor API and MCP bridge.

## 4. Offline Voice and Meeting Transcription

The supervisor transcribes `.wav`, `.mp3`, `.ogg`, `.flac`, and `.m4a` files
with whisper.cpp `whisper-cli`.

- `Entry16G`: `ggml-base.en-q5_1.bin`
- `Pro32G` and `Enterprise64GPlus`: `ggml-small.en-q5_1.bin`
- Transcripts can optionally be chunked and indexed into the knowledge vault.
- `HARTSANTICLOUD_FORCE_ENTRY_MODEL=1` selects the entry transcription model
  even on a larger-RAM node.

## 5. Disposable Agent Sandboxes

The [`sandbox` module](crates/supervisor/src/sandbox.rs) supports only the
`firecracker` and `wasmtime` runtimes.

### Firecracker KVM

- Requires `/dev/kvm`, Firecracker, and provisioned `vmlinux.bin` and
  `rootfs.ext4` assets.
- Applies tier-scaled vCPU, memory, timeout, and concurrency limits.
- Uses the Firecracker jailer when a jailer binary is available and the
  supervisor runs with sufficient privileges.
- Jailer launches use UID/GID `65534`, an isolated chroot, a new PID
  namespace, cgroup v2 memory limits, and seccomp level 2.
- Falls back to the direct Firecracker VMM when jailer isolation is unavailable.

### Wasmtime WASI

- Runs `wasm32-wasip1` modules with the Bytecode Alliance Wasmtime runtime.
- Applies payload and memory limits and is available when KVM isolation is
  unavailable or a lightweight sandbox is requested.

## 6. Security and Isolation

- The supervisor exposes no public or supervisor TCP listener. Its primary
  service interface is an owner-only `AF_UNIX` socket at
  `/var/lib/hartsanticloud/run/supervisor.sock`.
- The supervisor creates an owner-only bearer token and validates requests
  with constant-time comparison.
- The internal worker API key and SQLite state use owner-controlled files.
- Interactive analytics use a dedicated read-only SQLite connection with
  `PRAGMA query_only = ON` and accept only a single `SELECT` or `WITH`
  statement.
- Audit tables record latency, character and token counts, and estimated savings;
  they do not store raw prompts or model output.
- The MCP bridge communicates only over stdio and delegates to the local
  supervisor over the authenticated Unix socket.

The desktop snapshot also reports the status of the NetBird `wt0` interface
when present. It does not enable or configure NetBird itself.

## 7. Native Desktop Control Plane

The [`control-plane`](control-plane) provides:

- Live CPU, RAM, NVMe, and microVM telemetry.
- Local CPU and optional vision inference testing.
- Optional grounding of chat requests with local vault RAG.
- Read-only SQLite inspection for `telemetry_rollups`, `inference_audit`,
  `sandbox_audit`, and `vault_documents`.
- Firecracker and Wasmtime sandbox execution.
- One-click KVM asset provisioning from the UI.
- PDF, image, and Markdown file ingestion.
- Semantic vault search.
- Offline audio transcription with optional vault indexing.
- Capability badges for the vision projector, OCR tools, jailer, NetBird, and
  Firecracker assets.

## 8. MCP Tools

| MCP Tool | Capability |
| :--- | :--- |
| `local_ai_chat` | Runs local LFM2.5 inference and optionally injects vault context. |
| `vault_ingest_document` | Extracts and embeds PDF, DOCX, spreadsheet, CSV, Markdown, text, image, and supported text files. |
| `vault_semantic_search` | Searches the local 384-dimensional sqlite-vec vault. |
| `transcribe_audio_file` | Transcribes supported audio completely offline and optionally indexes the result. |
| `sqlite_readonly_query` | Runs read-only `SELECT` or `WITH` queries against the audit and vault tables. |

Example Claude Code configuration:

```json
{
  "mcpServers": {
    "harts-anticloud": {
      "command": "/opt/hartsanticloud/bin/hartsanticloud-supervisor",
      "args": ["mcp-bridge"]
    }
  }
}
```

Or register it with:

```bash
claude mcp add harts-anticloud -- /opt/hartsanticloud/bin/hartsanticloud-supervisor mcp-bridge
```

## 9. Release Bundle

The v0.2.0 release build script creates
`hartsanticloud-v0.2.0-linux-x86_64.tar.zst`, with an XZ fallback if Zstandard
is unavailable. The bundle includes:

- `bin/hartsanticloud-supervisor` - stripped LTO Rust supervisor and MCP bridge.
- `bin/llama-server` - llama.cpp inference server, when available in
  `/opt/hartsanticloud/bin`.
- `bin/whisper-cli` - whisper.cpp transcription binary, when available.
- `bin/wasmtime` - Wasmtime WASI runtime, when available.
- `bin/firecracker` - Firecracker VMM, when available.
- `Hart's AntiCloud Control Plane_0.2.0_amd64.deb` - Debian/Ubuntu/Mint package,
  when built.
- `Hart's AntiCloud Control Plane_0.2.0_amd64.AppImage` - portable Linux
  desktop package, when built.
- `install.sh` - installer and systemd user-service setup.
- `SHA256SUMS` - checksum manifest for packaged binaries and desktop installers.
- `docs/` - bundled project documentation.
- `assets/wasm/` - optional bundled WASM tasks.

The current `build-release-bundle.sh` does not include a `jailer` binary in the
runtime bundle. Jailer support is used only if a compatible binary is already
available on the target under a supported search path.

## 10. Installation and Source Build

Packaged releases require no Rust or Node.js toolchain:

```bash
cd dist-release
./install.sh
```

To build from source:

```bash
cargo build --release -p hartsanticloud-supervisor

HARTSANTICLOUD_DATA_DIR=/tmp/hartsanticloud-dev \
  ./target/release/hartsanticloud-supervisor

cd control-plane
npm ci
HARTSANTICLOUD_DATA_DIR=/tmp/hartsanticloud-dev npm run tauri dev

cd ..
./scripts/build-release-bundle.sh
```
