# Security Policy & Vulnerability Reporting

## Supported Versions

We actively backport critical security patches to the current release line and
the immediate prior minor release.

| Version | Supported |
| :--- | :--- |
| `0.1.x` / `1.x` (Current) | Yes |
| `< 0.1.0` | No (Upgrade required) |

## Architectural Security Guarantees

- **Zero Exposed Local TCP Ports on the Supervisor:** The Rust supervisor binds
  strictly to a `0600` `AF_UNIX` domain socket (`supervisor.sock`) authenticated
  with a 256-bit constant-time-verified bearer token (`supervisor.token`).
- **Authenticated Ephemeral Inference Worker:** `llama-server` is compiled with
  `-DLLAMA_CURL=OFF` (no outbound C++ network stack), launched with
  `--no-webui`, bound to an OS-assigned ephemeral `127.0.0.1` port, and
  authenticated via a `0600` tmpfs `--api-key-file` in `/dev/shm` so no secret
  ever appears in process arguments.
- **Cryptographic Model Pinning:** GGUF model weights are verified against their
  SHA-256 digest before loading into memory.
- **Read-Only SQLite Isolation:** User analytics queries execute against a
  separate `SQLITE_OPEN_READ_ONLY` connection with `PRAGMA query_only = ON` and
  AST/keyword validation.

## Operational Secret Hygiene (What Never to Share)

When submitting bug reports, pasting terminal logs, or backing up an edge
node, **never share, commit, or attach**:

- **Runtime Bearer and Worker Tokens:** The generated 256-bit
  `supervisor.token` (`/var/lib/hartsanticloud/run/supervisor.token`) or
  ephemeral tmpfs worker keys
  (`/dev/shm/hartsanticloud-worker-*.key`).
- **Cloud Burst Credentials:** Any customer-supplied
  `HARTSANTICLOUD_BURST_KEY` environment variable or API secret.
- **Private Network and Endpoint Topology:** Custom
  `HARTSANTICLOUD_BURST_URL` endpoints, private NetBird WireGuard mesh IPs
  (`wt0`), or internal DNS and routing tables.

## Reporting a Vulnerability (Private Disclosure)

**Do not open public GitHub issues for suspected security vulnerabilities.**

1. **GitHub Private Vulnerability Reporting:** Navigate to the **Security** tab
   of the repository and click **"Report a vulnerability"** to open a private
   advisory thread.
2. **Direct Encrypted Email:** Send a detailed report to
   `security@hartsanticloudsoftware.com`.

### What to Include

- Affected component (e.g., Rust Supervisor `AF_UNIX` API, Firecracker/Wasmtime
  Sandbox Manager, Tauri Control Plane, or `llama-server` worker lifecycle).
- Host OS/kernel version and exact package versions from
  [THIRD_PARTY_PACKAGES.md](THIRD_PARTY_PACKAGES.md).
- Step-by-step reproduction instructions or a minimal proof-of-concept.

## Disclosure Timeline & SLA

- **Acknowledgment:** Within **24 hours** of report receipt.
- **Triage & Severity Assessment (CVSS v4.0):** Within **72 hours**.
- **Patch Release Target:** Critical sandbox escapes or IPC authentication
  bypasses are patched within **7 days**; high/medium issues within
  **14–30 days**.

## Upstream Dependency Vulnerabilities

If a vulnerability originates in an underlying package (e.g., `llama.cpp`,
Firecracker, Wasmtime, NetBird, or Tauri), please report it both to us (so we
can ship an updated pinned binary release) and to the upstream security advisory
link listed in [THIRD_PARTY_PACKAGES.md](THIRD_PARTY_PACKAGES.md).
