<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import zhCN from 'ant-design-vue/es/locale/zh_CN';
import enUS from 'ant-design-vue/es/locale/en_US';
import { theme as antdTheme } from 'ant-design-vue';
import { useAppStore } from '@/stores/app';
import { getSchedulerRuntime } from '@scheduler-auth';

const workspace = Boolean(getSchedulerRuntime());
const sessionRevision = ref(0);
function refreshView() { sessionRevision.value += 1; }
onMounted(() => {
  if (workspace) window.addEventListener('scheduler:session-restored', refreshView);
});
onBeforeUnmount(() => window.removeEventListener('scheduler:session-restored', refreshView));

const appStore = useAppStore();

/** antd 组件内建文案语言（分页 / 空状态 / 确认框默认按钮等） */
const antdLocale = computed(() => (appStore.language === 'en' ? enUS : zhCN));

/**
 * antd 主题 token：与 `style.css` 设计 token 对齐（科技蓝紫主色 + 圆角 + 字号）。
 * 浅色 / 暗黑切换 `algorithm`，并按主题给出 token 与组件覆盖色，避免暗色下出现浅色残留。
 */
const themeConfig = computed(() => {
  const dark = appStore.isDark;
  return {
    algorithm: dark ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
    token: {
      colorPrimary: workspace ? '#087f8a' : '#4f6bff',
      colorInfo: workspace ? '#087f8a' : '#4f6bff',
      colorSuccess: '#16a34a',
      colorWarning: '#d97706',
      colorError: '#dc2626',
      ...(dark
        ? {}
        : {
            colorText: workspace ? '#10213d' : '#1f2329',
            colorTextSecondary: workspace ? '#60758d' : '#646a73',
            colorTextTertiary: '#8f959e',
            colorBorder: '#e5e6eb',
            colorBorderSecondary: '#eef0f3',
          }),
      borderRadius: 8,
      borderRadiusLG: 12,
      fontSize: 13,
      controlHeight: 34,
      boxShadowTertiary: dark
        ? '0 1px 2px rgba(0,0,0,0.4)'
        : '0 1px 2px rgba(15,23,42,0.04), 0 1px 3px rgba(15,23,42,0.06)',
    },
    components: {
      Table: dark
        ? {}
        : {
            headerBg: workspace ? '#f2f5f9' : '#fafbfc',
            headerColor: '#1f2329',
            rowHoverBg: workspace ? '#f0f8f8' : '#f7f9ff',
            borderColor: '#eef0f3',
          },
      Card: {
        headerFontSize: 14,
      },
      Tag: dark ? {} : { defaultBg: '#f2f3f5' },
    },
  };
});
</script>

<template>
  <a-config-provider :locale="antdLocale" :theme="themeConfig">
    <!-- 沉浸式嵌入（VOS 多入口模式）：平台导航菜单以同域 iframe 将本应用每个路由
         作为独立入口加载（见 apps/com.ictrek.modelinfer/src/routers.yml），
         本应用不提供内置侧边栏/导航，仅渲染当前路由组件。 -->
    <div class="app-shell" :class="{'app-shell--workspace':workspace}">
      <router-view :key="sessionRevision" />
    </div>
  </a-config-provider>
</template>

<style scoped>
.app-shell {
  min-height: 100vh;
  padding: 20px 24px 24px;
  background:
    radial-gradient(1200px 320px at 8% -12%, rgba(79, 107, 255, 0.07), transparent 60%),
    radial-gradient(900px 260px at 96% -8%, rgba(124, 92, 255, 0.06), transparent 55%),
    var(--mi-bg);
}
.app-shell--workspace {
  min-height: 0;
  padding: 0 0 12px;
  background: transparent;
}
</style>