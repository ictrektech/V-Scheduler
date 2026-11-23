/**
 * 任务状态 / 进度展示与超时选项纯函数（与后端 `progress.rs` 的阶段语义对应）。
 *
 * 本模块保持零依赖（便于 `node --test` 直接测试），因此**不返回已翻译文案**：
 * - 状态 / 阶段返回 i18n key，由组件层 `t()` 渲染；
 * - 计数 / 重试 / 超时返回结构化值，由组件层组合本地化文案。
 */

/** 任务状态（与后端 `JobStatus` 一致） */
export type TaskStatus = 'pending' | 'running' | 'succeeded' | 'failed' | 'cancelled';

/** 结构化进度（与后端 `ProgressDetail` 一致） */
export interface TaskProgressDetail {
  phase: string;
  message: string;
  percent: number | null;
  items_done: number;
  items_total: number;
  sub_current: number | null;
  sub_total: number | null;
  layers_done: number;
  /** 当前尝试序号（1 起；仅重试场景出现） */
  attempt: number;
  /** 最大尝试次数（= 1 + 重试次数） */
  max_attempts: number;
  /** 重试退避剩余秒数（非退避阶段为 null；可选，兼容旧后端） */
  retry_in_secs?: number | null;
}

/** 单次传输超时默认值（分钟）——与后端 `DEFAULT_TRANSFER_TIMEOUT_MINUTES` 一致 */
export const DEFAULT_TIMEOUT_MINUTES = 60;
/** 超时下限（分钟） */
export const MIN_TIMEOUT_MINUTES = 1;
/** 超时「无限制」取值（分钟，`0`）——与后端 `UNLIMITED_TRANSFER_TIMEOUT_MINUTES` 一致 */
export const UNLIMITED_TIMEOUT_MINUTES = 0;

/** 自定义超时输入的可选单位（分钟 / 小时；labelKey 由组件层翻译） */
export const TIMEOUT_UNITS: { value: number; labelKey: string }[] = [
  { value: 1, labelKey: 'timeout.unitMinute' },
  { value: 60, labelKey: 'timeout.unitHour' },
];

/** 常用超时预设（`value` 为分钟；`labelKey` 由组件层翻译，`0` 表示无限制） */
export const TIMEOUT_PRESETS: { value: number; labelKey: string }[] = [
  { value: 30, labelKey: 'timeout.presets.m30' },
  { value: 60, labelKey: 'timeout.presets.h1' },
  { value: 120, labelKey: 'timeout.presets.h2' },
  { value: 720, labelKey: 'timeout.presets.h12' },
  { value: 1440, labelKey: 'timeout.presets.h24' },
  { value: UNLIMITED_TIMEOUT_MINUTES, labelKey: 'timeout.presets.unlimited' },
];

/** 传输失败重试次数默认值——与后端 `DEFAULT_RETRY_TIMES` 一致 */
export const DEFAULT_RETRY_TIMES = 2;
/** 重试次数下限 */
export const MIN_RETRY_TIMES = 1;
/** 重试次数上限 */
export const MAX_RETRY_TIMES = 5;

/** 重试次数可选值（1~5；labelKey 由组件层翻译并传入 count） */
export const RETRY_PRESETS: { value: number; labelKey: string }[] = [
  { value: 1, labelKey: 'retry.count' },
  { value: 2, labelKey: 'retry.countDefault' },
  { value: 3, labelKey: 'retry.count' },
  { value: 4, labelKey: 'retry.count' },
  { value: 5, labelKey: 'retry.count' },
];

/** 重试退避起始 / 上限（秒）——与后端 `RETRY_BACKOFF_BASE_SECS` / `RETRY_BACKOFF_MAX_SECS` 一致 */
export const RETRY_BACKOFF_BASE_SECS = 5;
export const RETRY_BACKOFF_MAX_SECS = 60;

/**
 * 去掉本地 registry 主机段，得到上游引用（纯函数）。
 *
 * 任务记录的 `dest` / 克隆源形如 `<registry-host>/<upstream-repo>:<tag>`，
 * 去掉首段主机后即 `<upstream-registry>/<repo>:<tag>`（用于本地化任务标题）。
 */
export function stripRegistryHost(value: string): string {
  const raw = (value ?? '').trim();
  const idx = raw.indexOf('/');
  return idx < 0 ? raw : raw.slice(idx + 1);
}

/** 状态展示 i18n key（`tasks.status.*`） */
export function statusKey(status: TaskStatus): string {
  switch (status) {
    case 'running':
      return 'tasks.status.running';
    case 'succeeded':
      return 'tasks.status.succeeded';
    case 'failed':
      return 'tasks.status.failed';
    case 'cancelled':
      return 'tasks.status.cancelled';
    default:
      return 'tasks.status.pending';
  }
}

/** 状态标签颜色（Ant Design Vue Tag 的 color） */
export function statusColor(status: TaskStatus): string {
  switch (status) {
    case 'running':
      return 'processing';
    case 'succeeded':
      return 'success';
    case 'failed':
      return 'error';
    case 'cancelled':
      return 'default';
    default:
      return 'default';
  }
}

/** 阶段标签 i18n key（`tasks.phase.*`；未知阶段回退「进行中」） */
export function phaseKey(phase: string): string {
  switch (phase) {
    case 'queued':
      return 'tasks.phase.queued';
    case 'login':
      return 'tasks.phase.login';
    case 'preparing':
      return 'tasks.phase.preparing';
    case 'copying':
      return 'tasks.phase.copying';
    case 'writing':
      return 'tasks.phase.writing';
    case 'gc':
      return 'tasks.phase.gc';
    case 'done':
      return 'tasks.phase.done';
    case 'cancelled':
      return 'tasks.phase.cancelled';
    case 'failed':
      return 'tasks.phase.failed';
    default:
      return 'tasks.phase.running';
  }
}

/** 结构化进度计数（组件层据此组合本地化摘要） */
export interface ProgressParts {
  /** 第 N/M 次尝试（`max_attempts > 1` 时） */
  attempt: { current: number; max: number } | null;
  /** 重试退避剩余秒数（处于退避等待时） */
  retryInSecs: number | null;
  /** 标签计数（`items_total > 1` 时） */
  items: { done: number; total: number } | null;
  /** 子镜像计数（`sub_total > 1` 时） */
  sub: { current: number; total: number } | null;
  /** 已复制层数（> 0 时） */
  layers: number;
}

/** 从结构化进度提取计数明细（无可用计数时各字段为 null / 0） */
export function progressParts(
  detail: TaskProgressDetail | null | undefined,
): ProgressParts {
  if (!detail) {
    return { attempt: null, retryInSecs: null, items: null, sub: null, layers: 0 };
  }
  const attempt =
    detail.max_attempts > 1 && detail.attempt > 0
      ? { current: detail.attempt, max: detail.max_attempts }
      : null;
  const retryInSecs =
    typeof detail.retry_in_secs === 'number' && detail.retry_in_secs > 0
      ? detail.retry_in_secs
      : null;
  const items =
    detail.items_total > 1
      ? { done: detail.items_done, total: detail.items_total }
      : null;
  const sub =
    detail.sub_total != null && detail.sub_total > 1
      ? { current: detail.sub_current ?? 0, total: detail.sub_total }
      : null;
  return { attempt, retryInSecs, items, sub, layers: detail.layers_done };
}

/** 是否已请求停止（UI 显示「正在停止…」并禁用停止按钮） */
export function isStopping(status: TaskStatus, cancelRequested: boolean): boolean {
  return status === 'running' && cancelRequested;
}

/**
 * 超时分钟数是否合法：`0`（无限制）或不小于 1 的整数；**无上限**（对应后端校验）
 */
export function isValidTimeoutMinutes(value: number | null | undefined): boolean {
  return (
    typeof value === 'number' &&
    Number.isInteger(value) &&
    (value === UNLIMITED_TIMEOUT_MINUTES || value >= MIN_TIMEOUT_MINUTES)
  );
}

/** 重试次数是否合法（对应后端 1~5 校验） */
export function isValidRetryTimes(value: number | null | undefined): boolean {
  return (
    typeof value === 'number' &&
    Number.isInteger(value) &&
    value >= MIN_RETRY_TIMES &&
    value <= MAX_RETRY_TIMES
  );
}

/** 第 N 次失败后的退避秒数（与后端 `retry_backoff` 一致：5/10/20/40/60 封顶） */
export function retryBackoffSecs(failureIndex: number): number {
  const index = Math.max(1, Math.floor(failureIndex));
  const factor = 2 ** Math.min(index - 1, 16);
  return Math.min(RETRY_BACKOFF_BASE_SECS * factor, RETRY_BACKOFF_MAX_SECS);
}

/** 重试说明的结构化参数（供 `retry.hint` 插值） */
export function retryHintParts(times: number | null | undefined): {
  attempts: number;
  backoffs: string;
} {
  const value = isValidRetryTimes(times) ? (times as number) : DEFAULT_RETRY_TIMES;
  const backoffs = Array.from({ length: value }, (_, i) => `${retryBackoffSecs(i + 1)}s`).join(' / ');
  return { attempts: 1 + value, backoffs };
}

/** 超时展示的结构化值（`unlimited` / 整小时 / 分钟） */
export type TimeoutParts =
  | { kind: 'unlimited' }
  | { kind: 'hours'; value: number }
  | { kind: 'minutes'; value: number };

/** 超时分钟数 → 展示结构（`0` → 无限制；整小时 → 小时；其余 → 分钟） */
export function timeoutParts(minutes: number | null | undefined): TimeoutParts {
  const value = isValidTimeoutMinutes(minutes) ? (minutes as number) : DEFAULT_TIMEOUT_MINUTES;
  if (value === UNLIMITED_TIMEOUT_MINUTES) return { kind: 'unlimited' };
  if (value >= 60 && value % 60 === 0) return { kind: 'hours', value: value / 60 };
  return { kind: 'minutes', value };
}

/** 超时时长（分钟）→ 「数值 + 单位」拆解（自定义输入回显用） */
export function splitTimeout(minutes: number): { value: number; unit: number } {
  if (minutes >= 60 && minutes % 60 === 0) {
    return { value: minutes / 60, unit: 60 };
  }
  return { value: minutes, unit: 1 };
}