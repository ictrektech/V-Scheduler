<script setup lang="ts">
/**
 * 状态标签：本地化语义 + 语义色圆点（不直接回显英文原始枚举值）。
 * 映射见 `utils/status.ts`（返回 i18n key），文案经 vue-i18n 渲染。
 */
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { statusMeta, type StatusKind } from '@/utils/status';

const props = defineProps<{
  status?: string | null;
  kind: StatusKind;
}>();

const { t } = useI18n();
const meta = computed(() => statusMeta(props.kind, props.status));
const label = computed(() => (meta.value.labelKey ? t(meta.value.labelKey) : meta.value.fallback));
</script>

<template>
  <span class="mi-status" :class="`mi-status--${meta.tone}`">
    <span class="mi-status__dot" />
    <span class="mi-status__label">{{ label }}</span>
  </span>
</template>

<style scoped>
.mi-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  height: 22px;
  padding: 0 9px;
  border-radius: 999px;
  font-size: 12px;
  line-height: 1;
  white-space: nowrap;
  border: 1px solid transparent;
}
.mi-status__dot {
  width: 6px;
  height: 6px;
  border-radius: 50%;
  background: currentColor;
  flex: none;
}
.mi-status--success {
  color: var(--mi-success);
  background: var(--mi-success-soft);
  border-color: var(--mi-success-border);
}
.mi-status--processing {
  color: var(--mi-processing);
  background: var(--mi-processing-soft);
  border-color: var(--mi-processing-border);
}
.mi-status--warning {
  color: var(--mi-warning);
  background: var(--mi-warning-soft);
  border-color: var(--mi-warning-border);
}
.mi-status--error {
  color: var(--mi-error);
  background: var(--mi-error-soft);
  border-color: var(--mi-error-border);
}
.mi-status--neutral {
  color: var(--mi-neutral);
  background: var(--mi-neutral-soft);
  border-color: var(--mi-neutral-border);
}
</style>