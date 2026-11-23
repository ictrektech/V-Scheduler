/**
 * 模型家族**呈现映射**（纯函数）。
 *
 * 家族**推断**由后端作 SSOT（`backend/src/model_family.rs`，见
 * `openspec/changes/modelinfer-list-pagination/design.md` §5）：列表视图随条目返回
 * `family` 标识，前端据此解析展示标签 / 品牌色 / 图标。
 *
 * 展示名称经 i18n（`modelFamily.<family>` key；品牌名为专有名词，中英可一致）由调用方用
 * `modelFamilyLabel(family, t)` 或 `t(theme.labelKey)` 渲染；本模块返回 `labelKey`。
 *
 * 本模块 MUST 保持纯函数（无副作用、无 I/O），便于 vitest 单测；图标 URL 解析在
 * `modelIconAssets.ts`（依赖 Vite `import.meta.glob`）中单独处理。
 * 本模块 MUST NOT 维护家族关键词表或推断逻辑（避免与后端两份表漂移）。
 */
import { translate } from '@/i18n';

export interface ModelFamilyDef {
  /** 家族标识（与后端 `family` 字段、图标文件名一致） */
  family: string;
  /** 展示名 i18n key（`modelFamily.<family>`） */
  labelKey: string;
  /** 品牌强调色（头像底 / 卡片背景渐变 / 水印） */
  accent: string;
}

/** 通用兜底家族 */
export const GENERIC_FAMILY: ModelFamilyDef = {
  family: 'generic',
  labelKey: 'modelFamily.generic',
  accent: '#64748b',
};

/** 家族呈现表（family → 展示标签 key / 品牌色） */
const FAMILY_THEMES: ModelFamilyDef[] = [
  { family: 'qwen', labelKey: 'modelFamily.qwen', accent: '#6336e7' },
  { family: 'deepseek', labelKey: 'modelFamily.deepseek', accent: '#4d6bfe' },
  { family: 'glm', labelKey: 'modelFamily.glm', accent: '#3859ff' },
  { family: 'kimi', labelKey: 'modelFamily.kimi', accent: '#1783ff' },
  { family: 'minimax', labelKey: 'modelFamily.minimax', accent: '#e2167e' },
  { family: 'internlm', labelKey: 'modelFamily.internlm', accent: '#4b65a5' },
  { family: 'yi', labelKey: 'modelFamily.yi', accent: '#00a870' },
  { family: 'baichuan', labelKey: 'modelFamily.baichuan', accent: '#ff7a33' },
  { family: 'hunyuan', labelKey: 'modelFamily.hunyuan', accent: '#0055e9' },
  { family: 'stepfun', labelKey: 'modelFamily.stepfun', accent: '#0160ff' },
  { family: 'doubao', labelKey: 'modelFamily.doubao', accent: '#1e37fc' },
  { family: 'ernie', labelKey: 'modelFamily.ernie', accent: '#2932e1' },
  { family: 'nemotron', labelKey: 'modelFamily.nemotron', accent: '#74b71b' },
  { family: 'llama', labelKey: 'modelFamily.llama', accent: '#0082fb' },
  { family: 'mistral', labelKey: 'modelFamily.mistral', accent: '#fa500f' },
  { family: 'gemma', labelKey: 'modelFamily.gemma', accent: '#446eff' },
  { family: 'gemini', labelKey: 'modelFamily.gemini', accent: '#3186ff' },
  { family: 'phi', labelKey: 'modelFamily.phi', accent: '#00a4ef' },
  { family: 'command', labelKey: 'modelFamily.command', accent: '#39594d' },
  { family: 'falcon', labelKey: 'modelFamily.falcon', accent: '#6400ff' },
  { family: 'solar', labelKey: 'modelFamily.solar', accent: '#805dfa' },
  { family: 'olmo', labelKey: 'modelFamily.olmo', accent: '#f0529c' },
  { family: 'dbrx', labelKey: 'modelFamily.dbrx', accent: '#ee3d2c' },
  { family: 'arctic', labelKey: 'modelFamily.arctic', accent: '#249edc' },
  { family: 'gpt', labelKey: 'modelFamily.gpt', accent: '#10a37f' },
  { family: 'claude', labelKey: 'modelFamily.claude', accent: '#d97757' },
  { family: 'grok', labelKey: 'modelFamily.grok', accent: '#111827' },
  // 工具 / 生态图标：可供显式展示（不参与后端家族推断）
  { family: 'huggingface', labelKey: 'modelFamily.huggingface', accent: '#ffd21e' },
  { family: 'vllm', labelKey: 'modelFamily.vllm', accent: '#3b6ef6' },
  { family: 'ollama', labelKey: 'modelFamily.ollama', accent: '#111827' },
];

const FAMILY_BY_KEY: Record<string, ModelFamilyDef> = FAMILY_THEMES.reduce(
  (acc, def) => {
    acc[def.family] = def;
    return acc;
  },
  {} as Record<string, ModelFamilyDef>,
);

/** 所有可展示家族（用于下拉筛选等） */
export const MODEL_FAMILIES: ModelFamilyDef[] = [...FAMILY_THEMES, GENERIC_FAMILY];

/** 按 family 键取呈现主题（未知 / 空回落兜底，MUST NOT 抛错） */
export function modelFamilyTheme(family?: string | null): ModelFamilyDef {
  if (!family) return GENERIC_FAMILY;
  return FAMILY_BY_KEY[family] ?? GENERIC_FAMILY;
}

/**
 * 家族展示名（当前语言）。传入响应式 `t`（`useI18n().t`）以保证语言切换后重算；
 * 缺省回退 i18n 全局单例。
 */
export function modelFamilyLabel(
  family?: string | null,
  t?: (key: string) => string,
): string {
  const key = modelFamilyTheme(family).labelKey;
  return t ? t(key) : translate(key);
}

/** 家族图标文件名（与家族键一致；未知回落 `generic`） */
export function modelFamilyIcon(family?: string | null): string {
  return modelFamilyTheme(family).family;
}

/** #rgb / #rrggbb → rgba(r,g,b,alpha)；非法输入回落中性灰 */
export function hexToRgba(hex: string, alpha: number): string {
  let h = hex.replace('#', '').trim();
  if (h.length === 3) h = h.split('').map((c) => c + c).join('');
  if (!/^[0-9a-f]{6}$/i.test(h)) return `rgba(100, 116, 139, ${alpha})`;
  const r = parseInt(h.slice(0, 2), 16);
  const g = parseInt(h.slice(2, 4), 16);
  const b = parseInt(h.slice(4, 6), 16);
  return `rgba(${r}, ${g}, ${b}, ${alpha})`;
}