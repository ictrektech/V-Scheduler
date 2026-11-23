<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import {
  DEFAULT_RETRY_TIMES,
  RETRY_PRESETS,
  isValidRetryTimes,
  retryHintParts,
} from '@/utils/task';

/**
 * 传输失败重试次数选择（1~5，默认 2）：
 * - `v-model` 值为重试次数（失败后最多再试 N 次 → 最多共 N+1 次尝试）
 * - 退避序列 5/10/20/40/60 秒；退避等待期间点击「停止」立即生效
 * - 文案随语言切换（预设 labelKey + 结构化退避参数）
 */
const { t } = useI18n();

const props = defineProps<{
  modelValue: number;
}>();

const emit = defineEmits<{
  'update:modelValue': [number];
}>();

const options = computed(() =>
  RETRY_PRESETS.map((p) => ({ value: p.value, label: t(p.labelKey, { count: p.value }) })),
);

const current = computed(() =>
  isValidRetryTimes(props.modelValue) ? props.modelValue : DEFAULT_RETRY_TIMES,
);

const hint = computed(() => {
  const parts = retryHintParts(current.value);
  return t('retry.hint', { attempts: parts.attempts, backoffs: parts.backoffs });
});

function onChange(value: unknown): void {
  emit('update:modelValue', Number(value));
}
</script>

<template>
  <div class="retry-select">
    <a-select
      :value="current"
      :options="options"
      style="width: 160px"
      @change="onChange"
    />
    <div class="retry-hint">{{ hint }}</div>
  </div>
</template>

<style scoped>
.retry-hint {
  margin-top: 6px;
  font-size: 12px;
  color: var(--ih-text-secondary);
  line-height: 1.6;
}
</style>