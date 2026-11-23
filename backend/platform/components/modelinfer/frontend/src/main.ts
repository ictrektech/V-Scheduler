import { createApp, watch } from 'vue';
import { createPinia } from 'pinia';
import Antd from 'ant-design-vue';
import App from './App.vue';
import { router, applyRouteTitle } from './router';
import i18n from './i18n';
import 'ant-design-vue/dist/reset.css';
import './style.css';
import { initPlatformSync } from './platform/vos-platform';
import { useAppStore } from './stores/app';

const app = createApp(App);
app.use(createPinia());
app.use(i18n);
app.use(Antd);
app.use(router);
app.mount('#app');

// 语言切换时重新应用当前路由的浏览器标题（页面内文案由 vue-i18n 响应式更新）
watch(
  () => useAppStore().language,
  () => applyRouteTitle(),
);

// 平台语言/主题联动（非阻塞，不阻塞首屏挂载）：
// - 嵌入 VOS 平台时跟随平台的 locale 与明暗主题；
// - 独立访问（无 window.vos_platform）时保持本地设置（localStorage / 浅色）。
void initPlatformSync({
  onLocale: (locale) => useAppStore().changeLanguage(locale),
  onTheme: (theme) => useAppStore().setTheme(theme),
});