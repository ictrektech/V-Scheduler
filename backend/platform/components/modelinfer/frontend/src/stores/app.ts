import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { getLanguage, setLanguage, type AppLocale } from '@/i18n';

/** 应用主题 */
export type AppTheme = 'dark' | 'light';

/** 主题持久化 key（记录上次主题，减少 iframe 重载 / 平台同步前的闪烁） */
const THEME_STORAGE_KEY = 'modelinfer-theme';

/** 读取持久化主题（缺省浅色） */
export function readStoredTheme(): AppTheme {
  if (typeof localStorage === 'undefined') return 'light';
  return localStorage.getItem(THEME_STORAGE_KEY) === 'dark' ? 'dark' : 'light';
}

/** 同步主题标记到根元素（供 CSS 变量 `html.dark` 生效） */
export function applyThemeToDom(theme: AppTheme) {
  if (typeof document !== 'undefined') {
    document.documentElement.classList.toggle('dark', theme === 'dark');
  }
}

export const useAppStore = defineStore('app', () => {
  /** 当前语言（初始值来自 i18n 持久化） */
  const language = ref<AppLocale>(getLanguage());
  /** 当前主题（初始值来自本地缓存） */
  const theme = ref<AppTheme>(readStoredTheme());
  /** 是否暗黑主题 */
  const isDark = computed(() => theme.value === 'dark');

  // 初始化：把本地缓存主题应用到根元素（独立运行态：浅色 / 上次主题）
  applyThemeToDom(theme.value);

  /** 切换语言（联动 vue-i18n + localStorage + document.lang/title + dayjs） */
  function changeLanguage(lang: AppLocale) {
    setLanguage(lang);
    language.value = lang;
  }

  /** 设置主题（更新根元素标记 + 持久化） */
  function setTheme(value: AppTheme) {
    theme.value = value;
    if (typeof localStorage !== 'undefined') localStorage.setItem(THEME_STORAGE_KEY, value);
    applyThemeToDom(value);
  }

  return {
    language,
    theme,
    isDark,
    changeLanguage,
    setTheme,
  };
});