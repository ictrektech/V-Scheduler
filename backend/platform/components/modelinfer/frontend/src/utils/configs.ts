/**
 * 运行配置自定义参数（configs）展示辅助（纯函数）。
 *
 * 用于运行配置详情页「参数配置」审查弹窗：把 `deployment.configs` 转为有序的
 * 「参数名 / 当前值」展示行，值按类型格式化，避免在组件内散写格式化逻辑。
 */

import { translate } from '@/i18n';

export interface ConfigRow {
  /** 参数名（卡片 configs 的 `name`） */
  name: string;
  /** 展示值文本 */
  valueText: string;
}

/**
 * 参数值 → 展示文本：
 * - `boolean` → 当前语言的「是」/「否」；
 * - `string` / `number` / `bigint` → 原样；
 * - `null` / `undefined` → `-`；
 * - 对象 / 数组 → 紧凑 JSON 文本。
 */
export function formatConfigValue(value: unknown): string {
  if (value === null || value === undefined) return '-';
  if (typeof value === 'boolean') return value ? translate('common.yes') : translate('common.no');
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'bigint') return String(value);
  try {
    return JSON.stringify(value) ?? String(value);
  } catch {
    return String(value);
  }
}

/** configs → 按参数名字典序排序的展示行（容错非对象 / 空值） */
export function configRows(configs: Record<string, unknown> | null | undefined): ConfigRow[] {
  if (!configs || typeof configs !== 'object') return [];
  return Object.keys(configs)
    .sort((a, b) => a.localeCompare(b))
    .map((name) => ({ name, valueText: formatConfigValue(configs[name]) }));
}