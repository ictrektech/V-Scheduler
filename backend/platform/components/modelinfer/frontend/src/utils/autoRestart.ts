/**
 * 探活失败自动重启：纯函数（分钟 ↔ 秒换算、health-probe 声明判定、相位映射）
 *
 * 后端契约（见 `specs/docs/design/modelinfer/auto-restart.md`）：
 * - 配置字段 `auto_restart_enabled`（开关）+ `auto_restart_fail_secs`（连续失败阈值，秒）；
 * - 阈值合法区间 `30..=86400` 秒；UI 以「分钟」输入，默认 5 分钟（= 300 秒）；
 * - 卡片未声明 `health-probe` 时自动重启不会生效（平台不伪造存活检查）；
 * - 运行态相位 `""`（未在循环）/ `stopping`（停止清理中，随后启动）/ `starting`（启动中）。
 *
 * 本模块只做展示与输入归一，不承担后端校验职责（后端仍会返回 `20748`）。
 */

import { parse as parseYaml } from 'yaml';

/** 阈值下限（秒） */
export const MIN_FAIL_SECS = 30;
/** 阈值上限（秒，24 小时） */
export const MAX_FAIL_SECS = 86400;
/** 阈值默认值（秒，5 分钟） */
export const DEFAULT_FAIL_SECS = 300;
/** UI 分钟下限（1 分钟 = 60 秒 ≥ 后端下限 30 秒） */
export const MIN_FAIL_MINUTES = 1;
/** UI 分钟上限（1440 分钟 = 24 小时） */
export const MAX_FAIL_MINUTES = 1440;
/** UI 默认分钟数（5 分钟） */
export const DEFAULT_FAIL_MINUTES = 5;

/** 自动重启相位常量（与后端一致） */
export const PHASE_IDLE = '';
export const PHASE_STOPPING = 'stopping';
export const PHASE_STARTING = 'starting';

/** 秒 → 分钟（向上取整到整数分钟，非法值回落默认） */
export function secsToMinutes(secs: unknown): number {
  const n = typeof secs === 'number' && Number.isFinite(secs) ? secs : NaN;
  if (!Number.isFinite(n) || n < MIN_FAIL_SECS) return DEFAULT_FAIL_MINUTES;
  return clampMinutes(Math.ceil(n / 60));
}

/** 分钟 → 秒（先按 UI 区间归一，再换算） */
export function minutesToSecs(minutes: unknown): number {
  return clampMinutes(Number(minutes)) * 60;
}

/** 归一化 UI 分钟数（`1..=1440`；非法 / 空 → 默认 5） */
export function clampMinutes(value: unknown): number {
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(n)) return DEFAULT_FAIL_MINUTES;
  const rounded = Math.round(n);
  if (rounded < MIN_FAIL_MINUTES) return MIN_FAIL_MINUTES;
  if (rounded > MAX_FAIL_MINUTES) return MAX_FAIL_MINUTES;
  return rounded;
}

/** 运行态是否处于自动重启循环中（相位非 idle） */
export function inRestartLoop(phase: unknown): boolean {
  return phase === PHASE_STOPPING || phase === PHASE_STARTING;
}

/**
 * 自动重启阶段的 i18n key（用于详情页标签文案）
 *
 * @param phase 后端 `auto_restart_phase`
 */
export function phaseLabelKey(phase: unknown): string {
  switch (phase) {
    case PHASE_STOPPING:
      return 'runDetail.overview.autoRestartPhaseStopping';
    case PHASE_STARTING:
      return 'runDetail.overview.autoRestartPhaseStarting';
    default:
      return 'runDetail.overview.autoRestartPhaseIdle';
  }
}

/**
 * 卡片 manifest 是否声明了 `lifecycle.health-probe`
 *
 * 接受 manifest 对象或 YAML / JSON 原文（`manifest_json` 字段），解析失败返回 `false`
 * （即视为未声明）。
 */
export function hasHealthProbe(manifest: unknown): boolean {
  const obj = toObject(manifest) ?? parseManifestText(manifest);
  if (!obj) return false;
  const lifecycle = toObject(obj.lifecycle);
  if (!lifecycle) return false;
  return typeof lifecycle['health-probe'] === 'string' && lifecycle['health-probe'].trim() !== '';
}

/**
 * 判断一组卡片版本是否都没有声明 health-probe（用于向导提示）
 *
 * 入参为已登记卡片版本（含 `manifest_json`）；无版本（正在加载）时返回 `false`，
 * 避免加载期间闪烁警告。
 */
export function allVersionsLackHealthProbe(
  versions: Array<{ manifest_json?: unknown }> | null | undefined,
): boolean {
  if (!versions || versions.length === 0) return false;
  return versions.every((v) => !hasHealthProbe(v.manifest_json));
}

function parseManifestText(text: unknown): Record<string, unknown> | null {
  if (typeof text !== 'string' || !text.trim()) return null;
  try {
    return toObject(parseYaml(text));
  } catch {
    return null;
  }
}

function toObject(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null;
  return value as Record<string, unknown>;
}