export interface HardwareTelemetry {
  physical_cores: number;
  logical_cores: number;
  total_ram_bytes: number;
  available_ram_bytes: number;
  total_disk_bytes: number;
  available_disk_bytes: number;
  kvm_available: boolean;
  cpu_brand: string;
}

export interface RuntimeBudget {
  tier: 'Entry16G' | 'Pro32G' | 'Enterprise64GPlus';
  telemetry: HardwareTelemetry;
  ai_threads: number;
  ai_context_tokens: number;
  ai_model_filename: string;
  ai_engine_backend: string;
  max_firecracker_microvms: number;
  firecracker_vcpu_per_vm: number;
  firecracker_mem_mib_per_vm: number;
  sqlite_cache_size_kb: number;
  sqlite_mmap_size_bytes: number;
  garage_cache_mb: number;
}

export interface NodeDashboardSnapshot {
  supervisor_online: boolean;
  supervisor_url: string;
  local_budget: RuntimeBudget;
  live_telemetry: HardwareTelemetry;
  inference_backend_healthy: boolean;
  total_local_requests: number;
  total_burst_requests: number;
  estimated_cloud_savings_usd: number;
  active_firecracker_vms: number;
  kvm_assets_ready?: boolean;
  netbird_interface_up: boolean;
  vision_mmproj_active: boolean;
  mmproj_filename?: string | null;
  firecracker_jailer_ready: boolean;
  ocr_engine_ready: boolean;
}
