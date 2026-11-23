<template>
  <a-config-provider :locale="locale" :theme="workspaceTheme">
    <!-- 沉浸式嵌入（VOS 多入口模式）：平台导航菜单以同域 iframe 将本应用每个路由
         作为独立入口加载（见 apps/com.ictrek.vos-cluster-manager/src/routers.yml），
         因此本应用不提供内置侧边栏/导航，仅渲染当前路由组件并保留页面留白。 -->
    <div class="app-shell" :class="{'app-shell--workspace':runtime}">
      <router-view />
    </div>
  </a-config-provider>
</template>

<script setup lang="ts">
import zhCN from 'ant-design-vue/es/locale/zh_CN';
import enUS from 'ant-design-vue/es/locale/en_US';
import { onMounted, onUnmounted, shallowRef } from 'vue';
import { getSchedulerRuntime } from '@scheduler-auth';

const runtime = getSchedulerRuntime();
const locale = shallowRef(zhCN);
const workspaceTheme = runtime ? { token: {
  colorPrimary:'#087f8a', colorInfo:'#087f8a', colorText:'#10213d',
  colorTextSecondary:'#60758d', colorBorder:'#d9e3ee', borderRadius:8,
} } : undefined;
function syncLocale() {
  locale.value = runtime?.locale() === 'en' ? enUS : zhCN;
}
if (runtime) document.documentElement.dataset.schedulerWorkspace = '';
onMounted(() => { syncLocale(); window.addEventListener('scheduler:locale', syncLocale); });
onUnmounted(() => window.removeEventListener('scheduler:locale', syncLocale));
</script>

<style scoped>
.app-shell {
  min-height: 100vh;
  padding: 16px 24px 24px;
  background: #f5f5f5;
}
.app-shell--workspace {
  min-height: 0;
  padding: 0 0 12px;
  background: transparent;
}
</style>