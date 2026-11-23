/**
 * 多语言标题工具（openTab 详情标签页标题）。
 *
 * 平台（桥接 >= 0.4.0，`iframe-bridge-open-tab-title-i18n`）支持 `openTab.title` 传多语言 map，
 * 由平台按当前语言解析并在语言切换 / 刷新恢复时重解析。应用侧负责提供"与语言无关的多语言标题
 * 内容"：本模块提供产出 map / 取单值 / 桥接能力探测的纯函数。
 *
 * 依赖：`components/vivibit-web/openspec/changes/iframe-bridge-open-tab-title-i18n/`。
 */

import i18n, { SUPPORTED_LOCALES, type AppLocale } from '@/i18n'

import type { VosPlatformLike, VosPlatformLocalizedText } from './vos-platform'

/** 多语言标题（对齐平台 `VosPlatformLocalizedText`） */
export type LocalizedTitle = VosPlatformLocalizedText

/** 支持多语言 openTab 标题的最低桥接版本 */
export const LOCALIZED_OPEN_TAB_TITLE_MIN_VERSION = '0.4.0'

/**
 * 按点路径从消息对象取值（纯函数）。
 *
 * @param messages 语言包消息对象（如 `i18n.global.getLocaleMessage('en')`）
 * @param key 点分路径（如 `routes.nodeDetail`）
 * @returns 命中的字符串；非字符串 / 缺失时 undefined
 */
export function resolveMessagePath(messages: unknown, key: string): string | undefined {
  let current: unknown = messages
  for (const part of key.split('.')) {
    if (current && typeof current === 'object' && part in (current as Record<string, unknown>)) {
      current = (current as Record<string, unknown>)[part]
    } else {
      return undefined
    }
  }
  return typeof current === 'string' ? current : undefined
}

/**
 * 在指定语言下取某 i18n key 的文案。
 *
 * @param locale 目标语言（应用支持的语言）
 * @param key 点分路径
 * @param fallback 取值失败时的回退（缺省回退 key 本身）
 */
export function translateIn(locale: AppLocale, key: string, fallback?: string): string {
  const messages = i18n.global.getLocaleMessage(locale) as unknown
  return resolveMessagePath(messages, key) ?? fallback ?? key
}

/**
 * 产出某 i18n key 的多语言标题 map（遍历应用支持语言）。
 *
 * 同一 key 在各语言下的文案 + 语言中性的实体名后缀（` · <suffix>`）；未知 key 回退 key 本身。
 *
 * @param key i18n key（如 `routes.nodeDetail`）
 * @param suffix 语言中性后缀（如实体名）；缺省不拼接
 * @returns `{ 'zh-CN': ..., en: ... }`
 */
export function localizedTitle(key: string, suffix?: string): Record<string, string> {
  const result: Record<string, string> = {}
  for (const locale of SUPPORTED_LOCALES) {
    const base = translateIn(locale, key)
    result[locale] = suffix ? `${base} · ${suffix}` : base
  }
  return result
}

/**
 * 从多语言标题取指定语言的单值字符串（纯函数）。
 *
 * 回退链：精确 locale → `zh-CN` → `en` → 首个可用值 → 空串。
 */
export function pickLocalized(text: LocalizedTitle, locale: AppLocale): string {
  if (typeof text === 'string') return text
  return text[locale] ?? text['zh-CN'] ?? text.en ?? Object.values(text)[0] ?? ''
}

/**
 * 语义化版本比较：`version >= minimum`（纯函数）。
 *
 * 忽略预发布段（`-` 之后），仅比较 major.minor.patch；缺失 / 非法段按 0 处理。
 */
export function isVersionAtLeast(version: string | undefined, minimum: string): boolean {
  if (!version) return false
  const parse = (value: string): number[] =>
    value
      .split('-')[0]!
      .split('.')
      .map((part) => Number.parseInt(part, 10) || 0)
  const actual = parse(version)
  const required = parse(minimum)
  for (let index = 0; index < 3; index += 1) {
    const left = actual[index] ?? 0
    const right = required[index] ?? 0
    if (left !== right) return left > right
  }
  return true
}

/**
 * 探测平台桥接对象是否支持多语言 openTab 标题（`version >= 0.4.0`）。
 *
 * 不支持时调用方应把多语言标题折叠为当前语言字符串，避免旧平台把对象渲染为 `[object Object]`。
 */
export function supportsLocalizedOpenTabTitle(platform?: VosPlatformLike | null): boolean {
  return isVersionAtLeast(platform?.version, LOCALIZED_OPEN_TAB_TITLE_MIN_VERSION)
}