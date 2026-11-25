import { createApp } from 'vue';
import { createPinia } from 'pinia';
import Antd from 'ant-design-vue';
import App from './App.vue';
import { applyRouteTitle, router } from './router';
import i18n from './i18n';
import 'ant-design-vue/dist/reset.css';
import './style.css';
import { initPlatformSync } from './platform/vos-platform';
import { useAppStore } from './stores/app';
import { getSchedulerRuntime } from '@scheduler-auth';

const app = createApp(App);
app.use(createPinia());
app.use(i18n);
app.use(Antd);
app.use(router);
app.mount('#app');

// 首帧标题（路由 ready 后按当前路由的 i18n 标题刷新）
void router.isReady().then(() => applyRouteTitle());

// 平台语言/主题联动（非阻塞，不阻塞首屏挂载）：
// - 嵌入 VOS 平台时跟随平台的 locale 与明暗主题（先订阅事件，再 getState 对齐）；
// - 独立访问（无 window.vos_platform）时保持本地设置（localStorage / 浅色）。
const scheduler = getSchedulerRuntime();
if (scheduler) {
  const applyLocale = () => useAppStore().changeLanguage(scheduler.locale() === 'en' ? 'en' : 'zh-CN');
  applyLocale();
  window.addEventListener('scheduler:locale', applyLocale);
}
void initPlatformSync({
  onLocale: (locale) => { if (!scheduler) useAppStore().changeLanguage(locale); },
  onTheme: (theme) => useAppStore().setTheme(theme),
});