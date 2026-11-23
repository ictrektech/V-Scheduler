/**
 * 指标板 profile 注册表（数据驱动版面）。
 *
 * - `systemProfile`：node-exporter 系统基础指标（CPU / 内存 / 负载 / 磁盘 / 网络 / IO / 文件系统 / 时长）；
 * - `nvidiaGpuProfile`：nvidia_gpu_exporter 的 NVIDIA GPU 指标（利用率 / 显存 / 温度 / 功耗 / 时钟 /
 *   进程 / 采集健康）；
 * - `nodeMetricsProfile(node)`：按节点 `gpu_vendor` 组合（非 NVIDIA 仅有系统分区）。
 *
 * 标题 / 序列名以 i18n key 声明（`metrics.*`），由 `buildBoard` 在构建时按当前语言解析。
 * 面板**始终渲染**（即使指标组缺失，见 `buildBoard`），以在 Orin NX 等受限设备上保持标准版面并
 * 显示空态；后续运行配置详情页可新增 profile 复用同一指标板组件。
 */
import type { NodeItem } from '@/api/nodes';
import type { MetricsProfile } from './metrics';

/** 系统指标分区（node-exporter） */
export const systemCategory = {
  key: 'system',
  titleKey: 'metrics.system.title',
  category: 'system',
  stats: [
    { key: 'cpu', titleKey: 'metrics.system.cpuUsage', group: 'cpu_usage' },
    { key: 'mem', titleKey: 'metrics.system.memUsage', group: 'mem_usage' },
    { key: 'load', titleKey: 'metrics.system.load1', group: 'load1' },
    {
      key: 'disk',
      titleKey: 'metrics.system.diskUsageMax',
      group: 'disk_usage',
      reduce: 'maxSeries' as const,
    },
    { key: 'uptime', titleKey: 'metrics.system.uptime', group: 'uptime' },
  ],
  panels: [
    { key: 'cpu', titleKey: 'metrics.system.cpuUsage', members: [{ group: 'cpu_usage', nameKey: 'metrics.system.cpu' }] },
    {
      key: 'load',
      titleKey: 'metrics.system.load',
      unit: '',
      members: [
        { group: 'load1', nameKey: 'metrics.system.load1Series' },
        { group: 'load5', nameKey: 'metrics.system.load5Series' },
        { group: 'load15', nameKey: 'metrics.system.load15Series' },
      ],
    },
    { key: 'mem', titleKey: 'metrics.system.memUsage', members: [{ group: 'mem_usage', nameKey: 'metrics.system.mem' }] },
    { key: 'disk', titleKey: 'metrics.system.disk', members: [{ group: 'disk_usage' }] },
    {
      key: 'net',
      titleKey: 'metrics.system.net',
      unit: 'B/s',
      members: [
        { group: 'net_rx', nameKey: 'metrics.system.netRx' },
        { group: 'net_tx', nameKey: 'metrics.system.netTx' },
      ],
    },
    {
      key: 'io',
      titleKey: 'metrics.system.io',
      unit: 'B/s',
      members: [
        { group: 'disk_read', nameKey: 'metrics.system.diskRead' },
        { group: 'disk_write', nameKey: 'metrics.system.diskWrite' },
      ],
    },
    { key: 'fs', titleKey: 'metrics.system.fs', unit: 'B', members: [{ group: 'fs_avail' }] },
  ],
};

/** NVIDIA GPU 指标分区（nvidia_gpu_exporter） */
export const nvidiaCategory = {
  key: 'nvidia',
  titleKey: 'metrics.nvidia.title',
  category: 'gpu',
  stats: [
    { key: 'gpu_util', titleKey: 'metrics.nvidia.gpuUtil', group: 'gpu_util' },
    { key: 'gpu_mem_pct', titleKey: 'metrics.nvidia.gpuMemPct', group: 'gpu_mem_pct' },
    { key: 'gpu_proc_mem', titleKey: 'metrics.nvidia.gpuProcMem', group: 'gpu_proc_mem' },
    { key: 'gpu_temp', titleKey: 'metrics.nvidia.gpuTemp', group: 'gpu_temp' },
    { key: 'gpu_power', titleKey: 'metrics.nvidia.gpuPower', group: 'gpu_power' },
  ],
  panels: [
    { key: 'gpu_util', titleKey: 'metrics.nvidia.gpuUtil', members: [{ group: 'gpu_util' }] },
    {
      key: 'gpu_mem',
      titleKey: 'metrics.nvidia.gpuMem',
      unit: 'B',
      members: [
        { group: 'gpu_mem_used', nameKey: 'metrics.nvidia.memUsed' },
        { group: 'gpu_mem_total', nameKey: 'metrics.nvidia.memTotal' },
        { group: 'gpu_proc_mem', nameKey: 'metrics.nvidia.procUsed' },
      ],
    },
    { key: 'gpu_mem_pct', titleKey: 'metrics.nvidia.gpuMemPct', members: [{ group: 'gpu_mem_pct' }] },
    {
      key: 'gpu_temp',
      titleKey: 'metrics.nvidia.temp',
      unit: '°C',
      members: [
        { group: 'gpu_temp', nameKey: 'metrics.nvidia.tempGpu' },
        { group: 'gpu_temp_mem', nameKey: 'metrics.nvidia.tempMem' },
      ],
    },
    {
      key: 'gpu_power',
      titleKey: 'metrics.nvidia.power',
      unit: 'W',
      members: [
        { group: 'gpu_power', nameKey: 'metrics.nvidia.powerCurrent' },
        { group: 'gpu_power_limit', nameKey: 'metrics.nvidia.powerLimit' },
      ],
    },
    {
      key: 'gpu_clocks',
      titleKey: 'metrics.nvidia.clocks',
      unit: 'Hz',
      members: [
        { group: 'gpu_clocks_graphics', nameKey: 'metrics.nvidia.clocksGraphics' },
        { group: 'gpu_clocks_sm', nameKey: 'metrics.nvidia.clocksSm' },
        { group: 'gpu_clocks_memory', nameKey: 'metrics.nvidia.clocksMemory' },
      ],
    },
    { key: 'gpu_procs', titleKey: 'metrics.nvidia.procs', unit: '', members: [{ group: 'gpu_procs' }] },
    {
      key: 'gpu_health',
      titleKey: 'metrics.nvidia.health',
      unit: '',
      members: [
        { group: 'gpu_collect_success', nameKey: 'metrics.nvidia.collectSuccess' },
        { group: 'gpu_command_exit_code', nameKey: 'metrics.nvidia.commandExitCode' },
      ],
    },
  ],
};

/** 推理指标分区（vLLM，`category=inference`） */
export const vllmInferenceCategory = {
  key: 'inference',
  titleKey: 'metrics.inference.title',
  category: 'inference',
  stats: [
    { key: 'running', titleKey: 'metrics.inference.running', group: 'vllm_running' },
    { key: 'waiting', titleKey: 'metrics.inference.waiting', group: 'vllm_waiting' },
    { key: 'kv_cache', titleKey: 'metrics.inference.kvCache', group: 'vllm_kv_cache' },
    { key: 'gen_throughput', titleKey: 'metrics.inference.genThroughput', group: 'vllm_gen_throughput' },
  ],
  panels: [
    {
      key: 'throughput',
      titleKey: 'metrics.inference.throughput',
      unit: 'tok/s',
      members: [
        { group: 'vllm_gen_throughput', nameKey: 'metrics.inference.throughputOut' },
        { group: 'vllm_prompt_throughput', nameKey: 'metrics.inference.throughputIn' },
      ],
    },
    {
      key: 'requests',
      titleKey: 'metrics.inference.requests',
      unit: '',
      members: [
        { group: 'vllm_running', nameKey: 'metrics.inference.requestsRunning' },
        { group: 'vllm_waiting', nameKey: 'metrics.inference.requestsWaiting' },
      ],
    },
    { key: 'kv_cache', titleKey: 'metrics.inference.kvCache', unit: '%', members: [{ group: 'vllm_kv_cache' }] },
    {
      key: 'ttft',
      titleKey: 'metrics.inference.ttft',
      unit: 's',
      members: [
        { group: 'vllm_ttft_p50', nameKey: 'metrics.inference.p50' },
        { group: 'vllm_ttft_p95', nameKey: 'metrics.inference.p95' },
      ],
    },
    {
      key: 'tpot',
      titleKey: 'metrics.inference.tpot',
      unit: 's',
      members: [
        { group: 'vllm_tpot_p50', nameKey: 'metrics.inference.p50' },
        { group: 'vllm_tpot_p95', nameKey: 'metrics.inference.p95' },
      ],
    },
    {
      key: 'e2e',
      titleKey: 'metrics.inference.e2e',
      unit: 's',
      members: [
        { group: 'vllm_e2e_p50', nameKey: 'metrics.inference.p50' },
        { group: 'vllm_e2e_p95', nameKey: 'metrics.inference.p95' },
        { group: 'vllm_e2e_p99', nameKey: 'metrics.inference.p99' },
      ],
    },
    {
      key: 'phase',
      titleKey: 'metrics.inference.phase',
      unit: 's',
      members: [
        { group: 'vllm_queue_time', nameKey: 'metrics.inference.phaseQueue' },
        { group: 'vllm_prefill_time', nameKey: 'metrics.inference.phasePrefill' },
        { group: 'vllm_decode_time', nameKey: 'metrics.inference.phaseDecode' },
      ],
    },
    { key: 'prefix_hit', titleKey: 'metrics.inference.prefixHit', unit: '%', members: [{ group: 'vllm_prefix_hit' }] },
    { key: 'preemptions', titleKey: 'metrics.inference.preemptions', unit: '/s', members: [{ group: 'vllm_preemptions' }] },
    {
      key: 'success',
      titleKey: 'metrics.inference.success',
      unit: '/s',
      members: [{ group: 'vllm_success' }],
    },
  ],
};

/** vLLM 推理指标 profile（单分区） */
export const vllmMetricsProfile: MetricsProfile = {
  id: 'vllm-inference',
  categories: [vllmInferenceCategory],
};

/** SGLang 推理指标分区（`category=inference`、group key 前缀 `sglang_`） */
export const sglangInferenceCategory = {
  key: 'inference',
  titleKey: 'metrics.inference.title',
  category: 'inference',
  stats: [
    { key: 'running', titleKey: 'metrics.inference.running', group: 'sglang_running' },
    { key: 'waiting', titleKey: 'metrics.inference.waiting', group: 'sglang_waiting' },
    { key: 'token_usage', titleKey: 'metrics.inference.kvCache', group: 'sglang_token_usage' },
    {
      key: 'gen_throughput',
      titleKey: 'metrics.inference.genThroughput',
      group: 'sglang_gen_throughput',
    },
  ],
  panels: [
    {
      key: 'throughput',
      titleKey: 'metrics.inference.throughput',
      unit: 'tok/s',
      members: [
        { group: 'sglang_gen_throughput', nameKey: 'metrics.inference.throughputOut' },
        { group: 'sglang_prompt_throughput', nameKey: 'metrics.inference.throughputIn' },
      ],
    },
    {
      key: 'requests',
      titleKey: 'metrics.inference.requests',
      unit: '',
      members: [
        { group: 'sglang_running', nameKey: 'metrics.inference.requestsRunning' },
        { group: 'sglang_waiting', nameKey: 'metrics.inference.requestsWaiting' },
      ],
    },
    {
      key: 'token_usage',
      titleKey: 'metrics.inference.kvCache',
      unit: '%',
      members: [{ group: 'sglang_token_usage' }],
    },
    {
      key: 'ttft',
      titleKey: 'metrics.inference.ttft',
      unit: 's',
      members: [
        { group: 'sglang_ttft_p50', nameKey: 'metrics.inference.p50' },
        { group: 'sglang_ttft_p95', nameKey: 'metrics.inference.p95' },
      ],
    },
    {
      key: 'tpot',
      titleKey: 'metrics.inference.tpot',
      unit: 's',
      members: [
        { group: 'sglang_tpot_p50', nameKey: 'metrics.inference.p50' },
        { group: 'sglang_tpot_p95', nameKey: 'metrics.inference.p95' },
      ],
    },
    {
      key: 'e2e',
      titleKey: 'metrics.inference.e2e',
      unit: 's',
      members: [
        { group: 'sglang_e2e_p50', nameKey: 'metrics.inference.p50' },
        { group: 'sglang_e2e_p95', nameKey: 'metrics.inference.p95' },
        { group: 'sglang_e2e_p99', nameKey: 'metrics.inference.p99' },
      ],
    },
    {
      key: 'queue',
      titleKey: 'metrics.inference.queueLatency',
      unit: 's',
      members: [{ group: 'sglang_queue_time' }],
    },
    { key: 'cache_hit', titleKey: 'metrics.inference.cacheHit', unit: '%', members: [{ group: 'sglang_cache_hit' }] },
    {
      key: 'retracted',
      titleKey: 'metrics.inference.retracted',
      unit: '',
      members: [{ group: 'sglang_retracted' }],
    },
    { key: 'aborted', titleKey: 'metrics.inference.aborted', unit: '/s', members: [{ group: 'sglang_aborted' }] },
    {
      key: 'success',
      titleKey: 'metrics.inference.successByStatus',
      unit: '/s',
      members: [{ group: 'sglang_success' }],
    },
  ],
};

/** SGLang 推理指标 profile（单分区） */
export const sglangMetricsProfile: MetricsProfile = {
  id: 'sglang-inference',
  categories: [sglangInferenceCategory],
};

/**
 * 按推理引擎返回指标 profile：vllm → vLLM 分区；sglang → SGLang 分区；
 * 未知 / 不受支持 → null（前端显示提示）。
 */
export function inferenceProfileFor(engine: string | null | undefined): MetricsProfile | null {
  switch ((engine || '').trim().toLowerCase()) {
    case 'vllm':
      return vllmMetricsProfile;
    case 'sglang':
      return sglangMetricsProfile;
    default:
      return null;
  }
}

/** 系统指标 profile（单分区） */
export const systemProfile: MetricsProfile = { id: 'system', categories: [systemCategory] };

/** NVIDIA GPU profile（单分区） */
export const nvidiaGpuProfile: MetricsProfile = { id: 'nvidia-gpu', categories: [nvidiaCategory] };

/** 按节点组合 profile：始终含系统分区；NVIDIA 节点追加 GPU 分区 */
export function nodeMetricsProfile(node: NodeItem | null | undefined): MetricsProfile {
  const categories = [systemCategory];
  if (node?.gpu_vendor === 'nvidia') categories.push(nvidiaCategory);
  return { id: 'node', categories };
}

/** 监控组件标识 → i18n key（未知组件返回空串，由调用方回显原值） */
export function monitorComponentLabelKey(component: string): string {
  switch (component) {
    case 'node-exporter':
      return 'metrics.monitorComponent.nodeExporter';
    case 'nvidia-gpu-exporter':
      return 'metrics.monitorComponent.nvidiaGpuExporter';
    default:
      return '';
  }
}