<script lang="ts">
  import { invoke } from '@tauri-apps/api/core';
  import { onMount } from 'svelte';
  import type { NodeDashboardSnapshot } from './lib/types';

  let snapshot = $state<NodeDashboardSnapshot | null>(null);
  let errorMsg = $state<string | null>(null);
  let promptInput = $state('');
  let inferenceReply = $state('');
  let runningPrompt = $state(false);
  let sqlInput = $state('SELECT recorded_at, tier, available_ram_mb, active_vms FROM telemetry_rollups ORDER BY recorded_at DESC LIMIT 25');
  let sqlResult = $state<string>('');
  let sandboxRuntime = $state('firecracker');
  let sandboxWorkload = $state('agent-shell-task');
  let sandboxPayload = $state('{"task":"healthcheck"}');
  let sandboxResult = $state<string>('');
  let runningSandbox = $state(false);
  let provisioningKvm = $state(false);
  let useVaultRag = $state(true);
  let vaultFilePath = $state('');
  let vaultNoteTitle = $state('');
  let vaultNoteBody = $state('');
  let vaultSearchQuery = $state('');
  let vaultOutput = $state('');
  let busyVault = $state(false);
  let pickingVaultFile = $state(false);
  let audioFilePath = $state('');
  let audioIngestVault = $state(true);
  let audioOutput = $state('');
  let busyAudio = $state(false);

  const ramUsedPct = $derived.by(() => {
    if (!snapshot) return 0;
    const { total_ram_bytes, available_ram_bytes } = snapshot.live_telemetry;
    if (total_ram_bytes === 0) return 0;
    return Math.round(((total_ram_bytes - available_ram_bytes) / total_ram_bytes) * 100);
  });

  const diskUsedPct = $derived.by(() => {
    if (!snapshot) return 0;
    const { total_disk_bytes, available_disk_bytes } = snapshot.live_telemetry;
    if (total_disk_bytes === 0) return 0;
    return Math.round(((total_disk_bytes - available_disk_bytes) / total_disk_bytes) * 100);
  });

  function formatGiB(bytes: number): string {
    return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GiB';
  }

  async function refreshTelemetry() {
    try {
      snapshot = await invoke<NodeDashboardSnapshot>('get_node_snapshot');
      errorMsg = null;
    } catch (err) {
      errorMsg = String(err);
    }
  }

  async function runLocalInference() {
    if (!promptInput.trim() || runningPrompt) return;
    runningPrompt = true;
    inferenceReply = '';
    try {
      inferenceReply = await invoke<string>('send_local_prompt', { prompt: promptInput, useVaultRag });
      await refreshTelemetry();
    } catch (err) {
      inferenceReply = `Error: ${String(err)}`;
    } finally {
      runningPrompt = false;
    }
  }

  async function runSqlQuery() {
    try {
      const res = await invoke<unknown>('execute_sqlite_query', { sql: sqlInput });
      sqlResult = JSON.stringify(res, null, 2);
    } catch (err) {
      sqlResult = `Query Error: ${String(err)}`;
    }
  }

  async function provisionKvmAssets() {
    if (provisioningKvm) return;
    provisioningKvm = true;
    sandboxResult = 'Downloading Firecracker Linux kernel & ext4 rootfs (~425 MB)...';
    try {
      const res = await invoke<unknown>('provision_kvm_assets');
      sandboxResult = JSON.stringify(res, null, 2);
      await refreshTelemetry();
    } catch (err) {
      sandboxResult = `KVM Provisioning Error: ${String(err)}`;
    } finally {
      provisioningKvm = false;
    }
  }

  async function executeSandbox() {
    if (runningSandbox) return;
    runningSandbox = true;
    sandboxResult = '';
    try {
      const res = await invoke<unknown>('run_sandbox_task', {
        runtime: sandboxRuntime,
        workloadName: sandboxWorkload,
        payload: sandboxPayload
      });
      sandboxResult = JSON.stringify(res, null, 2);
      await refreshTelemetry();
    } catch (err) {
      sandboxResult = `Sandbox Error: ${String(err)}`;
    } finally {
      runningSandbox = false;
    }
  }

  async function handleBrowseVaultFile() {
    if (busyVault || pickingVaultFile) return;
    pickingVaultFile = true;
    try {
      const selected = await invoke<string | null>('pick_vault_file');
      if (selected) {
        vaultFilePath = selected;
        await handleVaultIngest();
      }
    } catch (err) {
      vaultOutput = `File Picker Error: ${String(err)}`;
    } finally {
      pickingVaultFile = false;
    }
  }

  async function handleVaultIngest() {
    if (busyVault) return;
    if (!vaultFilePath.trim() && !vaultNoteBody.trim()) return;
    busyVault = true;
    vaultOutput = 'Extracting text / running offline OCR & embedding into sqlite-vec...';
    try {
      const res = await invoke<unknown>('ingest_vault_item', {
        filePath: vaultFilePath.trim() || null,
        title: vaultNoteTitle.trim() || null,
        markdown: vaultNoteBody.trim() || null
      });
      vaultOutput = JSON.stringify(res, null, 2);
      vaultFilePath = '';
      vaultNoteBody = '';
    } catch (err) {
      vaultOutput = `Vault Ingest Error: ${String(err)}`;
    } finally {
      busyVault = false;
    }
  }

  async function handleVaultSearch() {
    if (busyVault || !vaultSearchQuery.trim()) return;
    busyVault = true;
    vaultOutput = 'Searching 384-dim sqlite-vec index...';
    try {
      const res = await invoke<unknown>('search_vault_items', {
        query: vaultSearchQuery,
        topK: 4
      });
      vaultOutput = JSON.stringify(res, null, 2);
    } catch (err) {
      vaultOutput = `Vault Search Error: ${String(err)}`;
    } finally {
      busyVault = false;
    }
  }

  async function handleAudioTranscribe() {
    if (busyAudio || !audioFilePath.trim()) return;
    busyAudio = true;
    audioOutput = 'Transcribing offline with whisper.cpp...';
    try {
      const res = await invoke<unknown>('transcribe_audio_item', {
        filePath: audioFilePath.trim(),
        ingestToVault: audioIngestVault
      });
      audioOutput = JSON.stringify(res, null, 2);
    } catch (err) {
      audioOutput = `Transcription Error: ${String(err)}`;
    } finally {
      busyAudio = false;
    }
  }

  onMount(() => {
    refreshTelemetry();
    const timer = setInterval(refreshTelemetry, 4000);
    return () => clearInterval(timer);
  });
</script>

<main style="max-width:1180px;margin:0 auto;padding:24px;">
  <header style="display:flex;justify-content:space-between;align-items:center;flex-wrap:wrap;gap:12px;border-bottom:1px solid #21262d;padding-bottom:16px;margin-bottom:24px;">
    <div>
      <h1 style="margin:0;font-size:1.4rem;letter-spacing:-0.02em;">hartsanticloudsoftware.com — Edge Node Control Plane</h1>
      <p style="margin:4px 0 0;color:#8b949e;font-size:0.88rem;">Zero-SSH Flatcar Host · Firecracker Jailer + Wasmtime · Liquid AI LFM2.5 CPU & Vision Inference</p>
    </div>
    {#if snapshot}
      <div style="display:flex;gap:8px;align-items:center;flex-wrap:wrap;">
        <span style="padding:4px 10px;border-radius:999px;font-size:0.78rem;background:#161b22;border:1px solid #30363d;">
          Tier: <strong>{snapshot.local_budget.tier}</strong>
        </span>
        <span
          title={snapshot.mmproj_filename ? `Active projector: ${snapshot.mmproj_filename}` : 'Drop an *mmproj*.gguf projector into /opt/hartsanticloud/models to enable multimodal vision'}
          style="padding:4px 10px;border-radius:999px;font-size:0.78rem;background:{snapshot.vision_mmproj_active ? '#12261e' : '#161b22'};color:{snapshot.vision_mmproj_active ? '#3fb950' : '#8b949e'};border:1px solid {snapshot.vision_mmproj_active ? '#238636' : '#30363d'};"
        >
          Vision (--mmproj): <strong>{snapshot.vision_mmproj_active ? 'Active' : 'Standby'}</strong>
        </span>
        <span
          title={snapshot.firecracker_jailer_ready ? 'Firecracker KVM Jailer chroot + seccomp-level 2 isolation active' : 'Using direct VMM or Wasmtime WASI sandbox fallback'}
          style="padding:4px 10px;border-radius:999px;font-size:0.78rem;background:{snapshot.firecracker_jailer_ready ? '#13233a' : '#161b22'};color:{snapshot.firecracker_jailer_ready ? '#58a6ff' : '#8b949e'};border:1px solid {snapshot.firecracker_jailer_ready ? '#1f6feb' : '#30363d'};"
        >
          Jailer: <strong>{snapshot.firecracker_jailer_ready ? 'Seccomp + Chroot' : 'WASI / Direct VMM'}</strong>
        </span>
        <span style="padding:4px 10px;border-radius:999px;font-size:0.78rem;background:{snapshot.supervisor_online ? '#12261e' : '#2d1619'};color:{snapshot.supervisor_online ? '#3fb950' : '#f85149'};border:1px solid currentColor;">
          {snapshot.supervisor_online ? 'Supervisor Authenticated' : 'Supervisor Offline (Local Probe)'}
        </span>
      </div>
    {/if}
  </header>

  {#if errorMsg}
    <div style="background:#2d1619;border:1px solid #f85149;color:#ffa198;padding:12px;border-radius:8px;margin-bottom:16px;">
      {errorMsg}
    </div>
  {/if}

  {#if snapshot}
    <section style="display:grid;grid-template-columns:repeat(4,1fr);gap:14px;margin-bottom:24px;">
      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <div style="color:#8b949e;font-size:0.8rem;">COMPUTE TOPOLOGY</div>
        <div style="font-size:1.35rem;font-weight:700;margin:6px 0;">{snapshot.live_telemetry.logical_cores} vCPU / {snapshot.live_telemetry.physical_cores} Physical</div>
        <div style="font-size:0.8rem;color:#8b949e;">{snapshot.live_telemetry.cpu_brand}</div>
      </div>

      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <div style="color:#8b949e;font-size:0.8rem;">MEMORY FOOTPRINT ({ramUsedPct}%)</div>
        <div style="font-size:1.35rem;font-weight:700;margin:6px 0;">{formatGiB(snapshot.live_telemetry.available_ram_bytes)} Free</div>
        <div style="font-size:0.8rem;color:#8b949e;">Total: {formatGiB(snapshot.live_telemetry.total_ram_bytes)}</div>
      </div>

      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <div style="color:#8b949e;font-size:0.8rem;">NVME STORAGE ({diskUsedPct}%)</div>
        <div style="font-size:1.35rem;font-weight:700;margin:6px 0;">{formatGiB(snapshot.live_telemetry.available_disk_bytes)} Free</div>
        <div style="font-size:0.8rem;color:#8b949e;">Total: {formatGiB(snapshot.live_telemetry.total_disk_bytes)}</div>
      </div>

      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <div style="color:#8b949e;font-size:0.8rem;">ESTIMATED CLOUD SAVINGS</div>
        <div style="font-size:1.35rem;font-weight:700;margin:6px 0;color:#3fb950;">${snapshot.estimated_cloud_savings_usd.toFixed(4)}</div>
        <div style="font-size:0.8rem;color:#8b949e;">Local Calls: {snapshot.total_local_requests} · Burst: {snapshot.total_burst_requests}</div>
      </div>
    </section>

    <section style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;margin-bottom:24px;">
      <h2 style="margin:0 0 12px;font-size:1rem;">Auto-Scaled Runtime Allocation</h2>
      <div style="display:grid;grid-template-columns:repeat(3,1fr);gap:12px;font-size:0.86rem;">
        <div><strong>AI Backend:</strong> {snapshot.local_budget.ai_engine_backend} ({snapshot.local_budget.ai_threads} threads)</div>
        <div><strong>Active Model:</strong> {snapshot.local_budget.ai_model_filename}</div>
        <div><strong>Context Window:</strong> {snapshot.local_budget.ai_context_tokens} tokens</div>
        <div><strong>Vision Projector (--mmproj):</strong> {snapshot.vision_mmproj_active ? (snapshot.mmproj_filename ?? 'Loaded') : 'Not Loaded (Text-Only)'}</div>
        <div><strong>Firecracker Jailer:</strong> {snapshot.firecracker_jailer_ready ? 'Chroot + Seccomp-2 Ready' : (snapshot.live_telemetry.kvm_available ? 'KVM Direct / WASI' : 'Wasmtime WASI')} ({snapshot.active_firecracker_vms}/{snapshot.local_budget.max_firecracker_microvms} VMs)</div>
        <div><strong>Offline OCR (Tesseract):</strong> {snapshot.ocr_engine_ready ? 'pdftoppm + tesseract Ready' : 'Native PDF/Office Text'} · <strong>NetBird:</strong> {snapshot.netbird_interface_up ? 'wt0 Active' : 'Standby'}</div>
      </div>
    </section>

    <section style="display:grid;grid-template-columns:1fr 1fr;gap:16px;">
      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <h2 style="margin:0 0 10px;font-size:1rem;">Local CPU AI Inference Test</h2>
        <textarea
          bind:value={promptInput}
          rows="4"
          maxlength="32000"
          placeholder="Send a prompt directly to local Liquid AI LFM2.5 via the authenticated Rust supervisor..."
          style="width:100%;box-sizing:border-box;background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:10px;font-family:inherit;"
        ></textarea>
        <div style="display:flex;justify-content:space-between;align-items:center;margin-top:10px;gap:12px;">
          <button
            onclick={runLocalInference}
            disabled={runningPrompt}
            style="background:#238636;color:#fff;border:none;padding:8px 16px;border-radius:6px;cursor:pointer;font-weight:600;"
          >
            {runningPrompt ? 'Executing on Local CPU...' : 'Run Local Inference'}
          </button>
          <label style="display:flex;align-items:center;gap:6px;font-size:0.82rem;color:#c9d1d9;cursor:pointer;">
            <input type="checkbox" bind:checked={useVaultRag} />
            Ground with Local Vault (RAG)
          </label>
        </div>
        {#if inferenceReply}
          <pre style="margin-top:12px;background:#0d1117;border:1px solid #30363d;padding:10px;border-radius:6px;white-space:pre-wrap;font-size:0.84rem;">{inferenceReply}</pre>
        {/if}
      </div>

      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <h2 style="margin:0 0 10px;font-size:1rem;">Read-Only SQLite WAL Telemetry Console</h2>
        <textarea
          bind:value={sqlInput}
          rows="4"
          maxlength="4096"
          style="width:100%;box-sizing:border-box;background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:10px;font-family:monospace;font-size:0.82rem;"
        ></textarea>
        <button
          onclick={runSqlQuery}
          style="margin-top:10px;background:#1f6feb;color:#fff;border:none;padding:8px 16px;border-radius:6px;cursor:pointer;font-weight:600;"
        >
          Execute SQLite Query
        </button>
        {#if sqlResult}
          <pre style="margin-top:12px;background:#0d1117;border:1px solid #30363d;padding:10px;border-radius:6px;max-height:180px;overflow:auto;font-size:0.8rem;">{sqlResult}</pre>
        {/if}
      </div>
    </section>

    <section style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;margin-top:16px;">
      <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:10px;gap:12px;flex-wrap:wrap;">
        <div style="display:flex;align-items:center;gap:10px;flex-wrap:wrap;">
          <h2 style="margin:0;font-size:1rem;">Disposable Agent Sandbox Runner (Firecracker KVM / Wasmtime)</h2>
          <span style="font-size:0.75rem;padding:2px 8px;border-radius:999px;background:{snapshot.firecracker_jailer_ready ? '#13233a' : '#21262d'};color:{snapshot.firecracker_jailer_ready ? '#58a6ff' : '#8b949e'};border:1px solid {snapshot.firecracker_jailer_ready ? '#1f6feb' : '#30363d'};">
            {snapshot.firecracker_jailer_ready ? 'Jailer Chroot + Seccomp-2 Active' : 'Direct / WASI Isolation'}
          </span>
        </div>
        {#if sandboxRuntime === 'firecracker'}
          {#if snapshot?.kvm_assets_ready}
            <span style="font-size:0.78rem;color:#3fb950;border:1px solid #238636;padding:4px 10px;border-radius:6px;font-weight:600;">
              KVM Assets Ready (~425 MB disk)
            </span>
          {:else}
            <button
              onclick={provisionKvmAssets}
              disabled={provisioningKvm}
              style="background:#1f6feb;color:#fff;border:none;border-radius:6px;padding:6px 12px;font-size:0.8rem;font-weight:600;cursor:pointer;"
            >
              {provisioningKvm ? 'Downloading (~425 MB)...' : 'Install Firecracker KVM (~425 MB disk)'}
            </button>
          {/if}
        {/if}
      </div>
      <div style="display:grid;grid-template-columns:180px 220px 1fr auto;gap:10px;align-items:center;">
        <select bind:value={sandboxRuntime} style="background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;color-scheme:dark;">
          <option value="firecracker">Firecracker KVM</option>
          <option value="wasmtime">Wasmtime WASI 1.0</option>
        </select>
        <input bind:value={sandboxWorkload} placeholder="Workload ID" style="background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;" />
        <input bind:value={sandboxPayload} placeholder="JSON input payload" style="background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;font-family:monospace;font-size:0.82rem;" />
        <button onclick={executeSandbox} disabled={runningSandbox} style="background:#8957e5;color:#fff;border:none;padding:8px 16px;border-radius:6px;cursor:pointer;font-weight:600;">
          {runningSandbox ? 'Isolating...' : 'Launch Sandbox'}
        </button>
      </div>
      {#if sandboxResult}
        <pre style="margin-top:12px;background:#0d1117;border:1px solid #30363d;padding:10px;border-radius:6px;max-height:160px;overflow:auto;font-size:0.8rem;">{sandboxResult}</pre>
      {/if}
    </section>

    <section style="display:grid;grid-template-columns:1fr 1fr;gap:16px;margin-top:16px;">
      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <div style="display:flex;justify-content:space-between;align-items:center;margin-bottom:10px;gap:8px;">
          <h2 style="margin:0;font-size:1rem;">Local Knowledge Vault (sqlite-vec + PDF/OCR)</h2>
          <span style="font-size:0.74rem;color:{snapshot.ocr_engine_ready ? '#3fb950' : '#8b949e'};">
            {snapshot.ocr_engine_ready ? 'OCR Enabled' : 'PDF/Office Text'}
          </span>
        </div>
        <div style="display:grid;grid-template-columns:1fr auto auto;gap:8px;margin-bottom:8px;">
          <input bind:value={vaultFilePath} placeholder="Path (.pdf, .png, .jpg, .webp, .tiff, .docx, .xlsx, .md)" style="background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;font-size:0.82rem;" />
          <button
            onclick={handleBrowseVaultFile}
            disabled={busyVault || pickingVaultFile}
            title="Select a PDF, scanned image, or Office document to index"
            style="background:#30363d;color:#e6edf3;border:1px solid #484f58;padding:8px 12px;border-radius:6px;cursor:pointer;font-weight:600;font-size:0.82rem;"
          >
            {pickingVaultFile ? 'Opening...' : 'Browse PDF/Image...'}
          </button>
          <button onclick={handleVaultIngest} disabled={busyVault} style="background:#238636;color:#fff;border:none;padding:8px 14px;border-radius:6px;cursor:pointer;font-weight:600;">
            Index File
          </button>
        </div>
        <div style="display:grid;grid-template-columns:160px 1fr auto;gap:8px;margin-bottom:10px;">
          <input bind:value={vaultNoteTitle} placeholder="Note title.md" style="background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;font-size:0.82rem;" />
          <input bind:value={vaultNoteBody} placeholder="Or paste quick Markdown note to index..." style="background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;font-size:0.82rem;" />
          <button onclick={handleVaultIngest} disabled={busyVault} style="background:#2ea043;color:#fff;border:none;padding:8px 14px;border-radius:6px;cursor:pointer;font-weight:600;">
            Index Note
          </button>
        </div>
        <div style="display:grid;grid-template-columns:1fr auto;gap:8px;">
          <input bind:value={vaultSearchQuery} placeholder="Semantic vector search across indexed documents..." style="background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;font-size:0.82rem;" />
          <button onclick={handleVaultSearch} disabled={busyVault} style="background:#1f6feb;color:#fff;border:none;padding:8px 14px;border-radius:6px;cursor:pointer;font-weight:600;">
            KNN Search
          </button>
        </div>
        {#if vaultOutput}
          <pre style="margin-top:10px;background:#0d1117;border:1px solid #30363d;padding:10px;border-radius:6px;max-height:170px;overflow:auto;font-size:0.78rem;">{vaultOutput}</pre>
        {/if}
      </div>

      <div style="background:#161b22;border:1px solid #30363d;border-radius:10px;padding:16px;">
        <h2 style="margin:0 0 10px;font-size:1rem;">Offline Voice & Meeting Transcription (whisper.cpp)</h2>
        <p style="margin:0 0 10px;color:#8b949e;font-size:0.82rem;">Transcribes .wav, .mp3, .ogg, .flac, or .m4a locally on CPU and optionally indexes into your vault.</p>
        <input bind:value={audioFilePath} placeholder="/path/to/meeting-recording.wav" style="width:100%;box-sizing:border-box;background:#0d1117;color:#e6edf3;border:1px solid #30363d;border-radius:6px;padding:8px;font-size:0.82rem;" />
        <div style="display:flex;justify-content:space-between;align-items:center;margin-top:10px;gap:12px;">
          <button onclick={handleAudioTranscribe} disabled={busyAudio} style="background:#8957e5;color:#fff;border:none;padding:8px 16px;border-radius:6px;cursor:pointer;font-weight:600;">
            {busyAudio ? 'Transcribing Offline...' : 'Transcribe Audio'}
          </button>
          <label style="display:flex;align-items:center;gap:6px;font-size:0.82rem;color:#c9d1d9;cursor:pointer;">
            <input type="checkbox" bind:checked={audioIngestVault} />
            Auto-index transcript into Vault
          </label>
        </div>
        {#if audioOutput}
          <pre style="margin-top:10px;background:#0d1117;border:1px solid #30363d;padding:10px;border-radius:6px;max-height:170px;overflow:auto;font-size:0.78rem;">{audioOutput}</pre>
        {/if}
      </div>
    </section>

  {/if}
</main>
