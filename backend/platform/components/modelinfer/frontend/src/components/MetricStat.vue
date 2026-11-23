<script setup lang="ts">
/**
 * 指标概览 stat 卡（Grafana stat 风格）：标题 + 大数字 + 迷你 sparkline（纯 SVG，无依赖）。
 */
import { computed } from 'vue';
import { formatMetricValue, sparklinePoints } from '@/utils/metrics';

const props = defineProps<{
  title: string;
  value: number | null;
  unit: string;
  points: [number, number][];
  note?: string;
}>();

const display = computed(() => formatMetricValue(props.value, props.unit));
const spark = computed(() => sparklinePoints(props.points));
</script>

<template>
  <div class="mi-stat">
    <div class="mi-stat__head">
      <span class="mi-stat__title">{{ title }}</span>
      <span v-if="note" class="mi-stat__note">{{ note }}</span>
    </div>
    <div class="mi-stat__value">{{ display }}</div>
    <svg v-if="spark" class="mi-stat__spark" viewBox="0 0 100 28" preserveAspectRatio="none">
      <polyline :points="spark" fill="none" stroke="currentColor" stroke-width="1.5" />
    </svg>
    <div v-else class="mi-stat__spark mi-stat__spark--empty" />
  </div>
</template>

<style scoped>
.mi-stat {
  border: 1px solid var(--mi-border);
  border-radius: 8px;
  background: var(--mi-surface);
  padding: 10px 12px;
  min-width: 0;
  color: var(--mi-primary);
}
.mi-stat__head {
  display: flex;
  align-items: baseline;
  justify-content: space-between;
  gap: 8px;
}
.mi-stat__title {
  font-size: var(--mi-font-hint);
  color: var(--mi-text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.mi-stat__note {
  font-size: var(--mi-font-hint);
  color: var(--mi-text-secondary);
  white-space: nowrap;
}
.mi-stat__value {
  margin-top: 2px;
  font-size: 22px;
  font-weight: 600;
  color: var(--mi-text);
  font-variant-numeric: tabular-nums;
  line-height: 1.3;
}
.mi-stat__spark {
  display: block;
  width: 100%;
  height: 28px;
  margin-top: 4px;
  opacity: 0.75;
}
.mi-stat__spark--empty {
  height: 28px;
}
</style>