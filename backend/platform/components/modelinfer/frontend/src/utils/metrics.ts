/**
 * 指标展示工具（纯函数，便于单测）。
 *
 * - 指标值 / 坐标轴按单位格式化（`%` / `B/s` / `B` / `s` / `W` / `Hz` / `°C` / 其它）；
 * - 时间区间 / 自动刷新选项集中定义；
 * - 序列标签 → 可读名称（挂载点 / 设备 / GPU / instance）；
 * - **通用指标板模型**：由 profile（分类 → stat 定义 + 面板定义）+ 后端指标组构建，
 *   供节点详情与后续运行配置详情等页面复用。
 */
import dayjs from 'dayjs';
import { formatBytes, formatSeconds } from './format';
import { translate } from '@/i18n';
import type { MetricGroup, MetricSeries } from '@/api/nodes';

/** i18n 取值函数（默认取全局单例；组件可传入响应式 `t` 以随语言切换重算） */
export type Translate = (key: string) => string;

function defaultTranslate(key: string): string {
  return translate(key);
}

/** 时间语义统一 CST（Asia/Shanghai；dayjs tz 插件由 ./format 初始化） */
const TZ = 'Asia/Shanghai';

/** 预设时间区间 */
export interface RangeOption {
  /** i18n key（`metrics.ranges.<value>`） */
  labelKey: string;
  value: string;
}

/** 自动刷新间隔选项 */
export interface AutoRefreshOption {
  /** i18n key（`metrics.autoRefresh.<seconds>`） */
  labelKey: string;
  seconds: number;
}

export const AUTO_REFRESH_OPTIONS: AutoRefreshOption[] = [
  { labelKey: 'metrics.autoRefresh.30', seconds: 30 },
  { labelKey: 'metrics.autoRefresh.60', seconds: 60 },
  { labelKey: 'metrics.autoRefresh.300', seconds: 300 },
];

/** 自动刷新默认间隔（秒） */
export const DEFAULT_AUTO_REFRESH_SECS = 30;

/** 时间点 → `HH:mm:ss`（CST），用于「更新于」展示；空值 → `-` */
export function formatClock(value?: number | Date | null): string {
  if (value == null) return '-';
  const d = typeof value === 'number' ? dayjs(value * 1000).tz(TZ) : dayjs(value).tz(TZ);
  return d.isValid() ? d.format('HH:mm:ss') : '-';
}

export const RANGE_OPTIONS: RangeOption[] = [
  { labelKey: 'metrics.ranges.15m', value: '15m' },
  { labelKey: 'metrics.ranges.1h', value: '1h' },
  { labelKey: 'metrics.ranges.6h', value: '6h' },
  { labelKey: 'metrics.ranges.24h', value: '24h' },
  { labelKey: 'metrics.ranges.7d', value: '7d' },
];

/**
 * 按单位格式化指标值（MUST NOT 抛错）。
 * - `%` → 1 位小数 + `%`
 * - `B/s` → IEC 字节 + `/s`
 * - `B` → IEC 字节
 * - `s` → 人类可读时长
 * - `W` / `Hz` / `°C` → 数值 + 单位
 * - `tok/s` → token 速率；`/s` → 通用速率
 * - 其它 → 保留至多 2 位小数
 */
export function formatMetricValue(value: number | null | undefined, unit: string): string {
  if (value == null || !Number.isFinite(value)) return '-';
  switch (unit) {
    case '%':
      return `${value.toFixed(1)}%`;
    case 'B/s':
      return `${formatBytes(value)}/s`;
    case 'B':
      return formatBytes(value);
    case 's':
      // 亚秒级时延（TTFT / TPOT 等）以 ms 展示，避免全部显示为 0s
      return value < 1 ? `${Math.round(value * 1000)} ms` : formatSeconds(value);
    case 'W':
      return `${value.toFixed(1)} W`;
    case '°C':
      return `${value.toFixed(0)}°C`;
    case 'Hz':
      return value >= 1e9 ? `${(value / 1e9).toFixed(2)} GHz` : `${(value / 1e6).toFixed(0)} MHz`;
    case 'tok/s':
      return `${value.toFixed(1)} tok/s`;
    case '/s':
      return `${value.toFixed(2)}/s`;
    default:
      return Number.isInteger(value) ? String(value) : value.toFixed(2);
  }
}

/**
 * 坐标轴刻度格式化（量纲友好、紧凑）。
 */
export function formatAxis(value: number | null | undefined, unit: string): string {
  if (value == null || !Number.isFinite(value)) return '';
  if (unit === 'B' || unit === 'B/s') return formatBytes(value);
  if (unit === 's') return value < 1 ? `${Math.round(value * 1000)}ms` : formatSeconds(value);
  if (unit === 'Hz') return value >= 1e9 ? `${(value / 1e9).toFixed(1)}G` : `${(value / 1e6).toFixed(0)}M`;
  if (unit === 'W' || unit === '°C') return `${Math.round(value)}`;
  const abs = Math.abs(value);
  if (abs >= 1e6) return `${(value / 1e6).toFixed(1)}M`;
  if (abs >= 1e3) return `${(value / 1e3).toFixed(1)}k`;
  if (Number.isInteger(value)) return String(value);
  return value.toFixed(2);
}

/** 序列标签 → 可读名称（挂载点 / 设备 / GPU index / instance） */
export function seriesLabel(
  labels: Record<string, string> | undefined,
  fallback: string = defaultTranslate('common.default'),
): string {
  if (!labels) return fallback;
  if (labels.mountpoint) return labels.mountpoint;
  if (labels.device) return labels.device;
  if (labels.index != null && labels.index !== '') return `GPU ${labels.index}`;
  // vLLM 按完成原因分组的成功请求速率
  if (labels.finished_reason) return labels.finished_reason;
  // SGLang 按 HTTP 状态码分组的请求速率
  if (labels.status_code) return `HTTP ${labels.status_code}`;
  // SGLang 多模型服务按 model_name 区分
  if (labels.model_name) return labels.model_name;
  if (labels.instance) return labels.instance;
  return fallback;
}

/** 取时间序列的最后一个有限值（用于瞬时指标）。 */
export function lastValue(points: [number, number][] | undefined): number | null {
  if (!points || points.length === 0) return null;
  for (let i = points.length - 1; i >= 0; i -= 1) {
    const v = points[i][1];
    if (Number.isFinite(v)) return v;
  }
  return null;
}

/** 面板序列模型 */
export interface ChartSeriesModel {
  name: string;
  points: [number, number][];
}

/** 概览 stat 卡模型 */
export interface OverviewStat {
  key: string;
  title: string;
  /** 瞬时值（无数据为 null） */
  value: number | null;
  unit: string;
  /** sparkline 数据 */
  points: [number, number][];
  /** 附注（如磁盘最高占用所在挂载点 / GPU index） */
  note?: string;
}

// ============ 通用指标板（profile 驱动） ============

/** 面板成员：合并某指标组（同单位）到面板，`nameKey` 为序列名覆盖（i18n key） */
export interface PanelMember {
  group: string;
  nameKey?: string;
}

/** 面板定义 */
export interface PanelDef {
  key: string;
  /** 标题 i18n key */
  titleKey: string;
  /** 面板单位（缺省取首个成员组单位） */
  unit?: string;
  members: PanelMember[];
}

/** stat 卡定义 */
export interface StatDef {
  key: string;
  /** 标题 i18n key */
  titleKey: string;
  /** 取值指标组 key */
  group: string;
  unit?: string;
  /**
   * 取值方式：
   * - `last`（缺省）：组内首序列末值；
   * - `maxSeries`：组内各序列末值取最大（如磁盘使用率最高挂载点 / 多卡最高）。
   */
  reduce?: 'last' | 'maxSeries';
}

/** 分类分区定义 */
export interface ProfileCategory {
  key: string;
  /** 标题 i18n key */
  titleKey: string;
  /** 过滤的指标分类（`system` / `gpu`） */
  category: string;
  stats: StatDef[];
  panels: PanelDef[];
}

/** 指标板 profile（数据驱动的版面） */
export interface MetricsProfile {
  id: string;
  categories: ProfileCategory[];
}

/** 构建后的面板（可直接渲染） */
export interface BoardPanel {
  key: string;
  title: string;
  unit: string;
  series: ChartSeriesModel[];
}

/** 构建后的分类分区（可直接渲染） */
export interface BoardCategory {
  key: string;
  title: string;
  stats: OverviewStat[];
  panels: BoardPanel[];
}

/** 组装面板的序列（同单位多成员合并；名字优先用覆盖名，其次标签） */
function collectSeries(
  members: PanelMember[],
  byKey: Map<string, MetricGroup>,
  fallback: string,
  t: Translate,
): ChartSeriesModel[] {
  const out: ChartSeriesModel[] = [];
  for (const mem of members) {
    const g = byKey.get(mem.group);
    const override = mem.nameKey ? t(mem.nameKey) : undefined;
    for (const s of g?.series || []) {
      out.push({ name: seriesName(s, override, fallback), points: s.points });
    }
  }
  return out;
}

/** 序列显示名：覆盖名 +（GPU index 后缀）；无覆盖名时按标签（挂载点 / 设备 / GPU / instance） */
function seriesName(s: MetricSeries, override: string | undefined, fallback: string): string {
  const hasGpu = s.labels?.index != null && s.labels.index !== '';
  if (override) return hasGpu ? `${override} · GPU${s.labels.index}` : override;
  return seriesLabel(s.labels, hasGpu ? `GPU ${s.labels.index}` : fallback);
}

function buildStat(def: StatDef, byKey: Map<string, MetricGroup>, t: Translate): OverviewStat {
  const g = byKey.get(def.group);
  if (def.reduce === 'maxSeries') {
    let best: { value: number; series: MetricSeries } | null = null;
    for (const s of g?.series || []) {
      const v = lastValue(s.points);
      if (v != null && (best == null || v > best.value)) best = { value: v, series: s };
    }
    return {
      key: def.key,
      title: t(def.titleKey),
      value: best?.value ?? null,
      unit: def.unit || g?.unit || '',
      points: best?.series.points || [],
      note: best ? seriesLabel(best.series.labels) : undefined,
    };
  }
  return {
    key: def.key,
    title: t(def.titleKey),
    value: lastValue(g?.series?.[0]?.points),
    unit: def.unit || g?.unit || '',
    points: g?.series?.[0]?.points || [],
  };
}

/**
 * 由指标组 + profile 构建通用指标板。
 *
 * - 分类按 `category` 过滤指标组；
 * - 面板**始终保留**（即使无数据），保证受限设备（如 Orin NX）上版面稳定、空态可见；
 * - stat 卡按 `reduce` 取值。
 */
export function buildBoard(
  groups: MetricGroup[],
  profile: MetricsProfile,
  t: Translate = defaultTranslate,
): BoardCategory[] {
  return profile.categories.map((cat) => {
    const scoped = groups.filter((g) => g.category === cat.category);
    const byKey = new Map(scoped.map((g) => [g.key, g]));
    return {
      key: cat.key,
      title: t(cat.titleKey),
      stats: cat.stats.map((s) => buildStat(s, byKey, t)),
      panels: cat.panels.map((p) => {
        const title = t(p.titleKey);
        return {
          key: p.key,
          title,
          unit: p.unit || byKey.get(p.members[0]?.group || '')?.unit || '',
          series: collectSeries(p.members, byKey, title, t),
        };
      }),
    };
  });
}

/**
 * 生成 sparkline 的 SVG polyline 坐标（viewBox 100×28，y 轴反转）。
 * 少于 2 个点或全部相同 → 空串（调用方不渲染）。
 */
export function sparklinePoints(points: [number, number][], width = 100, height = 28): string {
  const valid = points.filter((p) => Number.isFinite(p[1]));
  if (valid.length < 2) return '';
  const values = valid.map((p) => p[1]);
  const min = Math.min(...values);
  const max = Math.max(...values);
  const span = max - min || 1;
  return valid
    .map((p, i) => {
      const x = (i / (valid.length - 1)) * width;
      const y = height - ((p[1] - min) / span) * height;
      return `${x.toFixed(1)},${y.toFixed(1)}`;
    })
    .join(' ');
}