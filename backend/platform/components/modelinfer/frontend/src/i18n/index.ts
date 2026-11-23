import { createI18n } from 'vue-i18n';
import dayjs from 'dayjs';
import 'dayjs/locale/zh-cn';
import zhCN from './locales/zh-CN';
import en from './locales/en';

/** 应用支持的语言（与 locales 目录一致） */
export type AppLocale = 'en' | 'zh-CN';

/** 全部支持语言（供 app store / 测试引用） */
export const SUPPORTED_LOCALES: AppLocale[] = ['zh-CN', 'en'];

/** 语言持久化 key */
export const LOCALE_STORAGE_KEY = 'modelinfer-lang';

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
  return localStorage.getItem(LOCALE_STORAGE_KEY) === 'en' ? 'en' : 'zh-CN';
}

// 语言包 key 数量很大，显式放宽 messages 类型，避免 TS 对深层 key 联合类型做超深实例化（TS2589）。
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
 * 同步更新 vue-i18n locale、持久化（localStorage）、`document.documentElement.lang` 与 dayjs locale；
 * 浏览器标题由 router 的 `applyRouteTitle` 统一维护（避免此处覆盖当前路由标题）；
 * antd `ConfigProvider` 的 locale 由 `App.vue` 依据 app store 响应切换。
 */
export function setLanguage(lang: AppLocale) {
  i18n.global.locale.value = lang;
  if (typeof localStorage !== 'undefined') localStorage.setItem(LOCALE_STORAGE_KEY, lang);
  dayjs.locale(DAYJS_LOCALE[lang]);
  if (typeof document !== 'undefined') {
    document.documentElement.lang = HTML_LANG[lang];
  }
}

/** 读取当前语言 */
export function getLanguage(): AppLocale {
  return i18n.global.locale.value as AppLocale;
}

// 首帧同步 dayjs locale（node/单测环境无 document 也需生效）
const initialLang = getLanguage();
dayjs.locale(DAYJS_LOCALE[initialLang]);
// 首帧同步 <html lang>（标题由 setLanguage 更新；此处不覆盖 index.html 初始值）
if (typeof document !== 'undefined') {
  document.documentElement.lang = HTML_LANG[initialLang];
}