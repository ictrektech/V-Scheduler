import { createI18n } from 'vue-i18n';
import dayjs from 'dayjs';
import 'dayjs/locale/zh-cn';
import zhCN from './locales/zh-CN';
import en from './locales/en';

/**
 * 应用语言（与 `locales/` 目录一致）。
 *
 * 平台语言（BCP-47）经 `platform/vos-platform.ts` 的 `mapPlatformLocale` 映射：
 * `zh*` → `zh-CN`，其余 → `en`。
 */
export type AppLocale = 'zh-CN' | 'en';

/** 全部支持语言（供 app store / 测试引用） */
export const SUPPORTED_LOCALES: AppLocale[] = ['zh-CN', 'en'];

/** 语言持久化 key（独立访问 / 平台同步前的本地兜底） */
export const LOCALE_STORAGE_KEY = 'image-hub-lang';

/** `<html lang>` 取值 */
const HTML_LANG: Record<AppLocale, string> = {
  'zh-CN': 'zh-CN',
  en: 'en',
};

/** dayjs locale 取值 */
const DAYJS_LOCALE: Record<AppLocale, string> = {
  'zh-CN': 'zh-cn',
  en: 'en',
};

/** 读取持久化语言（缺省 / 未知值回退中文） */
export function readStoredLocale(): AppLocale {
  if (typeof localStorage === 'undefined') return 'zh-CN';
  try {
    return localStorage.getItem(LOCALE_STORAGE_KEY) === 'en' ? 'en' : 'zh-CN';
  } catch {
    return 'zh-CN';
  }
}

// 语言包 key 数量较大，显式放宽 messages 类型，避免 TS 对深层 key 联合类型做超深实例化（TS2589）。
const messages: Record<string, Record<string, unknown>> = {
  'zh-CN': zhCN as unknown as Record<string, unknown>,
  en: en as unknown as Record<string, unknown>,
};

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const i18n = createI18n({
  legacy: false,
  locale: readStoredLocale(),
  fallbackLocale: 'zh-CN',
  messages: messages as any,
});

export default i18n;

/** 语言变化后的标题刷新回调（由 router 注册，避免 i18n ↔ router 循环依赖） */
let titleResolver: (() => void) | null = null;

/**
 * 注册语言变化后需要执行的回调（当前用于刷新浏览器标题）。
 *
 * @param resolver 无参回调；重复注册时后注册者生效
 */
export function onLanguageChanged(resolver: () => void): void {
  titleResolver = resolver;
}

/**
 * 组件外翻译辅助（海量 key 下直接调用 `i18n.global.t('literal')` 会触发 TS 超深类型实例化，
 * 此处以宽松签名封装）。
 */
export function translate(key: string, named?: Record<string, unknown>): string {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return (i18n.global as any).t(key, named ?? {});
}

/** 判断 i18n key 是否存在（宽松签名，避免 TS 超深实例化）。 */
export function hasTranslation(key: string): boolean {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  return Boolean((i18n.global as any).te(key));
}

/**
 * 切换语言。
 *
 * 同步更新 vue-i18n locale、持久化（localStorage）、`document.documentElement.lang`、dayjs
 * locale，并触发已注册的标题刷新回调；antd `ConfigProvider.locale` 由 `App.vue` 依据 app
 * store 响应切换。
 */
export function setLanguage(lang: AppLocale): void {
  i18n.global.locale.value = lang;
  if (typeof localStorage !== 'undefined') {
    try {
      localStorage.setItem(LOCALE_STORAGE_KEY, lang);
    } catch {
      // 隐私模式等存储不可用时静默忽略
    }
  }
  dayjs.locale(DAYJS_LOCALE[lang]);
  if (typeof document !== 'undefined') {
    document.documentElement.lang = HTML_LANG[lang];
  }
  titleResolver?.();
}

/** 读取当前语言 */
export function getLanguage(): AppLocale {
  return i18n.global.locale.value as AppLocale;
}

// 首帧同步 dayjs locale（node 单测环境无 document 也需生效）
dayjs.locale(DAYJS_LOCALE[getLanguage()]);
// 首帧同步 <html lang>（标题由路由在挂载后应用）
if (typeof document !== 'undefined') {
  document.documentElement.lang = HTML_LANG[getLanguage()];
}