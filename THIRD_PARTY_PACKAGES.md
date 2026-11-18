# Third-Party Package Manifest & Upstream Security Directory

This document lists every locked dependency in **Hart's Anti-Cloud**
(`hartsanticloudsoftware.com`), its current pinned release (as of November
2026), license, upstream source for updates, and official vulnerability
reporting channels.

---

## Layer 1: Immutable Host OS & Appliance Provisioning

- **Flatcar Container Linux** — `Stable 4757.2.3` (Kernel `6.12.114`,
  Ignition `2.24.0`)
  - **Role:** Optional read-only `/usr` host OS with atomic A/B partition
    rollbacks and SSH disabled via Ignition for dedicated edge appliances.
  - **License:** Apache-2.0 / GPL-2.0 (Linux Kernel)
  - **Releases & Source:** [Flatcar
    releases](https://github.com/flatcar/Flatcar/releases) |
    [Flatcar website](https://www.flatcar.org/releases)
  - **Security Advisories:** [Flatcar security
    advisories](https://github.com/flatcar/Flatcar/security/advisories)

## Layer 2: Compute & Untrusted Agent Sandboxing

- **Bytecode Alliance Wasmtime** — `v29.0.1`
  (WASI 1.0 / `wasm32-wasip1`)
  - **Role:** Sub-millisecond memory-bounded sandboxed execution for
    deterministic polyglot agent tasks.
  - **License:** Apache-2.0 with LLVM Exception
  - **Releases & Source:**
    [Wasmtime releases](https://github.com/bytecodealliance/wasmtime/releases)
  - **Security Advisories:** [Wasmtime security advisories](https://github.com/bytecodealliance/wasmtime/security/advisories)
- **AWS Firecracker** — `v1.10.1` (KVM MicroVMs)
  - **Role:** Hardware-virtualized KVM microVMs (~30ms boot, strict vCPU and
    MiB RAM caps) for isolating untrusted AI agent code execution.
  - **License:** Apache-2.0
  - **Releases & Source:** [Firecracker releases](https://github.com/firecracker-microvm/firecracker/releases)
  - **Security Advisories:** [Firecracker security advisories](https://github.com/firecracker-microvm/firecracker/security/advisories)

## Layer 3: CPU-First Local AI Inference, Embeddings & Offline Speech

- **ggml-org llama.cpp** — `v1.0.0` (Static, headless `llama-server`)
  - **Role:** Pure-CPU GGUF inference and embedding worker bound to OS-assigned
    ephemeral loopback ports with `0600` tmpfs `--api-key-file` authentication
    and `--no-webui`.
  - **License:** MIT
  - **Releases & Source:** [llama.cpp releases](https://github.com/ggml-org/llama.cpp/releases)
  - **Security Advisories:** [llama.cpp security advisories](https://github.com/ggml-org/llama.cpp/security/advisories)
- **ggml-org whisper.cpp** — `v1.9.6-dev` (Static `whisper-cli`)
  - **Role:** Pure-CPU offline speech-to-text transcription for `.wav`, `.mp3`,
    `.ogg`, `.flac`, and `.m4a` audio files with optional automatic vault
    indexing.
  - **License:** MIT
  - **Releases & Source:** [whisper.cpp releases](https://github.com/ggml-org/whisper.cpp/releases)
  - **Security Advisories:** [whisper.cpp security advisories](https://github.com/ggml-org/whisper.cpp/security/advisories)
- **Liquid AI LFM2.5 Model Family**
  (`LFM2.5-1.2B-Instruct-Q4_K_M.gguf`, `LFM2.5-8B-A1B-Q4_K_M.gguf`, `LFM2.5-24B-A2B-Q4_K_M.gguf`)
  - **Role:** Tier-scaled CPU-optimized instruction-tuned models for structured
    JSON extraction, RAG, and office workflows with SHA-256 checksum pinning.
  - **Pinned Entry16G SHA-256:** `b1b3de114215d9507409a662a501a631095a479a419584e8a2ded6304b19b4f5`
  - **License:** LFM Open License v1.0
  - **Model Weights:** [LiquidAI/LFM2.5-1.2B-Instruct-GGUF](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Instruct-GGUF)
- **BAAI BGE-Small-en-v1.5 GGUF** (`bge-small-en-v1.5-q8_0.gguf`)
  - **Role:** Lightweight (~36 MB) 384-dimensional dense text embedding model
    for local `sqlite-vec` RAG indexing and semantic search.
  - **License:** MIT
  - **Model Weights:** [CompendiumLabs/bge-small-en-v1.5-gguf](https://huggingface.co/CompendiumLabs/bge-small-en-v1.5-gguf)
- **OpenAI Whisper GGML Quantized Weights**
  (`ggml-base.en-q5_1.bin` on `Entry16G`, `ggml-small.en-q5_1.bin` on `Pro32G+`)
  - **Role:** On-demand provisioned speech recognition weights executed locally
    by `whisper-cli`.
  - **License:** MIT
  - **Model Weights:** [ggerganov/whisper.cpp](https://huggingface.co/ggerganov/whisper.cpp)

## Layer 4: Local State, Vector Vault & Zero-Egress Storage

- **SQLite** — `v3.54.1` (Bundled via `rusqlite` `0.32.1` in WAL mode)
  - **Role:** Embedded transactional state store, hardware telemetry rollup
    engine, and append-only inference/sandbox audit log with dedicated read-only
    (`SQLITE_OPEN_READ_ONLY` + `PRAGMA query_only = ON`) query isolation.
  - **License:** Public Domain
  - **Releases & Source:** [SQLite news](https://www.sqlite.org/news.html) |
    [SQLite source](https://github.com/sqlite/sqlite)
- **sqlite-vec** — `v0.1.7-alpha.10` (Statically linked C vector extension)
  - **Role:** Embedded 384-dimensional `vec0` virtual tables for zero-egress
    local K-Nearest Neighbors (KNN) semantic search inside `state.db`.
  - **License:** Apache-2.0 / MIT
  - **Releases & Source:** [sqlite-vec releases](https://github.com/asg017/sqlite-vec/releases)
- **Pure-Rust Office & PDF Extraction Crates**
  (`pdf-extract` `0.7.12`, `lopdf` `0.34.0`, `calamine` `0.38.0`, `zip` `2.6.1`, `quick-xml` `0.37.5`)
  - **Role:** In-process `.pdf`, `.docx`, `.xlsx`/`.xls`/`.ods`, `.csv`, `.md`,
    and `.txt` to Markdown ingestion without LibreOffice, Python, or external
    subprocess dependencies.
  - **License:** MIT / Apache-2.0
  - **Releases & Source:** [crates.io](https://crates.io)
- **Deuxfleurs Garage** — `v2.5.0` (Optional S3-compatible object storage)
  - **Role:** Self-hosted S3-compatible local object store written in Rust.
  - **License:** AGPL-3.0
  - **Releases & Source:** [Garage source](https://github.com/deuxfleurs-org/garage)

## Layer 5: Zero-Trust Mesh & Kernel Security

- **NetBird** — `v0.83.1`
  - **Role:** Peer-to-peer WireGuard encrypted overlay mesh (`wt0`) with zero
    open inbound firewall ports.
  - **License:** BSD-3-Clause
  - **Releases & Source:** [NetBird releases](https://github.com/netbirdio/netbird/releases)
  - **Security Advisories:** [NetBird security advisories](https://github.com/netbirdio/netbird/security/advisories)
- **Cilium Tetragon** — `v1.7` (Optional kernel eBPF enforcement)
  - **Role:** eBPF-based real-time kernel security observability and runtime
    process/network enforcement.
  - **License:** Apache-2.0
  - **Releases & Source:** [Tetragon releases](https://github.com/cilium/tetragon/releases)
  - **Security Advisories:** [Tetragon security advisories](https://github.com/cilium/tetragon/security/advisories)

## Layer 6: Control Plane, MCP Bridge & Local IPC

- **Official Rust MCP SDK (`rmcp`)** — `v3.11.0` (with `schemars` `v1.2.3`)
  - **Role:** Zero-TCP-port Model Context Protocol (`stdio`) bridge exposing
    local AI chat, `sqlite-vec` vault ingestion/search, `whisper.cpp`
    transcription, and read-only SQLite diagnostics to local IDEs and coding
    agents over `0600` `AF_UNIX` `supervisor.sock`.
  - **License:** MIT / Apache-2.0
  - **Releases & Source:** [modelcontextprotocol/rust-sdk](https://github.com/modelcontextprotocol/rust-sdk)
- **Tauri** — `v2.12.6`
  - **Role:** Native Rust desktop control plane communicating exclusively over
    a `0600` `AF_UNIX` domain socket (`supervisor.sock`) with a 256-bit
    constant-time verified bearer token.
  - **License:** Apache-2.0 / MIT
  - **Releases & Source:** [Tauri releases](https://github.com/tauri-apps/tauri/releases)
  - **Security Advisories:** [Tauri security advisories](https://github.com/tauri-apps/tauri/security/advisories)
- **Svelte** — `v5` (with Vite `v6.4.6`)
  - **Role:** Zero-runtime-overhead compiled UI framework for the local node console.
  - **License:** MIT
  - **Releases & Source:** [Svelte releases](https://github.com/sveltejs/svelte/releases)
  - **Security Advisories:** [Svelte security advisories](https://github.com/sveltejs/svelte/security/advisories)
