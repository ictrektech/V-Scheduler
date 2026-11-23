/**
 * 模型卡片 manifest 解析（纯函数）：把 `manifest.yaml` 原文转换为「关键结构化信息」用于展示。
 *
 * 字段命名遵循模型包规范的 kebab-case（`min-length` / `lifecycle-order` / `lifecycle-timeout`
 * / `max-nodes` 等）；解析失败返回 `null`（调用方回落展示原文）。
 */
import { parse } from 'yaml';
import i18n, { translate } from '@/i18n';

/** 当前语言下的本地化文案（纯函数内取 i18n 单例，保持签名不变） */
function t(key: string, named?: Record<string, unknown>): string {
  return translate(key, named);
}

export interface ManifestConfigView {
  /** 环境变量名 */
  name: string;
  type: string;
  /** 展示名称（多语言取 zh-CN / 首个字符串） */
  title: string;
  /** 默认值展示文本（无默认 = 必填） */
  defaultValue: string;
  /** 取值约束（范围 / 长度 / 正则 / 枚举） */
  constraint: string;
  description: string;
  required: boolean;
}

export interface ManifestImageView {
  name: string;
  /** 适用架构；通用为 `（通用）` */
  arch: string;
}

export interface ManifestStageView {
  stage: string;
  /** 脚本（字符串按原样、数组按空格拼接） */
  script: string;
  /** 节点执行顺序（缺省 head-first） */
  order: string;
  /** 超时（秒）；未声明为 undefined */
  timeout?: number;
}

export interface ManifestView {
  id: string;
  name: string;
  version: string;
  author: string;
  arch: string;
  model: string;
  category: string[];
  tags: string[];
  /** `caps.max-nodes`：`不限制` 或具体上限 */
  maxNodes: string;
  images: ManifestImageView[];
  configs: ManifestConfigView[];
  lifecycle: ManifestStageView[];
}

function str(v: unknown): string {
  if (v == null) return '';
  if (Array.isArray(v)) return v.map((x) => String(x)).join(', ');
  return String(v);
}

/** 多语言值或字符串 → 展示文本（当前应用语言优先，其次 zh-CN，再次首个字符串） */
function pickText(v: unknown): string {
  if (v == null) return '';
  if (typeof v === 'string') return v;
  if (typeof v === 'object') {
    const o = v as Record<string, unknown>;
    const locale = String(i18n.global.locale.value);
    if (typeof o[locale] === 'string') return o[locale] as string;
    if (typeof o['zh-CN'] === 'string') return o['zh-CN'];
    const first = Object.values(o).find((x) => typeof x === 'string');
    return typeof first === 'string' ? first : '';
  }
  return String(v);
}

function arr(v: unknown): unknown[] {
  return Array.isArray(v) ? v : [];
}

function obj(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {};
}

/** 构造配置项取值约束展示文本 */
function constraintOf(c: Record<string, unknown>): string {
  const parts: string[] = [];
  if (typeof c.minimum === 'number') parts.push(t('common.rangeMin', { min: c.minimum }));
  if (typeof c.maximum === 'number') parts.push(t('common.rangeMax', { max: c.maximum }));
  if (typeof c['min-length'] === 'number')
    parts.push(t('common.lengthMin', { min: c['min-length'] }));
  if (typeof c['max-length'] === 'number')
    parts.push(t('common.lengthMax', { max: c['max-length'] }));
  if (typeof c.pattern === 'string' && c.pattern) parts.push(t('common.pattern', { pattern: c.pattern }));
  if (Array.isArray(c.enum)) parts.push(`${t('common.enumPrefix')}${c.enum.map((x) => String(x)).join(t('common.enumJoin'))}`);
  return parts.join(t('common.rangeJoin'));
}

function defaultValueOf(c: Record<string, unknown>): string {
  const d = c.default;
  if (d === undefined) return t('common.notDefinedRequired');
  if (typeof d === 'boolean') return d ? 'true' : 'false';
  if (d === '') return t('common.emptyString');
  return String(d);
}

function scriptOf(v: unknown): string {
  if (Array.isArray(v)) return v.map((x) => String(x)).join(' ');
  return str(v);
}

/**
 * 解析 manifest 原文为结构化视图；解析失败或非对象时返回 `null`。
 */
export function parseManifest(text: string): ManifestView | null {
  if (!text || !text.trim()) return null;
  let doc: unknown;
  try {
    doc = parse(text);
  } catch {
    return null;
  }
  if (!doc || typeof doc !== 'object' || Array.isArray(doc)) return null;
  const m = doc as Record<string, unknown>;

  const lifecycleRaw = obj(m.lifecycle);
  const orderRaw = obj(m['lifecycle-order']);
  const timeoutRaw = obj(m['lifecycle-timeout']);
  const caps = obj(m.caps);
  const maxNodesRaw = caps['max-nodes'];

  return {
    id: str(m.id),
    name: pickText(m.name),
    version: str(m.version),
    author: str(m.author),
    arch: str(m.arch) || 'any',
    model: str(m.model),
    category: arr(m.category).map((x) => String(x)),
    tags: arr(m.tags).map((x) => String(x)),
    maxNodes:
      typeof maxNodesRaw === 'number' && maxNodesRaw > 0 ? String(maxNodesRaw) : t('common.noLimit'),
    images: arr(m.images)
      .map((i) => {
        if (typeof i === 'string') return { name: i, arch: t('common.genericArch') };
        const o = obj(i);
        return { name: str(o.name), arch: str(o.arch) || t('common.genericArch') };
      })
      .filter((x) => x.name),
    configs: arr(m.configs).map((c) => {
      const o = obj(c);
      return {
        name: str(o.name),
        type: str(o.type),
        title: pickText(o.title),
        defaultValue: defaultValueOf(o),
        constraint: constraintOf(o),
        description: pickText(o.description),
        required: o.default === undefined,
      };
    }),
    lifecycle: Object.keys(lifecycleRaw).map((stage) => ({
      stage,
      script: scriptOf(lifecycleRaw[stage]),
      order: str(orderRaw[stage]) || 'head-first',
      timeout: typeof timeoutRaw[stage] === 'number' ? (timeoutRaw[stage] as number) : undefined,
    })),
  };
}