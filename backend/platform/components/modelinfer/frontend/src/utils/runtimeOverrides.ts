/**
 * 运行配置「权重 / 镜像覆盖 + 额外环境变量」纯函数集合。
 *
 * 设计见 `specs/docs/design/modelinfer/runtime-overrides.md`：
 * - 权重覆盖 `{source, model_id}`，未覆盖时使用卡片 `manifest.model`；
 * - 镜像覆盖与卡片 `manifest.images` 同结构（`string` 或 `{name, arch}`），非空**整体替换**；
 * - 额外环境变量在卡片 `configs` 之后注入，前端按与后端同规则做即时校验（后端为权威层）。
 */
import type { ImageEntry, WeightOverride } from '@/api/deployments';
import type { ModelHubVariant } from '@/api/modelHub';

/** 表单中的镜像行（arch 空串 = 通用） */
export interface EditableImage {
  name: string;
  arch: string;
}

/** 表单中的额外环境变量行 */
export interface EnvRow {
  key: string;
  value: string;
}

/** 可选镜像架构（arch 空串 = 通用） */
export const IMAGE_ARCH_VALUES = ['', 'amd64', 'arm64'] as const;

/** 平台保留环境变量前缀（与后端一致） */
export const RESERVED_ENV_PREFIX = 'MODELINFER_';

/** 禁止覆盖的关键系统变量（与后端一致） */
export const FORBIDDEN_ENV_KEYS: readonly string[] = [
  'PATH',
  'HOME',
  'SHELL',
  'IFS',
  'BASH_ENV',
  'ENV',
  'LD_PRELOAD',
  'LD_LIBRARY_PATH',
  'PWD',
  'OLDPWD',
  'TMPDIR',
];

/** 额外环境变量条数上限（与后端一致） */
export const MAX_EXTRA_ENV = 64;

/** 镜像覆盖条数上限（与后端一致） */
export const MAX_IMAGES_OVERRIDE = 32;

/** 环境变量名校验错误码（供 i18n 映射） */
export type EnvKeyError =
  | 'empty'
  | 'invalid'
  | 'reserved'
  | 'system'
  | 'conflict'
  | 'duplicate'
  | 'overflow';

/** 单个镜像条目的名称 */
export function imageEntryName(entry: ImageEntry): string {
  return typeof entry === 'string' ? entry : entry?.name || '';
}

/** 单个镜像条目的架构（空串 = 通用） */
export function imageEntryArch(entry: ImageEntry): string {
  return typeof entry === 'string' ? '' : entry?.arch || '';
}

/** 镜像条目 → 展示文本（通用架构不附加标记） */
export function imageEntryLabel(entry: ImageEntry): string {
  const name = imageEntryName(entry);
  const arch = imageEntryArch(entry);
  return arch ? `${name} (${arch})` : name;
}

/** 镜像条目数组 → 可编辑行（过滤空条目） */
export function toEditableImages(items: ImageEntry[] | undefined | null): EditableImage[] {
  if (!Array.isArray(items)) return [];
  return items
    .map((item) => ({ name: imageEntryName(item).trim(), arch: imageEntryArch(item).trim() }))
    .filter((row) => row.name !== '');
}

/** 可编辑行 → 镜像条目数组（去空白 / 丢弃空行；arch 空则写字符串形式） */
export function fromEditableImages(rows: EditableImage[] | undefined | null): ImageEntry[] {
  if (!Array.isArray(rows)) return [];
  const out: ImageEntry[] = [];
  for (const row of rows) {
    const name = (row?.name || '').trim();
    if (!name) continue;
    const arch = (row?.arch || '').trim();
    out.push(arch ? { name, arch } : name);
  }
  return out;
}

/** 权重覆盖 → 下拉值 `source:model_id`（未覆盖返回空串） */
export function weightOverrideValue(override?: WeightOverride | null): string {
  if (!override || !override.model_id?.trim()) return '';
  return `${override.source?.trim() || 'hf'}:${override.model_id.trim()}`;
}

/** 下拉值 → 权重覆盖（空值返回 null） */
export function parseWeightOverrideValue(value?: string | null): WeightOverride | null {
  const raw = (value || '').trim();
  if (!raw) return null;
  const idx = raw.indexOf(':');
  if (idx <= 0 || idx === raw.length - 1) return null;
  return { source: raw.slice(0, idx), model_id: raw.slice(idx + 1) };
}

/**
 * model-hub 已下载变体 → 权重下拉选项（按 `source:model_id` 去重，按 label 排序）。
 *
 * 同一权重可能同时存在 `hf` / `ms` 变体，因此以来源前缀区分，避免误选。
 */
export function weightOptions(
  models: ModelHubVariant[] | undefined | null,
  labelFn?: (source: string, modelId: string) => string,
): Array<{ label: string; value: string }> {
  const seen = new Set<string>();
  const out: Array<{ label: string; value: string }> = [];
  for (const m of models || []) {
    const source = (m?.source || '').trim();
    const modelId = (m?.model_id || '').trim();
    if (!source || !modelId) continue;
    const value = `${source}:${modelId}`;
    if (seen.has(value)) continue;
    seen.add(value);
    out.push({ label: labelFn ? labelFn(source, modelId) : value, value });
  }
  out.sort((a, b) => a.label.localeCompare(b.label));
  return out;
}

/** 额外环境变量对象 → 可编辑行（按 key 排序保证稳定） */
export function envMapToRows(map: Record<string, string> | undefined | null): EnvRow[] {
  if (!map || typeof map !== 'object') return [];
  return Object.entries(map)
    .map(([key, value]) => ({ key, value: value ?? '' }))
    .sort((a, b) => a.key.localeCompare(b.key));
}

/** 可编辑行 → 额外环境变量对象（忽略空键行；重复键取首个） */
export function envRowsToMap(rows: EnvRow[] | undefined | null): Record<string, string> {
  const out: Record<string, string> = {};
  for (const row of rows || []) {
    const key = (row?.key || '').trim();
    if (!key || key in out) continue;
    out[key] = row.value ?? '';
  }
  return out;
}

/** 环境变量名是否合法（`[A-Za-z_][A-Za-z0-9_]*`） */
export function isValidEnvKey(key: string): boolean {
  return /^[A-Za-z_][A-Za-z0-9_]*$/.test(key);
}

/**
 * 校验单个额外环境变量名（与后端 `validate_extra_env` 同规则）。
 *
 * @param key 变量名（去空白后）
 * @param cardConfigNames 卡片 `configs` 声明的参数名
 * @param seen 已出现的变量名（用于重复检测；不含当前行）
 */
export function envKeyError(
  key: string,
  cardConfigNames: readonly string[] = [],
  seen: ReadonlySet<string> = new Set(),
): EnvKeyError | null {
  const k = (key || '').trim();
  if (!k) return 'empty';
  if (!isValidEnvKey(k) || k.length > 128) return 'invalid';
  if (k.startsWith(RESERVED_ENV_PREFIX)) return 'reserved';
  if (FORBIDDEN_ENV_KEYS.includes(k)) return 'system';
  if (cardConfigNames.includes(k)) return 'conflict';
  if (seen.has(k)) return 'duplicate';
  return null;
}

/** 批量校验环境变量行：返回「行号 → 错误码」（行号从 0 开始） */
export function validateEnvRows(
  rows: EnvRow[] | undefined | null,
  cardConfigNames: readonly string[] = [],
): Map<number, EnvKeyError> {
  const errors = new Map<number, EnvKeyError>();
  const seen = new Set<string>();
  (rows || []).forEach((row, idx) => {
    const key = (row?.key || '').trim();
    // 整行空（键与值都为空）视为未填写，跳过
    if (!key && !(row?.value || '').trim()) return;
    const err = envKeyError(key, cardConfigNames, seen);
    if (err) errors.set(idx, err);
    if (key) seen.add(key);
  });
  if ((rows || []).length > MAX_EXTRA_ENV && !errors.has(0)) errors.set(0, 'overflow');
  return errors;
}

/** 镜像行是否超出上限（前端提示；后端为权威校验） */
export function imagesOverflow(rows: EditableImage[] | undefined | null): boolean {
  return (rows || []).length > MAX_IMAGES_OVERRIDE;
}