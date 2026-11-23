/**
 * VOS 平台桥接对象（`window.vos_platform`）适配层。
 *
 * 作为同域 iframe 嵌入 VOS 平台时：
 * - 等待平台注入桥接对象；
 * - **先**订阅平台语言（`locale-change`）与明暗主题（`theme-change`）变化；
 * - **再**调用 `getState()` 对齐最新状态，补齐订阅前发生的变化。
 *
 * 未嵌入（独立访问 / 跨域 / 超时）或能力缺失（旧平台 / 移动 WebView）时返回 null / 忽略，
 * 调用方保持本地设置（localStorage），不阻塞首屏。
 *
 * 契约来源：`specs/docs/design/vos-app-spec/iframe-bridge.md` §6 / §9；
 * 设计约束见 `components/image-hub/openspec/changes/image-hub-platform-theme-and-i18n/design.md`
 * （D-IH-87）。
 */

/** 平台明暗主题 */
export type PlatformTheme = 'dark' | 'light';

/**
 * 应用语言（与 `i18n/index.ts` 的 `AppLocale` 结构一致）。
 *
 * 此处独立声明而不导入 i18n 模块，保证本模块可被 `node --test` 直接导入（零别名 / 零三方依赖）。
 */
export type AppLocale = 'en' | 'zh-CN';

/** 平台状态快照（`vos_platform.getState()` 返回值的松散镜像） */
export interface VosPlatformState {
  locale?: string;
  theme?: string;
  themeMode?: string;
}

/** 平台事件广播命名空间（仅消费 `on`；旧平台可能缺失） */
export interface VosPlatformBroadcast {
  on: (event: string, handler: (payload: Record<string, unknown>) => void) => () => void;
}

/** 平台桥接对象的松散类型（应用侧按需读取，容忍旧版本字段缺失） */
export interface VosPlatformLike {
  version?: string;
  mode?: string;
  broadcast?: VosPlatformBroadcast;
  getState?: () => Promise<VosPlatformState | undefined>;
}

declare global {
  interface Window {
    vos_platform?: VosPlatformLike;
  }
}

/** 等待注入的轮询间隔（ms） */
const PLATFORM_POLL_INTERVAL_MS = 50;

/** 等待注入的默认超时（ms） */
export const DEFAULT_WAIT_TIMEOUT_MS = 3000;

/**
 * 平台语言值 -> 应用语言映射（纯函数）。
 *
 * - `zh-CN` / `zh` / 其它 `zh*` -> `zh-CN`
 * - `en-US` / 其它值 -> `en`
 * - 空值 / `undefined` -> `null`（调用方忽略，保持当前语言）
 */
export function mapPlatformLocale(locale: string | null | undefined): AppLocale | null {
  if (!locale) return null;
  return locale.toLowerCase().startsWith('zh') ? 'zh-CN' : 'en';
}

/**
 * 平台主题值 -> 明暗主题映射（纯函数）。
 *
 * 仅接受 `light` / `dark`；`auto` 等无法解析的值返回 `null`（调用方忽略）。
 * 注意：`themeMode` 不参与判断——`auto` 时平台已把 `theme` 解析为实际明暗。
 */
export function mapPlatformTheme(theme: string | null | undefined): PlatformTheme | null {
  if (theme === 'dark') return 'dark';
  if (theme === 'light') return 'light';
  return null;
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
  if (typeof window === 'undefined') return null;
  if (window.vos_platform) return window.vos_platform;
  // 非 iframe：独立访问，无平台可等待（移动 WebView 若是顶层窗口但已注入对象，
  // 上方 `window.vos_platform` 判断已覆盖）
  if (window.parent === window) return null;

  return new Promise((resolve) => {
    const start = Date.now();
    const timer = setInterval(() => {
      if (window.vos_platform) {
        clearInterval(timer);
        resolve(window.vos_platform);
      } else if (Date.now() - start > timeoutMs) {
        clearInterval(timer);
        resolve(null);
      }
    }, PLATFORM_POLL_INTERVAL_MS);
  });
}

/** 平台联动回调（由调用方接到 app store） */
export interface PlatformSyncHandlers {
  /** 语言变化（应用语言值） */
  onLocale?: (locale: AppLocale) => void;
  /** 主题变化 */
  onTheme?: (theme: PlatformTheme) => void;
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
  const platform = await waitForVosPlatform();
  if (!platform) return null;

  // 1. 先订阅（注册后不再丢事件）
  platform.broadcast?.on('locale-change', (payload) => {
    const locale = mapPlatformLocale(String(payload?.locale ?? ''));
    if (locale) handlers.onLocale?.(locale);
  });
  platform.broadcast?.on('theme-change', (payload) => {
    const theme = mapPlatformTheme(String(payload?.theme ?? ''));
    if (theme) handlers.onTheme?.(theme);
  });

  // 2. 再查询对齐（补齐订阅前已发生的变化）
  if (platform.getState) {
    try {
      const state = await platform.getState();
      const locale = mapPlatformLocale(state?.locale);
      if (locale) handlers.onLocale?.(locale);
      const theme = mapPlatformTheme(state?.theme);
      if (theme) handlers.onTheme?.(theme);
    } catch (error) {
      console.warn('[image-hub] platform.getState() failed:', error);
    }
  }

  return platform;
}