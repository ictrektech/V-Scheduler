import { defineStore } from 'pinia';
import { computed, ref } from 'vue';
import { getLanguage, setLanguage, type AppLocale } from '@/i18n';

/** 应用主题 */
export type AppTheme = 'dark' | 'light';

/** 主题持久化 key（独立访问 / 平台同步前的兜底，减少 iframe 重载闪烁） */
export const THEME_STORAGE_KEY = 'image-hub-theme';

/** 读取持久化主题（缺省浅色） */
export function readStoredTheme(): AppTheme {
  if (typeof localStorage === 'undefined') return 'light';
  try {
    return localStorage.getItem(THEME_STORAGE_KEY) === 'dark' ? 'dark' : 'light';
  } catch {
    return 'light';
  }
}

/** 同步主题标记到根元素（供 `html.dark` 下的 CSS 变量覆盖生效） */
export function applyThemeToDom(theme: AppTheme): void {
  if (typeof document !== 'undefined') {
    document.documentElement.classList.toggle('dark', theme === 'dark');
  }
}

/**
 * 应用级状态：语言与明暗主题。
 *
 * 语言 / 主题由平台 `vos_platform` 注入的状态驱动（见 `platform/vos-platform.ts`）；
 * 独立访问时读取 localStorage。antd 的 locale / theme 在 `App.vue` 中据此响应式切换。
 */
export const useAppStore = defineStore('app', () => {
  /** 当前语言（初始值来自 i18n 持久化） */
  const language = ref<AppLocale>(getLanguage());
  /** 当前主题（初始值来自本地缓存） */
  const theme = ref<AppTheme>(readStoredTheme());
  /** 是否暗黑主题 */
  const isDark = computed(() => theme.value === 'dark');

  // 初始化：把本地缓存主题应用到根元素（独立运行态；平台同步后覆盖）
  applyThemeToDom(theme.value);

  /** 切换语言（联动 vue-i18n + localStorage + document.lang + dayjs + 路由标题） */
  function changeLanguage(lang: AppLocale): void {
    setLanguage(lang);
    language.value = lang;
  }

  /** 设置主题（更新根元素标记 + 持久化） */
  function setTheme(value: AppTheme): void {
    theme.value = value;
    if (typeof localStorage !== 'undefined') {
      try {
        localStorage.setItem(THEME_STORAGE_KEY, value);
      } catch {
        // 存储不可用时静默忽略
      }
    }
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