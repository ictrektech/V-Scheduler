<script setup lang="ts">
/**
 * 统一页面壳：页头（标题 + 说明 + 操作区）+ 内容区。
 * 沉浸式多入口下每个 iframe 页面均使用本壳，保证观感统一。
 */
defineProps<{
  title: string;
  subtitle?: string;
  loading?: boolean;
}>();
</script>

<template>
  <div class="ih-page">
    <header class="ih-page__head">
      <div class="ih-page__titles">
        <h1 class="ih-page__title">{{ title }}</h1>
        <p v-if="subtitle" class="ih-page__subtitle">{{ subtitle }}</p>
      </div>
      <div v-if="$slots.actions" class="ih-page__actions">
        <slot name="actions" />
      </div>
    </header>

    <a-spin :spinning="!!loading">
      <section class="ih-page__body">
        <slot />
      </section>
    </a-spin>
  </div>
</template>

<style scoped>
.ih-page {
  max-width: 1360px;
  margin: 0 auto;
}
.ih-page__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--ih-space-lg);
  margin-bottom: var(--ih-space-lg);
}
.ih-page__title {
  margin: 0;
  font-size: 20px;
  font-weight: 600;
  color: var(--ih-text);
  line-height: 1.4;
}
.ih-page__subtitle {
  margin: var(--ih-space-xs) 0 0;
  font-size: 13px;
  color: var(--ih-text-secondary);
  line-height: 1.5;
}
.ih-page__actions {
  flex: none;
  display: flex;
  align-items: center;
  gap: var(--ih-space-sm);
}
.ih-page__body {
  min-height: 120px;
}
</style>