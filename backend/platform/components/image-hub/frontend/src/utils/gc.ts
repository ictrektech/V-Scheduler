/**
 * registry 垃圾回收（GC）展示纯函数（与后端 `gc.rs` / `handlers/gc.rs` 语义对应）。
 *
 * 本模块保持零依赖（便于 `node --test` 直接测试），因此返回 i18n key / 结构化值，
 * 由组件层 `t()` 组合本地化文案。
 */

/** GC 任务结果摘要（与后端 `GcSummary` 一致；非 gc 任务为 null/undefined） */
export interface GcSummary {
  dry_run: boolean;
  delete_untagged: boolean;
  blobs_marked: number;
  blobs_deleted: number;
  manifests_deleted: number;
  storage_bytes_before: number | null;
  storage_bytes_after: number | null;
  freed_bytes: number | null;
}

/** GC 状态 / 配置（与后端 `GcStatusData` 一致） */
export interface GcStatus {
  enabled: boolean;
  interval_hours: number;
  delete_untagged: boolean;
  last_run_at: string | null;
  next_run_at: string | null;
  available: boolean;
  unavailable_reason: string | null;
  /** 不可用原因稳定码（registry_binary_missing / storage_dir_missing；可选） */
  unavailable_code?: string | null;
  storage_dir: string | null;
  storage_bytes: number | null;
  running_task_id: string | null;
}

/** 自动回收间隔默认值（小时）—— 与后端 `DEFAULT_GC_INTERVAL_HOURS` 一致 */
export const GC_DEFAULT_INTERVAL_HOURS = 24;
/** 自动回收间隔下限 / 上限（小时）—— 与后端 `MIN/MAX_GC_INTERVAL_HOURS` 一致 */
export const GC_MIN_INTERVAL_HOURS = 1;
export const GC_MAX_INTERVAL_HOURS = 720;

/** 常用间隔预设（`value` 小时；`labelKey` 由组件层翻译） */
export const GC_INTERVAL_PRESETS: { value: number; labelKey: string }[] = [
  { value: 1, labelKey: 'gc.intervals.h1' },
  { value: 3, labelKey: 'gc.intervals.h3' },
  { value: 6, labelKey: 'gc.intervals.h6' },
  { value: 12, labelKey: 'gc.intervals.h12' },
  { value: 24, labelKey: 'gc.intervals.h24' },
  { value: 72, labelKey: 'gc.intervals.d3' },
  { value: 168, labelKey: 'gc.intervals.d7' },
];

/** 间隔是否合法（对应后端 1~720 校验） */
export function isValidGcInterval(hours: number | null | undefined): boolean {
  return (
    typeof hours === 'number' &&
    Number.isInteger(hours) &&
    hours >= GC_MIN_INTERVAL_HOURS &&
    hours <= GC_MAX_INTERVAL_HOURS
  );
}

/** 间隔展示结构：整天的整数倍 → 天，否则小时 */
export function gcIntervalParts(hours: number | null | undefined): {
  unit: 'days' | 'hours';
  value: number;
} {
  const value = isValidGcInterval(hours) ? (hours as number) : GC_DEFAULT_INTERVAL_HOURS;
  if (value >= 24 && value % 24 === 0) return { unit: 'days', value: value / 24 };
  return { unit: 'hours', value };
}

/** 任务类型展示 i18n key（`tasks.kind.*`；未知 kind 回落「任务」） */
export function kindKey(kind: string): string {
  switch (kind) {
    case 'pull':
      return 'tasks.kind.pull';
    case 'sync':
      return 'tasks.kind.sync';
    case 'clone':
      return 'tasks.kind.clone';
    case 'rename':
      return 'tasks.kind.rename';
    case 'import':
      return 'tasks.kind.import';
    case 'gc':
      return 'tasks.kind.gc';
    default:
      return 'tasks.kind.task';
  }
}

/** GC 结果计数（供 `gc.result.text` / `gc.result.textDry` 插值；非 gc 返回 null） */
export function gcResultCounts(summary: GcSummary | null | undefined): {
  blobs: number;
  manifests: number;
  dryRun: boolean;
} | null {
  if (!summary) return null;
  return {
    blobs: summary.blobs_deleted,
    manifests: summary.manifests_deleted,
    dryRun: summary.dry_run,
  };
}

/** GC 结果标题 i18n key：预演明确标注未删除数据 */
export function gcHeadlineKey(summary: GcSummary | null | undefined): string {
  if (!summary) return '';
  return summary.dry_run ? 'gc.result.dryHeadline' : 'gc.result.doneHeadline';
}

/** 仍被引用的 blob 数量（供 `gc.result.marked` 插值；非 gc 返回 null） */
export function gcMarkedCount(summary: GcSummary | null | undefined): number | null {
  if (!summary) return null;
  return summary.blobs_marked;
}

/** 下次自动回收文案 i18n key（未启用 / 无基准 / 有具体时间） */
export function nextRunKey(nextRunAt: string | null | undefined, enabled: boolean): string {
  if (!enabled) return 'gc.nextRunText.disabled';
  return nextRunAt ? 'gc.nextRunText.at' : 'gc.nextRunText.pending';
}

/** 是否展示 GC 定量进度（GC 无字节级进度，恒为不定量） */
export function isGcTask(kind: string): boolean {
  return kind === 'gc';
}