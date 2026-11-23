<script setup lang="ts">
/** 统一页面壳：页头（标题 + 说明 + 操作区）+ 可选统计条 + 内容区。 */
defineProps<{
  title: string;
  subtitle?: string;
  loading?: boolean;
  hideHeading?: boolean;
}>();
</script>

<template>
  <div class="mi-page">
    <header v-if="!hideHeading || $slots.actions" class="mi-page__head">
      <div v-if="!hideHeading" class="mi-page__titles">
        <h1 class="mi-page__title">
          <span class="mi-page__mark" aria-hidden="true" />
          {{ title }}
        </h1>
        <p v-if="subtitle" class="mi-page__subtitle">{{ subtitle }}</p>
      </div>
      <div v-if="$slots.actions" class="mi-page__actions" :class="{'mi-page__actions--alone':hideHeading}">
        <slot name="actions" />
      </div>
    </header>

    <div v-if="$slots.stats" class="mi-page__stats">
      <slot name="stats" />
    </div>

    <a-spin :spinning="!!loading">
      <section class="mi-page__body">
        <slot />
      </section>
    </a-spin>
  </div>
</template>

<style scoped>
.mi-page {
  max-width: 1360px;
  margin: 0 auto;
}
.mi-page__head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 16px;
  margin-bottom: 16px;
}
.mi-page__titles {
  min-width: 0;
}
.mi-page__title {
  display: flex;
  align-items: center;
  gap: 10px;
  margin: 0;
  font-size: var(--mi-font-page-title);
  font-weight: 600;
  color: var(--mi-text);
  line-height: 1.3;
}
.mi-page__mark {
  display: inline-block;
  width: 4px;
  height: 20px;
  border-radius: 2px;
  background: var(--mi-primary-gradient);
  flex: none;
}
.mi-page__subtitle {
  margin: 6px 0 0 14px;
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-body);
}
.mi-page__actions {
  display: flex;
  gap: 8px;
  flex-shrink: 0;
}
.mi-page__actions--alone {
  margin-inline-start: auto;
}
.mi-page__stats {
  display: flex;
  flex-wrap: wrap;
  gap: 12px;
  margin-bottom: 16px;
}
.mi-page__body {
  background: var(--mi-surface);
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-lg);
  box-shadow: var(--mi-shadow-sm);
  padding: 16px;
}
</style>