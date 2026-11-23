<script setup lang="ts">
/** 页头统计指标：数值 + 标签。 */
withDefaults(
  defineProps<{
    label: string;
    value: string | number;
    tone?: 'primary' | 'neutral';
  }>(),
  { tone: 'neutral' },
);
</script>

<template>
  <div class="mi-stat" :class="`mi-stat--${tone}`">
    <div class="mi-stat__value">{{ value }}</div>
    <div class="mi-stat__label">{{ label }}</div>
  </div>
</template>

<style scoped>
.mi-stat {
  min-width: 84px;
  padding: 8px 14px;
  border-radius: var(--mi-radius-md);
  background: var(--mi-surface);
  border: 1px solid var(--mi-border);
  line-height: 1.25;
}
.mi-stat__value {
  font-size: 18px;
  font-weight: 600;
  color: var(--mi-text);
  font-variant-numeric: tabular-nums;
}
.mi-stat__label {
  font-size: 12px;
  color: var(--mi-text-secondary);
  margin-top: 2px;
}
.mi-stat--primary .mi-stat__value {
  background: var(--mi-primary-gradient);
  -webkit-background-clip: text;
  background-clip: text;
  color: transparent;
}
</style>