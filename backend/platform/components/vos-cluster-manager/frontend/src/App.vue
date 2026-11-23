<template>
  <a-config-provider :locale="zhCN">
    <!-- 沉浸式嵌入（VOS 多入口模式）：平台导航菜单以同域 iframe 将本应用每个路由
         作为独立入口加载（见 apps/com.ictrek.vos-cluster-manager/src/routers.yml），
         因此本应用不提供内置侧边栏/导航，仅渲染当前路由组件并保留页面留白。 -->
    <div class="app-shell">
      <router-view />
    </div>
  </a-config-provider>
</template>

<script setup lang="ts">
import zhCN from 'ant-design-vue/es/locale/zh_CN';
</script>

<style scoped>
.app-shell {
  min-height: 100vh;
  padding: 16px 24px 24px;
  background: #f5f5f5;
}
</style>