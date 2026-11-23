<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import {
  DEFAULT_TIMEOUT_MINUTES,
  MIN_TIMEOUT_MINUTES,
  TIMEOUT_PRESETS,
  TIMEOUT_UNITS,
  UNLIMITED_TIMEOUT_MINUTES,
  isValidTimeoutMinutes,
  splitTimeout,
} from '@/utils/task';

/**
 * 单次传输超时选择：
 * - 预设：30 分钟 / 1 / 2 / 12 / 24 小时 / 无限制（`0`）
 * - 自定义：数值 + 单位（分钟 / 小时）→ 换算为分钟（**无上限**）
 * - `v-model` 值为分钟数（number；`0` = 无限制）
 * - 超时按**单个标签的单次传输**计时（同步多标签时每个标签各自计时）
 * - 展示文案随语言切换（预设 labelKey 由 `t()` 渲染）
 */
const { t } = useI18n();

const props = defineProps<{
  modelValue: number;
}>();

const emit = defineEmits<{
  'update:modelValue': [number];
}>();

const CUSTOM = '__custom__';

const choice = ref<string>(String(DEFAULT_TIMEOUT_MINUTES));
const customValue = ref<number>(DEFAULT_TIMEOUT_MINUTES);
const customUnit = ref<number>(1);

const options = computed(() => [
  ...TIMEOUT_PRESETS.map((p) => ({ value: String(p.value), label: t(p.labelKey) })),
  { value: CUSTOM, label: t('timeout.custom') },
]);

const unitOptions = computed(() =>
  TIMEOUT_UNITS.map((u) => ({ value: u.value, label: t(u.labelKey) })),
);

/** 是否为预设值（缺失表示需自定义） */
function isPreset(value: number): boolean {
  return TIMEOUT_PRESETS.some((p) => p.value === value);
}

watch(
  () => props.modelValue,
  (value) => {
    if (isPreset(value)) {
      choice.value = String(value);
      return;
    }
    choice.value = CUSTOM;
    const safe = isValidTimeoutMinutes(value) ? value : DEFAULT_TIMEOUT_MINUTES;
    const { value: v, unit } = splitTimeout(safe);
    customValue.value = v;
    customUnit.value = unit;
  },
  { immediate: true },
);

function onChoice(value: unknown): void {
  const v = String(value);
  choice.value = v;
  if (v === CUSTOM) {
    emit('update:modelValue', toMinutes());
  } else {
    emit('update:modelValue', Number(v));
  }
}

/** 自定义输入 → 分钟（单位换算） */
function toMinutes(): number {
  return Math.floor(customValue.value * customUnit.value);
}

watch([customValue, customUnit], () => {
  if (choice.value !== CUSTOM) return;
  if (isValidTimeoutMinutes(toMinutes())) {
    emit('update:modelValue', toMinutes());
  }
});

const customMinutes = computed(() => toMinutes());
const invalid = computed(
  () => choice.value === CUSTOM && !isValidTimeoutMinutes(customMinutes.value),
);

const isUnlimited = computed(() => props.modelValue === UNLIMITED_TIMEOUT_MINUTES);
</script>

<template>
  <div class="timeout-select">
    <div class="timeout-row">
      <a-select :value="choice" :options="options" style="width: 160px" @change="onChoice" />
      <template v-if="choice === CUSTOM">
        <a-input-number
          v-model:value="customValue"
          :min="1"
          :step="1"
          :precision="0"
          style="width: 120px; margin-left: 8px"
        />
        <a-select
          v-model:value="customUnit"
          :options="unitOptions"
          style="width: 100px; margin-left: 4px"
        />
      </template>
    </div>
    <div v-if="invalid" class="timeout-hint error">
      {{ t('timeout.invalid', { min: MIN_TIMEOUT_MINUTES }) }}
    </div>
    <div v-else-if="isUnlimited" class="timeout-hint">
      {{ t('timeout.unlimitedHint') }}
    </div>
    <div v-else class="timeout-hint">{{ t('timeout.hint') }}</div>
  </div>
</template>

<style scoped>
.timeout-row {
  display: flex;
  align-items: center;
}
.timeout-hint {
  margin-top: 6px;
  font-size: 12px;
  color: var(--ih-text-secondary);
  line-height: 1.6;
}
.timeout-hint.error {
  color: var(--ih-danger);
}
</style>