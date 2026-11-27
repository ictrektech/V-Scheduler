/**
 * 自动重启：纯函数（策略判定、次数阈值归一、health-probe 声明判定、相位映射）
 *
 * 后端契约（见 `specs/docs/design/modelinfer/auto-restart.md`）：
 * - `auto_restart_policy`：`never`（创建默认）或 `unless-stop`（docker unless-stopped 语义）；
 * - `unless-stop` 下 `auto_restart_fail_count` 为连续探活失败次数阈值（1~100，默认 10）；
 * - 启动过程失败或探活失败达到阈值都会触发自动重启；
 * - 卡片未声明 `health-probe` 时探活失败不会触发（但启动失败仍会触发）；
 * - 运行态相位 `""`（未在循环）/ `stopping`（停止清理中，随后启动）/ `starting`（启动中）。
 */

import { parse as parseYaml } from 'yaml';

/** 自动重启策略：不重启（创建默认） */
export const POLICY_NEVER = 'never';
/** 自动重启策略：除用户显式停止外自动重启 */
export const POLICY_UNLESS_STOP = 'unless-stop';
/** 连续探活失败次数下限 */
export const MIN_FAIL_COUNT = 1;
/** 连续探活失败次数上限 */
export const MAX_FAIL_COUNT = 100;
/** 连续探活失败次数默认值（按默认 30s 巡检周期约 5 分钟） */
export const DEFAULT_FAIL_COUNT = 10;

/** 自动重启相位常量（与后端一致） */
export const PHASE_IDLE = '';
export const PHASE_STOPPING = 'stopping';
export const PHASE_STARTING = 'starting';

/** 归一化策略（非法值回落 never） */
export function normalizePolicy(policy: unknown): string {
  return policy === POLICY_UNLESS_STOP ? POLICY_UNLESS_STOP : POLICY_NEVER;
}

/** 是否启用 unless-stop 自动重启 */
export function isUnlessStop(policy: unknown): boolean {
  return normalizePolicy(policy) === POLICY_UNLESS_STOP;
}

/** 归一化连续失败次数阈值（`1..=100`；非法 / 空 → 默认 10） */
export function clampFailCount(value: unknown): number {
  const n = typeof value === 'number' ? value : Number(value);
  if (!Number.isFinite(n)) return DEFAULT_FAIL_COUNT;
  const rounded = Math.round(n);
  if (rounded < MIN_FAIL_COUNT) return MIN_FAIL_COUNT;
  if (rounded > MAX_FAIL_COUNT) return MAX_FAIL_COUNT;
  return rounded;
}

/** 策略展示的 i18n key */
export function policyLabelKey(policy: unknown): string {
  return isUnlessStop(policy)
    ? 'runs.form.autoRestartPolicyUnlessStop'
    : 'runs.form.autoRestartPolicyNever';
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
