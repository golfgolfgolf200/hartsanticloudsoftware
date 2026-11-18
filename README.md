# Hart's Anti-Cloud Software

**Local-first, zero-egress edge computing and CPU-native AI platform.**

Hart's Anti-Cloud turns standard x86_64 Linux workstations, mini-PCs, and
on-premise servers into self-contained, cryptographically hardened compute
nodes. It eliminates recurring cloud API bills, third-party data exposure,
and GPU hardware lock-in.

---

## Why Hart's Anti-Cloud Is Significant

Modern software and AI workflows typically route sensitive business records,
internal prompts, and application state through external cloud APIs.

1. **Data sovereignty and compliance:** Hart's Anti-Cloud keeps inference,
   analytics, and sandboxed agent execution inside the local machine. It
   sends no telemetry, requires no cloud account, and supports optional
   air-gapped operation.
2. **Elimination of per-token cloud taxes:** Liquid AI's instruction-tuned
   **LFM2.5 (`1.2B-Instruct-Q4_K_M`)** runs on office/personal CPUs with a
   statically compiled, headless **`ggml-org/llama.cpp`** engine. It performs
   JSON extraction, RAG, and summarization without discrete GPU VRAM.
3. **Defense-in-depth local IPC:** The node exposes no public or supervisor
   TCP ports. It uses owner-only (`0600`) `AF_UNIX` domain sockets,
   constant-time 256-bit bearer authentication, tmpfs worker secrets, and
   SHA-256 weight verification.

---

## System Footprint & Resource Efficiency

Hart's Anti-Cloud is engineered to run comfortably on standard entry-level
workstations (4 CPU cores, 16 GB RAM, 256 GB NVMe storage) while leaving the
vast majority of system resources free for daily office workloads.

### Installed Disk Footprint

- **Standard Debian, Ubuntu, or Mint Node (`Entry16G`):** **~760 MB** total
  (**~0.3%** of a 256 GB drive). This includes stripped static runtime
  binaries (**~45–50 MB**), the native desktop Control Plane (**~10 MB**),
  initial SQLite WAL state (**~5 MB**), and the SHA-256 verified
  `LFM2.5-1.2B-Instruct-Q4_K_M.gguf` model (**697 MB**).
- **Portable Non-Debian Linux Node (AppImage fallback):** **~830 MB** total
  (**~0.32%** of a 256 GB drive).
- **Optional Firecracker KVM MicroVM Assets:** Add **~425 MB**
  (`vmlinux.bin` and `rootfs.ext4`, provisioned on demand only when `/dev/kvm`
  is enabled). This brings a full hardware-isolated microVM node to
  **~1.18 GB** (**~0.46%** of a 256 GB drive).

### Active Memory (RAM) Footprint (`Entry16G` Tier)

- **Rust Supervisor + Native Control Plane UI:** `~65–85 MB`
- **Local `llama-server` with `LFM2.5-1.2B-Instruct-Q4_K_M`**
  (`8,192` context): **~1.25 GB**
- **Total Active RAM Usage:** **~1.35 GB**. This leaves **~14.6 GB** free on
  a 16 GB machine for the OS and office applications.

## Core Architecture and Features

### 1. Hardware Auto-Scaler

The [`hw-scaler` crate](crates/hw-scaler) probes CPU cores, available RAM,
NVMe storage, and `/dev/kvm` support during startup.

- **`Entry16G` (16 GB RAM):** Physical cores minus 1, an 8,192-token context
  window, up to 4 concurrent microVMs, and a 64 MB SQLite WAL cache.
- **`Pro32G` (32 GB RAM):** Physical cores minus 2, a 16,384-token context
  window, up to 12 concurrent microVMs, and a 256 MB SQLite WAL cache.
- **`Enterprise64GPlus` (64 GB+ RAM):** Physical cores minus 2, a
  32,768-token context window, up to 32 concurrent microVMs, and a 1 GB
  SQLite WAL cache.

### 2. Hardened Rust Supervisor and Local AI Engine

The [`supervisor` crate](crates/supervisor) provides the core Rust daemon.

- **Zero exposed supervisor TCP ports:** It listens only at
  `unix:///var/lib/hartsanticloud/run/supervisor.sock` with mode `0600` and
  verifies a constant-time 256-bit token.
- **Cryptographic model provisioning:** On `Entry16G`, it loads
  `LFM2.5-1.2B-Instruct-Q4_K_M.gguf` during first run or from offline
  weights and verifies its exact SHA-256 digest:
  `b1b3de114215d9507409a662a501a631095a479a419584e8a2ded6304b19b4f5`.
- **Isolated `llama-server` worker:** It uses an upstream, statically built
  `ggml-org/llama.cpp` engine with `-DLLAMA_CURL=OFF` and `--no-webui`.
  The worker binds to an ephemeral `127.0.0.1` port and authenticates with a
  `0600` tmpfs `--api-key-file` under `/dev/shm`.
- **Opt-in cloud burst:** This is disabled by default. Oversized contexts
  route externally only when explicitly configured with
  `HARTSANTICLOUD_BURST_URL` and `allow_cloud_burst: true`.

### 3. Disposable Agent Sandbox Runner

The [`sandbox` module](crates/supervisor/src/sandbox.rs) executes untrusted
AI agent tasks and polyglot code in bounded, ephemeral sandboxes.

- **Firecracker KVM MicroVMs:** Jobs use dedicated `vmlinux.bin` and ephemeral
  `rootfs.ext4` copies, strict vCPU and memory limits, timeout enforcement,
  and automatic workspace teardown.
- **Bytecode Alliance Wasmtime (`WASI 1.0`):** WebAssembly tasks use
  `wasm32-wasip1` and strict `--max-memory-size` limits.

### 4. Pure-SQLite WAL State and Read-Only Analytics

- Transactional state, telemetry rollups, and append-only audit logs
  (`inference_audit` and `sandbox_audit`) use SQLite in WAL mode.
- Interactive analytics use a separate `SQLITE_OPEN_READ_ONLY` connection with
  `PRAGMA query_only = ON` and single-statement `SELECT` or `WITH` validation.
- Audit logs record latency, token and character counts, and estimated dollar
  savings. They do not retain raw prompt text or model outputs.

### 5. Native Tauri and Svelte Control Plane

The [`control-plane`](control-plane) uses Tauri v2 and Svelte 5. It displays
live CPU, memory, and NVMe telemetry, runs local CPU inference tests, executes
read-only SQLite audit queries, and launches disposable Firecracker or
Wasmtime sandboxes.

---

- **Real-Time Cloud Cost Savings Counter:** Tracks every local inference call
  versus opt-in cloud burst activity in the local SQLite audit log
  (`inference_audit`) and displays cumulative dollar savings in the Control
  Plane dashboard.

### Resetting Telemetry & Savings Counters

To reset the node's historical SQLite WAL telemetry, audit logs
(`sandbox_audit` and `inference_audit`), and cumulative cloud cost savings
counter back to `$0.0000`, stop the supervisor service before removing the
database files. Back up `state.db` first if you need to retain audit logs.

```bash
systemctl --user stop hartsanticloud-supervisor.service
rm -f /var/lib/hartsanticloud/sqlite/state.db*
systemctl --user start hartsanticloud-supervisor.service
```

### Tier-Scaled Model Provisioning & Bandwidth Overrides

On first boot, `hartsanticloud-supervisor` automatically provisions the Liquid
AI model matched to your node's detected RAM tier. It also provisions the
`~36 MB` `bge-small-en-v1.5-q8_0.gguf` local embedding model.

| Hardware Tier | Detected RAM | Default Liquid AI Model | First-Run Download |
| :--- | :--- | :--- | :--- |
| `Entry16G` | `< 28 GiB` | `LFM2.5-1.2B-Instruct-Q4_K_M.gguf` | `~700 MB` |
| `Pro32G` | `28 – 55 GiB` | `LFM2.5-8B-A1B-Q4_K_M.gguf` | `~5.0 GB` |
| `Enterprise64GPlus` | `>= 56 GiB` | `LFM2.5-24B-A2B-Q4_K_M.gguf` | `~14 GB` |

To keep a 32 GB+ node on the lightweight `~700 MB` `Entry16G` model, set
`HARTSANTICLOUD_FORCE_ENTRY_MODEL=1` in `/etc/default/hartsanticloud` or in
the systemd service environment. This is useful on metered connections or for
faster CPU inference while retaining `Pro32G` context, SQLite cache, and
microVM limits.

### Local Knowledge Vault, Offline Transcription & MCP Tool Bridge (`mcp-bridge`)

Every Hart's Anti-Cloud node includes an embedded `sqlite-vec` vector vault (`384`-dim `bge-small-en-v1.5-q8_0.gguf`), pure-Rust document extraction (`.pdf`, `.docx`, `.xlsx`, `.xls`, `.ods`, `.csv`, `.md`, `.txt`), offline `whisper.cpp` audio transcription (`ggml-base.en-q5_1.bin` on `Entry16G`, `ggml-small.en-q5_1.bin` on `Pro32G+`), and a zero-TCP-port **Model Context Protocol (MCP)** `stdio` bridge.

Local coding agents and IDEs (**Claude Code**, **Cursor**, **Zed**, **Continue**) can connect directly over `stdin`/`stdout` with zero open network ports while the supervisor authenticates over your `0600` `AF_UNIX` socket (`/var/lib/hartsanticloud/run/supervisor.sock`):

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

Or register it in one command with Claude Code:

```bash
claude mcp add harts-anticloud -- /opt/hartsanticloud/bin/hartsanticloud-supervisor mcp-bridge
```

| Exposed MCP Tool | Capability |
| :--- | :--- |
| `local_ai_chat` | Runs local zero-egress Liquid AI (LFM2.5) inference on the CPU, with optional `use_vault_rag` context injection. |
| `vault_ingest_document` | Extracts `.pdf`, `.docx`, `.xlsx`, `.xls`, `.ods`, `.csv`, `.md`, and `.txt` files and indexes embeddings into sqlite-vec. |
| `vault_semantic_search` | Executes 384-dimensional dense-vector KNN similarity search across the local knowledge vault. |
| `transcribe_audio_file` | Transcribes `.wav`, `.mp3`, `.ogg`, `.flac`, or `.m4a` completely offline with `whisper-cli` and can optionally index the transcript into the vault. |
| `sqlite_readonly_query` | Executes read-only `SELECT`/`WITH` diagnostics against `telemetry_rollups`, `inference_audit`, `sandbox_audit`, and `vault_documents`. |

## Repository Layout

```text
hartsanticloudsoftware.com/
├── crates/
│   ├── hw-scaler/          # Hardware probe and tier auto-scaler
│   └── supervisor/         # AF_UNIX daemon, AI worker, and sandbox manager
├── control-plane/          # Tauri v2 and Svelte 5 desktop UI
├── infra/                  # Flatcar Ignition and Butane node configs
├── scripts/
│   └── build-release-bundle.sh
├── THIRD_PARTY_PACKAGES.md
├── SECURITY.md
├── PRIVACY.md
├── TERMS.md
└── LICENSE.md
```

## Quick Start: End-User Installation

Packaged releases require no compilers or build tools.

```bash
cd dist-release
./install.sh
```

The installer places precompiled static binaries under
`/opt/hartsanticloud/bin/`, installs the `.deb` desktop package, enables the
`hartsanticloud-supervisor` systemd user service, and provisions the verified
Liquid AI model.

## Building from Source

```bash
# 1. Build the release Rust supervisor
cargo build --release -p hartsanticloud-supervisor

# 2. Run it in local development mode
HARTSANTICLOUD_DATA_DIR=/tmp/hartsanticloud-dev \
  ./target/release/hartsanticloud-supervisor

# 3. Launch the Tauri and Svelte Control Plane
cd control-plane
npm install
HARTSANTICLOUD_DATA_DIR=/tmp/hartsanticloud-dev npm run tauri dev

# 4. Generate the complete customer release bundle
./scripts/build-release-bundle.sh
```

## License and Governance

- **License:** [Apache License 2.0](LICENSE.md)
- **Privacy Policy:** [PRIVACY.md](PRIVACY.md)
- **Security Policy:** [SECURITY.md](SECURITY.md)
- **Terms of Service:** [TERMS.md](TERMS.md)
- **Third-Party Packages:** [THIRD_PARTY_PACKAGES.md](THIRD_PARTY_PACKAGES.md)
