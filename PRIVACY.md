# Privacy Policy — Hart's Anti-Cloud Software

**Effective Date:** November 17, 2026  
**Product:** Hart's Anti-Cloud Edge Node Platform (`hartsanticloudsoftware.com`)

---

## 1. Core Local-First Privacy Commitment

Hart's Anti-Cloud is engineered so your prompts, documents, SQLite databases,
and sandboxed workloads **never leave your physical hardware** during normal
operation.

- **Zero Telemetry or Phone-Home Tracking:** The supervisor, `llama-server`
  inference engine, and Tauri Control Plane collect zero usage analytics,
  crash telemetry, or behavioral tracking back to Hart's Anti-Cloud Software.
- **No Cloud Account Required:** You do not need to create an online account,
  register an email address, or authenticate against external servers to run
  local AI inference or sandboxed workloads.

## 2. Data Stored Locally on Your Machine

All operational state resides strictly inside your configured local directories
(`/var/lib/hartsanticloud` and `/opt/hartsanticloud`):

- **SQLite WAL Database (`state.db`):** Stores local hardware telemetry
  rollups, sandbox execution records (`sandbox_audit`), and inference metadata
  (`inference_audit` — timestamps, routing target, character counts, latency,
  and estimated cloud cost savings). **Raw prompt text and model responses are
  not persisted in `inference_audit`.**
- **Ephemeral Runtime Secrets (`run/` and `/dev/shm`):** Session bearer tokens
  and worker API keys are generated randomly in memory and stored with `0600`
  owner-only permissions.

## 3. Optional External Network Connections

Outbound network connections occur only in two explicit scenarios:

1. **First-Run Model Provisioning:** If
   `LFM2.5-1.2B-Instruct-Q4_K_M.gguf` is not already present in
   `/opt/hartsanticloud/models/`, the supervisor downloads it over HTTPS from
   Hugging Face (`huggingface.co`) and verifies its SHA-256 digest before use.
   Air-gapped deployments can pre-populate `/opt/hartsanticloud/models/`
   offline so zero external connections ever occur.
2. **Opt-In Cloud Burst (`allow_cloud_burst`):** Disabled by default (`false`).
   If you explicitly configure `HARTSANTICLOUD_BURST_URL` and enable
   `allow_cloud_burst` on an oversized prompt, that specific request is
   forwarded to your configured upstream endpoint.

## 4. Data Deletion & User Control

Because all data is stored locally on your disk, you retain 100% physical and
cryptographic control. Stopping the supervisor and removing
`/var/lib/hartsanticloud` and `/opt/hartsanticloud` permanently erases all
application data.

## 5. Contact

For privacy inquiries, contact `privacy@hartsanticloudsoftware.com`.
