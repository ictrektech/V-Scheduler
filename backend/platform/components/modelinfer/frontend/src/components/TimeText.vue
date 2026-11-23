<script setup lang="ts">
/** 时间展示：本地化可读时间 + 悬浮提示（完整时间 + 相对时间）。 */
import { computed } from 'vue';
import { formatTime, formatTimeTooltip } from '@/utils/format';

const props = defineProps<{ value?: string | null }>();

const display = computed(() => formatTime(props.value));
const tooltip = computed(() => formatTimeTooltip(props.value));
</script>

<template>
  <a-tooltip :title="tooltip">
    <span class="mi-time">{{ display }}</span>
  </a-tooltip>
</template>

<style scoped>
.mi-time {
  color: var(--mi-text-secondary);
  white-space: nowrap;
}
</style>