import { describe, expect, it } from 'vitest';
import {
  AUTO_REFRESH_OPTIONS,
  DEFAULT_AUTO_REFRESH_SECS,
  RANGE_OPTIONS,
  buildBoard,
  formatAxis,
  formatClock,
  formatMetricValue,
  lastValue,
  seriesLabel,
  sparklinePoints,
} from '../metrics';
import {
  inferenceProfileFor,
  nvidiaGpuProfile,
  nodeMetricsProfile,
  sglangMetricsProfile,
  systemProfile,
  vllmMetricsProfile,
} from '../metricProfiles';
import type { MetricGroup, NodeItem } from '@/api/nodes';

function group(
  key: string,
  name: string,
  unit: string,
  series: { labels: Record<string, string>; points: [number, number][] }[],
  category: string = 'system',
): MetricGroup {
  return { key, name, unit, source: category === 'gpu' ? 'modelinfer-nvidia-gpu-exporter' : 'modelinfer-node-exporter', category, series };
}

const SYS: MetricGroup[] = [
  group('cpu_usage', 'CPU 使用率', '%', [{ labels: {}, points: [[1, 20], [2, 30]] }]),
  group('mem_usage', '内存使用率', '%', [{ labels: {}, points: [[1, 40], [2, 50]] }]),
  group('load1', '系统负载（1m）', '', [{ labels: {}, points: [[1, 1], [2, 2]] }]),
  group('load5', '系统负载（5m）', '', [{ labels: {}, points: [[1, 1], [2, 1.5]] }]),
  group('load15', '系统负载（15m）', '', [{ labels: {}, points: [[1, 1], [2, 1.2]] }]),
  group('disk_usage', '磁盘使用率', '%', [
    { labels: { mountpoint: '/', device: 'sda' }, points: [[1, 10], [2, 12]] },
    { labels: { mountpoint: '/data' }, points: [[1, 60], [2, 75]] },
  ]),
  group('net_rx', '网络接收速率', 'B/s', [{ labels: {}, points: [[1, 100], [2, 200]] }]),
  group('net_tx', '网络发送速率', 'B/s', [{ labels: {}, points: [[1, 50], [2, 80]] }]),
  group('disk_read', '磁盘读取速率', 'B/s', [{ labels: {}, points: [[1, 0], [2, 10]] }]),
  group('disk_write', '磁盘写入速率', 'B/s', [{ labels: {}, points: [[1, 0], [2, 20]] }]),
  group('fs_avail', '文件系统可用空间', 'B', [
    { labels: { mountpoint: '/' }, points: [[1, 1000], [2, 900]] },
    { labels: { mountpoint: '/data' }, points: [[1, 2000], [2, 1800]] },
  ]),
  group('uptime', '运行时长', 's', [{ labels: {}, points: [[1, 100], [2, 200]] }]),
];

const GPU: MetricGroup[] = [
  group('gpu_util', 'GPU 使用率', '%', [{ labels: { uuid: 'u1', index: '0' }, points: [[1, 10], [2, 40]] }], 'gpu'),
  group('gpu_temp', 'GPU 温度', '°C', [{ labels: { uuid: 'u1', index: '0' }, points: [[1, 40], [2, 42]] }], 'gpu'),
  group('gpu_temp_mem', '显存温度', '°C', [{ labels: { uuid: 'u1', index: '0' }, points: [[1, 50], [2, 52]] }], 'gpu'),
  group('gpu_proc_mem', 'GPU 显存（进程占用）', 'B', [{ labels: { uuid: 'u1', index: '0' }, points: [[1, 100], [2, 200]] }], 'gpu'),
];

describe('指标展示工具', () => {
  it('按单位格式化', () => {
    expect(formatMetricValue(12.345, '%')).toBe('12.3%');
    expect(formatMetricValue(2048, 'B/s')).toBe('2.0 KiB/s');
    expect(formatMetricValue(1536, 'B')).toBe('1.5 KiB');
    expect(formatMetricValue(90, 's')).toBe('1m30s');
    expect(formatMetricValue(0.055, 's')).toBe('55 ms');
    expect(formatMetricValue(1.5, 's')).toBe('1s');
    expect(formatMetricValue(11.16, 'W')).toBe('11.2 W');
    expect(formatMetricValue(42.6, '°C')).toBe('43°C');
    expect(formatMetricValue(2.418e9, 'Hz')).toBe('2.42 GHz');
    expect(formatMetricValue(123.4, 'tok/s')).toBe('123.4 tok/s');
    expect(formatMetricValue(0.5, '/s')).toBe('0.50/s');
    expect(formatMetricValue(3, '')).toBe('3');
    expect(formatMetricValue(3.1415, '')).toBe('3.14');
    expect(formatMetricValue(null, '%')).toBe('-');
    expect(formatMetricValue(Number.NaN, '%')).toBe('-');
  });

  it('坐标轴量纲友好格式化', () => {
    expect(formatAxis(1536, 'B')).toBe('1.5 KiB');
    expect(formatAxis(90, 's')).toBe('1m30s');
    expect(formatAxis(1500, '')).toBe('1.5k');
    expect(formatAxis(2_000_000, '')).toBe('2.0M');
    expect(formatAxis(42, '%')).toBe('42');
    expect(formatAxis(2.418e9, 'Hz')).toBe('2.4G');
    expect(formatAxis(3.5, '')).toBe('3.50');
    expect(formatAxis(Number.NaN, '%')).toBe('');
  });

  it('序列标签优先级（含 GPU index）', () => {
    expect(seriesLabel({ mountpoint: '/data', device: 'sda' })).toBe('/data');
    expect(seriesLabel({ device: 'sda' })).toBe('sda');
    expect(seriesLabel({ uuid: 'u1', index: '1' })).toBe('GPU 1');
    expect(seriesLabel({ instance: '10.0.0.1:9100' })).toBe('10.0.0.1:9100');
    expect(seriesLabel({ finished_reason: 'stop' })).toBe('stop');
    // SGLang：按 HTTP 状态码 / 模型名区分
    expect(seriesLabel({ status_code: '200' })).toBe('HTTP 200');
    expect(seriesLabel({ model_name: 'deepseek-v4.1-flash' })).toBe('deepseek-v4.1-flash');
    expect(seriesLabel(undefined, '默认')).toBe('默认');
  });

  it('取最后一个有效值', () => {
    expect(lastValue([[1, 5], [2, 7]])).toBe(7);
    expect(lastValue([[1, Number.NaN], [2, 4]])).toBe(4);
    expect(lastValue([])).toBeNull();
    expect(lastValue(undefined)).toBeNull();
  });

  it('区间选项含预设', () => {
    expect(RANGE_OPTIONS.map((o) => o.value)).toEqual(['15m', '1h', '6h', '24h', '7d']);
  });

  it('通用指标板：系统分区合并 / 顺序 / 面板始终保留', () => {
    const [sys] = buildBoard(SYS, systemProfile);
    expect(sys.panels.map((p) => p.key)).toEqual(['cpu', 'load', 'mem', 'disk', 'net', 'io', 'fs']);
    expect(sys.panels.find((p) => p.key === 'load')!.series.map((s) => s.name)).toEqual(['负载 1m', '负载 5m', '负载 15m']);
    expect(sys.panels.find((p) => p.key === 'net')!.unit).toBe('B/s');
    expect(sys.panels.find((p) => p.key === 'io')!.series.map((s) => s.name)).toEqual(['读取', '写入']);
    expect(sys.panels.find((p) => p.key === 'disk')!.series.map((s) => s.name)).toEqual(['/', '/data']);

    // 概览 stat：磁盘取最高挂载点
    const stats = sys.stats;
    expect(stats.map((s) => s.key)).toEqual(['cpu', 'mem', 'load', 'disk', 'uptime']);
    expect(stats.find((s) => s.key === 'disk')!.value).toBe(75);
    expect(stats.find((s) => s.key === 'disk')!.note).toBe('/data');
    expect(stats.find((s) => s.key === 'uptime')!.value).toBe(200);
    expect(stats.find((s) => s.key === 'cpu')!.value).toBe(30);

    // 缺数据时面板仍保留（空态）
    const [empty] = buildBoard([], systemProfile);
    expect(empty.panels.map((p) => p.key)).toEqual(['cpu', 'load', 'mem', 'disk', 'net', 'io', 'fs']);
    expect(empty.panels.every((p) => p.series.length === 0)).toBe(true);
    expect(empty.stats.every((s) => s.value === null)).toBe(true);
  });

  it('通用指标板：GPU 分区按 category 过滤 + GPU 命名', () => {
    const [gpuCat] = buildBoard(GPU, nvidiaGpuProfile);
    expect(gpuCat.key).toBe('nvidia');
    // 系统指标组不会混入 GPU 分区
    const merged = buildBoard([...SYS, ...GPU], nvidiaGpuProfile);
    expect(merged[0].panels.find((p) => p.key === 'gpu_util')!.series[0].name).toBe('GPU 0');
    // 多成员温度面板：覆盖名 + GPU index 后缀
    const temp = merged[0].panels.find((p) => p.key === 'gpu_temp')!;
    expect(temp.unit).toBe('°C');
    expect(temp.series.map((s) => s.name)).toEqual(['GPU · GPU0', '显存 · GPU0']);
  });

  it('nodeMetricsProfile：非 NVIDIA 仅系统分区', () => {
    const nvidiaNode = { gpu_vendor: 'nvidia' } as NodeItem;
    const cpuNode = { gpu_vendor: '' } as NodeItem;
    expect(nodeMetricsProfile(nvidiaNode).categories.map((c) => c.key)).toEqual(['system', 'nvidia']);
    expect(nodeMetricsProfile(cpuNode).categories.map((c) => c.key)).toEqual(['system']);
    expect(nodeMetricsProfile(null).categories.map((c) => c.key)).toEqual(['system']);
  });

  it('vLLM 推理 profile：分区 / 面板 / 缺数据空态', () => {
    const INF: MetricGroup[] = [
      group('vllm_running', '运行中请求', '', [{ labels: {}, points: [[1, 1], [2, 3]] }], 'inference'),
      group('vllm_waiting', '排队请求', '', [{ labels: {}, points: [[1, 0], [2, 2]] }], 'inference'),
      group('vllm_kv_cache', 'KV-cache 使用率', '%', [{ labels: {}, points: [[1, 10], [2, 30]] }], 'inference'),
      group('vllm_gen_throughput', '输出吞吐', 'tok/s', [{ labels: {}, points: [[1, 100], [2, 200]] }], 'inference'),
      group('vllm_success', '成功请求速率', '/s', [
        { labels: { finished_reason: 'stop' }, points: [[1, 1], [2, 2]] },
        { labels: { finished_reason: 'length' }, points: [[1, 0], [2, 1]] },
      ], 'inference'),
    ];
    const [cat] = buildBoard(INF, vllmMetricsProfile);
    expect(cat.key).toBe('inference');
    expect(cat.stats.map((s) => s.key)).toEqual(['running', 'waiting', 'kv_cache', 'gen_throughput']);
    expect(cat.stats.find((s) => s.key === 'running')!.value).toBe(3);
    expect(cat.stats.find((s) => s.key === 'kv_cache')!.value).toBe(30);
    expect(cat.panels.map((p) => p.key)).toEqual([
      'throughput',
      'requests',
      'kv_cache',
      'ttft',
      'tpot',
      'e2e',
      'phase',
      'prefix_hit',
      'preemptions',
      'success',
    ]);
    expect(cat.panels.find((p) => p.key === 'throughput')!.unit).toBe('tok/s');
    // 成功速率按 finished_reason 命名
    expect(cat.panels.find((p) => p.key === 'success')!.series.map((s) => s.name)).toEqual([
      'stop',
      'length',
    ]);
    // 缺数据时面板仍保留（空态）
    const [empty] = buildBoard([], vllmMetricsProfile);
    expect(empty.panels.length).toBe(10);
    expect(empty.panels.every((p) => p.series.length === 0)).toBe(true);
    // 系统 / GPU 指标不混入推理分区
    const merged = buildBoard([...SYS, ...INF], vllmMetricsProfile);
    expect(merged[0].panels.find((p) => p.key === 'throughput')!.series.length).toBe(1);
  });

  it('SGLang 推理 profile：分区 / 面板 / 状态码序列名', () => {
    const INF: MetricGroup[] = [
      group('sglang_running', '运行中请求', '', [{ labels: {}, points: [[1, 1], [2, 4]] }], 'inference'),
      group('sglang_waiting', '排队请求', '', [{ labels: {}, points: [[1, 0], [2, 2]] }], 'inference'),
      group('sglang_token_usage', 'Token 使用率', '%', [{ labels: {}, points: [[1, 10], [2, 55]] }], 'inference'),
      group('sglang_gen_throughput', '输出吞吐', 'tok/s', [{ labels: {}, points: [[1, 20], [2, 80]] }], 'inference'),
      group('sglang_ttft_p95', '首 token 时延 P95', 's', [{ labels: {}, points: [[1, 0.2], [2, 0.4]] }], 'inference'),
      group('sglang_success', '请求速率', '/s', [
        { labels: { status_code: '200' }, points: [[1, 1], [2, 2]] },
        { labels: { status_code: '500' }, points: [[1, 0], [2, 0.1]] },
      ], 'inference'),
    ];
    const [cat] = buildBoard(INF, sglangMetricsProfile);
    expect(cat.key).toBe('inference');
    expect(cat.stats.map((s) => s.key)).toEqual(['running', 'waiting', 'token_usage', 'gen_throughput']);
    expect(cat.stats.find((s) => s.key === 'running')!.value).toBe(4);
    expect(cat.stats.find((s) => s.key === 'token_usage')!.value).toBe(55);
    expect(cat.panels.map((p) => p.key)).toEqual([
      'throughput',
      'requests',
      'token_usage',
      'ttft',
      'tpot',
      'e2e',
      'queue',
      'cache_hit',
      'retracted',
      'aborted',
      'success',
    ]);
    expect(cat.panels.find((p) => p.key === 'throughput')!.unit).toBe('tok/s');
    expect(cat.panels.find((p) => p.key === 'success')!.series.map((s) => s.name)).toEqual([
      'HTTP 200',
      'HTTP 500',
    ]);
    // 缺数据时面板仍保留（空态）
    const [empty] = buildBoard([], sglangMetricsProfile);
    expect(empty.panels.length).toBe(11);
    expect(empty.panels.every((p) => p.series.length === 0)).toBe(true);
  });

  it('inferenceProfileFor：vllm / sglang 受支持', () => {
    expect(inferenceProfileFor('vllm')?.id).toBe('vllm-inference');
    expect(inferenceProfileFor('VLLM')?.id).toBe('vllm-inference');
    expect(inferenceProfileFor('sglang')?.id).toBe('sglang-inference');
    expect(inferenceProfileFor('SGLang')?.id).toBe('sglang-inference');
    expect(inferenceProfileFor('comfyui')).toBeNull();
    expect(inferenceProfileFor('unknown')).toBeNull();
    expect(inferenceProfileFor('')).toBeNull();
    expect(inferenceProfileFor(null)).toBeNull();
    expect(inferenceProfileFor(undefined)).toBeNull();
  });

  it('自动刷新选项与时钟格式', () => {
    expect(AUTO_REFRESH_OPTIONS.map((o) => o.seconds)).toEqual([30, 60, 300]);
    expect(DEFAULT_AUTO_REFRESH_SECS).toBe(30);
    // 2026-09-30T17:47:00+08:00 → 17:47:00（CST）
    expect(formatClock(1790761620)).toBe('17:47:00');
    expect(formatClock(null)).toBe('-');
    expect(formatClock(new Date('2026-09-30T09:00:00Z'))).toBe('17:00:00');
  });

  it('sparkline 坐标归一化', () => {
    expect(sparklinePoints([[1, 1]])).toBe('');
    const pts = sparklinePoints([[1, 0], [2, 10]]);
    const parts = pts.split(' ');
    expect(parts.length).toBe(2);
    expect(parts[0]).toBe('0.0,28.0');
    expect(parts[1]).toBe('100.0,0.0');
    // 全等值不除零
    expect(sparklinePoints([[1, 5], [2, 5]]) !== '').toBe(true);
  });
});