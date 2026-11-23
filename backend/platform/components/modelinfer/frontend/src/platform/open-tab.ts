/**
 * 平台新标签页打开工具（`vos_platform.platform.openTab`）。
 *
 * 详情页（节点详情 `#/nodes/:id` / 运行配置详情 `#/runs/:id`）在平台同域 iframe 内
 * **优先**以独立 Vben 标签页打开：
 * - 桥接对象提供 `platform.openTab`（桥接版本 >= 0.3.0）时调用；
 * - 未注入 / 无该函数 / 调用失败（非未登录）时返回 `false`，由调用方回退当前页导航。
 *
 * 依赖：`components/vos-platform-backend/openspec/changes/iframe-bridge-open-tab/`
 * （`specs/docs/design/oidc-fastpath-iframe-auth/integration-guide.md` §7.7）。
 */

import { getLanguage } from '@/i18n'

import {
  pickLocalized,
  supportsLocalizedOpenTabTitle,
  type LocalizedTitle,
} from './localized-title'
import type {
  VosPlatformLike,
  VosPlatformLocalizedText,
  VosPlatformOpenTabOptions,
} from './vos-platform'

/** 详情页在新标签页打开的选项 */
export interface OpenDetailTabOptions {
  /** 应用内 hash 路由（含前导 `#`，如 `#/nodes/3`） */
  hashPath: string
  /**
   * 标签页标题（可选）。
   *
   * 建议传多语言 map（`localizedTitle(...)` 产出）：桥接 >= 0.4.0 时由平台按当前语言解析并
   * 在语言切换 / 刷新恢复时重解析；旧平台自动折叠为当前语言字符串。
   */
  title?: LocalizedTitle
}

/**
 * 构造指向应用内 hash 路由的**绝对 URL**（纯函数）。
 *
 * 以前端当前文档 URL 为基准解析：因构建 `base: './'`，不能依赖
 * `import.meta.env.BASE_URL`；以文档 URL 为基准可自动带上应用挂载基路径
 * （如 `/app/com.ictrek.modelinfer/`）。
 *
 * - 覆盖基准 URL 的 hash 为目标 hash（含/不含前导 `#` 均可）；
 * - 清空基准 URL 的 query，保证同一目标 URL 确定（利于平台按 fullPath 去重）。
 *
 * @param hashPath 目标 hash 路由
 * @param baseHref 基准 URL；缺省取 `window.location.href`
 * @returns 绝对 URL；基准缺失 / 非法时返回 `''`（调用方回退当前页）
 */
export function buildAppAbsoluteUrl(hashPath: string, baseHref?: string): string {
  const href =
    baseHref ?? (typeof window !== 'undefined' ? (window.location?.href ?? '') : '')
  if (!href) return ''
  try {
    const url = new URL(href)
    url.search = ''
    url.hash = hashPath.startsWith('#') ? hashPath : `#${hashPath}`
    return url.toString()
  } catch {
    return ''
  }
}

/** 读取当前注入的平台桥接对象（未注入 / 非浏览器环境返回 undefined） */
export function getInjectedVosPlatform(): VosPlatformLike | undefined {
  if (typeof window === 'undefined') return undefined
  return window.vos_platform
}

/**
 * 按平台桥接能力归一化标题：支持多语言（>= 0.4.0）时原样传 map，否则折叠为当前语言字符串。
 *
 * 旧平台会把 map 对象写入标签页 `meta.title` 并渲染为 `[object Object]`，故必须降级。
 *
 * @param title 多语言 / 字符串标题
 * @param platform 当前桥接对象
 */
export function normalizeTitleForPlatform(
  title: LocalizedTitle,
  platform: VosPlatformLike | null | undefined,
): VosPlatformLocalizedText {
  if (typeof title === 'string') return title
  if (supportsLocalizedOpenTabTitle(platform)) return title
  return pickLocalized(title, getLanguage())
}

/**
 * 在平台新标签页打开应用内详情路由。
 *
 * @returns `true` 表示已由平台处理（含未登录跳转登录页），调用方不应再回退；
 *          `false` 表示桥接不可用或调用失败，调用方应回退当前页导航。
 */
export async function openDetailInNewTab(options: OpenDetailTabOptions): Promise<boolean> {
  const platform = getInjectedVosPlatform()
  const openTab = platform?.platform?.openTab
  if (typeof openTab !== 'function') return false

  const url = buildAppAbsoluteUrl(options.hashPath)
  if (!url) return false

  const openOptions: VosPlatformOpenTabOptions = { url }
  if (options.title !== undefined) {
    openOptions.title = normalizeTitleForPlatform(options.title, platform)
  }

  try {
    await openTab(openOptions)
    return true
  } catch (error) {
    // 未登录：平台已跳转登录页，不再回退，避免二次动作
    if ((error as { name?: string } | undefined)?.name === 'PlatformNotAuthenticatedError') {
      return true
    }
    console.warn('[modelinfer] platform.openTab failed, fallback to in-page navigation:', error)
    return false
  }
}