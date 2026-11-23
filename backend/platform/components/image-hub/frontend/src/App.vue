<script setup lang="ts">
import { computed } from 'vue';
import { ConfigProvider, theme as antdTheme } from 'ant-design-vue';
import zhCN from 'ant-design-vue/es/locale/zh_CN';
import enUS from 'ant-design-vue/es/locale/en_US';
import { useAppStore } from '@/stores/app';

const appStore = useAppStore();

/** antd 内建文案语言（分页 / 空状态 / 确认框默认按钮等） */
const antdLocale = computed(() => (appStore.language === 'en' ? enUS : zhCN));

/**
 * antd 主题 token：与 `style.css` 设计令牌对齐。
 *
 * 明暗切换使用 `algorithm`；浅色下补充与设计令牌一致的文本 / 边框 / 表格覆盖色，
 * 暗色下不再硬编码这些颜色（交由 `darkAlgorithm`），避免出现浅底深字残留。
 */
const themeConfig = computed(() => {
  const dark = appStore.isDark;
  return {
    algorithm: dark ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
    token: {
      colorPrimary: dark ? '#4096ff' : '#1677ff',
      colorInfo: dark ? '#4096ff' : '#1677ff',
      colorSuccess: '#52c41a',
      colorWarning: '#faad14',
      colorError: dark ? '#ff4d4f' : '#f5222d',
      ...(dark
        ? {}
        : {
            colorText: '#1f2329',
            colorTextSecondary: '#6b7280',
            colorTextTertiary: '#9ca3af',
            colorBorder: '#e5e7eb',
            colorBorderSecondary: '#eef0f3',
          }),
      borderRadius: 8,
      fontSize: 13,
      controlHeight: 32,
      boxShadowTertiary: dark
        ? '0 1px 2px rgba(0,0,0,0.4)'
        : '0 1px 2px rgba(0, 0, 0, 0.04), 0 2px 8px -2px rgba(0, 0, 0, 0.06)',
    },
    components: {
      Table: dark
        ? {}
        : {
            headerBg: '#fafbfc',
            headerColor: '#1f2329',
            rowHoverBg: '#f5f9ff',
            borderColor: '#eef0f3',
          },
      Tag: dark ? {} : { defaultBg: '#f2f3f5' },
    },
  };
});
</script>

<template>
  <ConfigProvider :locale="antdLocale" :theme="themeConfig">
    <!-- 沉浸式嵌入（VOS 多入口模式）：平台导航菜单以同域 iframe 将本应用每个路由
         作为独立入口加载（见 apps/com.ictrek.image-hub/src/routers.yml），
         因此本应用不提供内置侧边栏/导航，仅渲染当前路由组件。 -->
    <div class="app-shell">
      <router-view />
    </div>
  </ConfigProvider>
</template>

<style scoped>
.app-shell {
  min-height: 100vh;
  padding: var(--ih-space-lg) var(--ih-space-xl) var(--ih-space-xl);
  background: var(--ih-bg);
}
</style>