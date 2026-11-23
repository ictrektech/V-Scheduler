/**
 * 状态语义色映射（纯函数）。
 *
 * 前端 MUST NOT 直接向用户回显英文原始枚举值（如 `stopped` / `unknown` / `available`）；
 * `StatusTag.vue` 依据本模块返回的 i18n key（`status.<kind>.<value>`）渲染本地化文案。
 * 未知取值回落原值（`fallback`），空值为 `common.none`（`-`）。
 */

export type StatusTone = 'success' | 'processing' | 'warning' | 'error' | 'neutral';

export interface StatusMeta {
  /** i18n key（如 `status.deployment.running`）；无可用 key 时为空串 */
  labelKey: string;
  /** 未知取值时的原值回显（已知或空值时为 ''） */
  fallback: string;
  tone: StatusTone;
}

/** 状态类别：决定使用哪张映射表 */
export type StatusKind =
  | 'deployment'
  | 'health'
  | 'version'
  | 'source'
  | 'vrouter'
  | 'node'
  | 'run'
  | 'runKind'
  | 'trigger'
  | 'stepState'
  | 'orchestrator'
  | 'usage'
  | 'authMethod'
  | 'gpuVendor'
  | 'monitor';

const DEPLOYMENT: Record<string, StatusTone> = {
  stopped: 'neutral',
  starting: 'processing',
  running: 'success',
  stopping: 'processing',
  failed: 'error',
  degraded: 'warning',
};

const HEALTH: Record<string, StatusTone> = {
  healthy: 'success',
  unhealthy: 'error',
  unknown: 'neutral',
};

const VERSION: Record<string, StatusTone> = {
  available: 'success',
  missing: 'warning',
  error: 'error',
};

const SOURCE: Record<string, StatusTone> = {
  local: 'neutral',
  plaza: 'processing',
};

const VROUTER: Record<string, StatusTone> = {
  registered: 'success',
  error: 'error',
  disabled: 'neutral',
  pending: 'warning',
  unregistered: 'warning',
};

const NODE: Record<string, StatusTone> = {
  online: 'success',
  offline: 'error',
  unknown: 'neutral',
};

/** 运行记录状态（run.state） */
const RUN: Record<string, StatusTone> = {
  succeeded: 'success',
  failed: 'error',
  running: 'processing',
  pending: 'warning',
};

/** 运行记录类型（run.kind） */
const RUN_KIND: Record<string, StatusTone> = {
  start: 'processing',
  stop: 'neutral',
  health: 'success',
  deploy: 'processing',
  cleanup: 'neutral',
};

/** 运行触发来源（run.trigger） */
const TRIGGER: Record<string, StatusTone> = {
  manual: 'neutral',
  auto: 'processing',
  scheduler: 'neutral',
  system: 'neutral',
};

/** 运行步骤状态（step.state） */
const STEP_STATE: Record<string, StatusTone> = {
  running: 'processing',
  succeeded: 'success',
  failed: 'error',
  skipped: 'neutral',
};

/** 步骤编排来源（step.orchestrator） */
const ORCHESTRATOR: Record<string, StatusTone> = {
  temporal: 'processing',
  scheduler: 'neutral',
};

/** 节点占用状态（空闲 / 占用） */
const USAGE: Record<string, StatusTone> = {
  idle: 'neutral',
  busy: 'warning',
};

/** 节点 SSH 认证方式 */
const AUTH_METHOD: Record<string, StatusTone> = {
  password: 'neutral',
  key: 'processing',
};

/** 节点监控部署状态 */
const MONITOR: Record<string, StatusTone> = {
  pending: 'neutral',
  deploying: 'processing',
  deployed: 'success',
  failed: 'error',
};

/** GPU 厂商 */
const GPU_VENDOR: Record<string, StatusTone> = {
  nvidia: 'success',
  amd: 'error',
  intel: 'processing',
};

const TABLES: Record<StatusKind, Record<string, StatusTone>> = {
  deployment: DEPLOYMENT,
  health: HEALTH,
  version: VERSION,
  source: SOURCE,
  vrouter: VROUTER,
  node: NODE,
  run: RUN,
  runKind: RUN_KIND,
  trigger: TRIGGER,
  stepState: STEP_STATE,
  orchestrator: ORCHESTRATOR,
  usage: USAGE,
  authMethod: AUTH_METHOD,
  gpuVendor: GPU_VENDOR,
  monitor: MONITOR,
};

/**
 * 状态 → i18n key + 语义色。未知取值回落原值（fallback），空值为 `common.none`。
 * MUST NOT 抛错。
 */
export function statusMeta(kind: StatusKind, value?: string | null): StatusMeta {
  if (!value) return { labelKey: 'common.none', fallback: '', tone: 'neutral' };
  const tone = TABLES[kind][value];
  if (tone) return { labelKey: `status.${kind}.${value}`, fallback: '', tone };
  return { labelKey: '', fallback: value, tone: 'neutral' };
}