/**
 * VOS 平台桥接对象（`window.vos_platform`）适配层。
 *
 * 作为同域 iframe 嵌入 VOS 平台时：
 * - 等待平台注入桥接对象；
 * - 订阅平台语言（`locale-change`）与明暗主题（`theme-change`）变化；
 * - 首次加载调用 `getState()` 对齐最新状态，避免订阅前发生的变化丢失。
 *
 * 未嵌入（独立访问 / 跨域 / 超时）时返回 null，调用方保持本地行为。
 *
 * 依赖：`components/vivibit-web/openspec/changes/iframe-bridge-state-broadcast/`
 */

/** 平台明暗主题 */
export type PlatformTheme = 'dark' | 'light'

/** 应用语言（modelinfer 的 locale 取值） */
export type AppLocale = 'en' | 'zh-CN'

/** 平台主题模式（含跟随系统） */
export type PlatformThemeMode = 'auto' | 'dark' | 'light'

/** 平台状态快照（`vos_platform.getState()` 返回值的松散镜像） */
export interface VosPlatformState {
  locale?: string
  theme?: string
  themeMode?: string
}

/** 平台事件广播命名空间（仅消费 `on`） */
export interface VosPlatformBroadcast {
  on: (event: string, handler: (payload: Record<string, unknown>) => void) => () => void
}

/** 应用页面目标：app id + 该应用 routers.yml 中的 page id（对齐上游 openTab 契约） */
export interface VosPlatformOpenTabAppTarget {
  /** 应用 id（如 `com.ictrek.modelinfer`） */
  id: string
  /** 该应用 routers.yml 中声明的 page id */
  page: string
}

/** 多语言文本：单值字符串或多语言映射（对齐平台 `VosPlatformLocalizedText`） */
export type VosPlatformLocalizedText = string | Record<string, string>

/** `vos_platform.platform.openTab()` 入参（url / page / app 三选一） */
export interface VosPlatformOpenTabOptions {
  /** 显式 URL：相对 / 绝对路径（相对平台 origin）或完整 URL */
  url?: string
  /** 平台页面稳定标识（= 平台路由 name，与 URL 位置无关） */
  page?: string
  /** 应用页面（app id + routers.yml 声明的 page id） */
  app?: VosPlatformOpenTabAppTarget
  /** 路由参数（配合 page / app） */
  params?: Record<string, string>
  /** 查询参数（配合 page / app） */
  query?: Record<string, string>
  /**
   * 标签页标题（可选；URL 目标缺省回退 URL）。
   *
   * - `string`：单值字符串（旧平台 / 旧行为）；
   * - `Record<string, string>`：多语言映射，平台（桥接 >= 0.4.0）按当前语言解析并在语言切换 /
   *   刷新恢复时重解析（依赖 `iframe-bridge-open-tab-title-i18n`）。
   */
  title?: VosPlatformLocalizedText
}

/**
 * `vos_platform.platform` 辅助能力（应用侧松散镜像）。
 *
 * 仅消费 `openTab`；旧版本桥接对象（< 0.3.0）可能缺失该方法，调用方需做存在性检查。
 */
export interface VosPlatformHelpersLike {
  openTab?: (options: VosPlatformOpenTabOptions) => Promise<void>
}

/** 平台桥接对象的松散类型（应用侧按需读取，容忍旧版本字段缺失） */
export interface VosPlatformLike {
  version?: string
  mode?: string
  broadcast?: VosPlatformBroadcast
  getState?: () => Promise<VosPlatformState | undefined>
  /** 平台辅助能力（`getUser` / `openTab` 等；`openTab` 版本 >= 0.3.0） */
  platform?: VosPlatformHelpersLike
}

declare global {
  interface Window {
    vos_platform?: VosPlatformLike
  }
}

/** 等待注入的轮询间隔（ms） */
const PLATFORM_POLL_INTERVAL_MS = 50

/** 等待注入的默认超时（ms） */
export const DEFAULT_WAIT_TIMEOUT_MS = 3000

/**
 * 平台语言值 -> 应用语言映射（纯函数）。
 *
 * - `zh-CN` / `zh*` -> `zh-CN`
 * - `en-US` / 其他值 -> `en`
 * - 空值 -> `null`（调用方忽略，保持当前语言）
 */
export function mapPlatformLocale(
  locale: string | null | undefined,
): AppLocale | null {
  if (!locale) return null
  return locale.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en'
}

/**
 * 平台主题值 -> 明暗主题映射（纯函数）。
 *
 * 仅接受 `light` / `dark`；`auto` 等无法解析的值返回 `null`（调用方忽略）。
 */
export function mapPlatformTheme(
  theme: string | null | undefined,
): PlatformTheme | null {
  if (theme === 'dark') return 'dark'
  if (theme === 'light') return 'light'
  return null
}

/**
 * 等待平台桥接对象注入。
 *
 * - 已存在 -> 立即返回；
 * - 非 iframe 环境（独立访问）-> 立即返回 null（不等待）；
 * - iframe 内尚未注入 -> 轮询等待至超时，超时返回 null。
 *
 * @param timeoutMs 等待超时（ms）
 */
export async function waitForVosPlatform(
  timeoutMs: number = DEFAULT_WAIT_TIMEOUT_MS,
): Promise<VosPlatformLike | null> {
  if (typeof window === 'undefined') return null
  if (window.vos_platform) return window.vos_platform
  // 非 iframe：独立访问，无平台可等待
  if (window.parent === window) return null

  return new Promise((resolve) => {
    const start = Date.now()
    const timer = setInterval(() => {
      if (window.vos_platform) {
        clearInterval(timer)
        resolve(window.vos_platform)
      } else if (Date.now() - start > timeoutMs) {
        clearInterval(timer)
        resolve(null)
      }
    }, PLATFORM_POLL_INTERVAL_MS)
  })
}

/** 平台联动回调（由调用方接到 app store） */
export interface PlatformSyncHandlers {
  /** 语言变化（应用语言值） */
  onLocale?: (locale: AppLocale) => void
  /** 主题变化 */
  onTheme?: (theme: PlatformTheme) => void
}

/**
 * 初始化平台联动（非阻塞调用，不阻塞首屏挂载）。
 *
 * 顺序：先订阅事件（不丢后续事件），再 `await getState()` 对齐（补齐订阅前的变化）。
 * 桥接对象缺失 `broadcast` / `getState`（旧版本）时安全降级，不抛错。
 *
 * @returns 检测到的平台对象；未嵌入时为 null
 */
export async function initPlatformSync(
  handlers: PlatformSyncHandlers,
): Promise<VosPlatformLike | null> {
  const platform = await waitForVosPlatform()
  if (!platform) return null

  // 1. 先订阅（注册后不再丢事件）
  platform.broadcast?.on('locale-change', (payload) => {
    const locale = mapPlatformLocale(String(payload?.locale ?? ''))
    if (locale) handlers.onLocale?.(locale)
  })
  platform.broadcast?.on('theme-change', (payload) => {
    const theme = mapPlatformTheme(String(payload?.theme ?? ''))
    if (theme) handlers.onTheme?.(theme)
  })

  // 2. 再查询对齐（补齐订阅前已发生的变化）
  if (platform.getState) {
    try {
      const state = await platform.getState()
      const locale = mapPlatformLocale(state?.locale)
      if (locale) handlers.onLocale?.(locale)
      const theme = mapPlatformTheme(state?.theme)
      if (theme) handlers.onTheme?.(theme)
    } catch (error) {
      console.warn('[modelinfer] platform.getState() failed:', error)
    }
  }

  return platform
}