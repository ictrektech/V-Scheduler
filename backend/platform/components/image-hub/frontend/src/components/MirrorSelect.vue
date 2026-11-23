<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import type { MirrorOption } from '@/api/imageHub';
import { hasTranslation } from '@/i18n';
import { filterMirrorsByRegistry, isValidMirrorHost, normalizeRegistry } from '@/utils/mirror';

/**
 * 镜像源（mirror）选择器：
 * - 「不使用镜像源」（默认）/ 该上游的候选镜像源 / 「手动输入…」
 * - `v-model` 取值约定：
 *   - `null` → 不使用镜像源（父组件用 `null` 重置本组件选择态）
 *   - `''`   → 处于「手动输入」态但尚未填写（等价于不使用）
 *   - `host` → 使用该镜像源主机名
 * - 仅改变实际拉取来源，落地仓库名保持上游完整镜像名（由父组件预览）
 * - 提供方名称按 `provider` 稳定标识本地化（未知 provider 回退后端 `name`）
 */
const { t } = useI18n();

const props = defineProps<{
  modelValue: string | null;
  /** 完整镜像源目录（后端 `GET /api/registry/mirrors`） */
  catalog: MirrorOption[];
  /** 当前镜像归一后的上游 registry（用于过滤候选） */
  upstreamRegistry: string;
}>();

const emit = defineEmits<{
  'update:modelValue': [string | null];
}>();

const NONE = '__none__';
const CUSTOM = '__custom__';

/** provider 稳定 id -> i18n key 段（数字开头的 id 需转义为合法 key 段） */
const PROVIDER_KEY: Record<string, string> = {
  daocloud: 'daocloud',
  '1panel': 'panel1',
  '1ms': 'ms1',
  xuanyuan: 'xuanyuan',
  dockerproxy: 'dockerproxy',
};

const choice = ref<string>(NONE);
const customValue = ref('');

const candidates = computed(() => filterMirrorsByRegistry(props.catalog, props.upstreamRegistry));

/** 提供方展示名（本地化优先，未知 provider 回退后端 name） */
function providerLabel(mirror: MirrorOption): string {
  const provider = mirror.provider ?? '';
  const segment = PROVIDER_KEY[provider] ?? provider;
  const key = `mirror.providers.${segment}`;
  return segment && hasTranslation(key) ? t(key) : mirror.name;
}

const options = computed(() => [
  { value: NONE, label: t('mirror.none') },
  ...candidates.value.map((m) => ({
    value: m.host,
    label: t('mirror.option', { host: m.host, provider: providerLabel(m) }),
  })),
  { value: CUSTOM, label: t('mirror.custom') },
]);

/** 外部值 → 内部选择态（`null` 为外部重置，`''` 为手动输入态） */
watch(
  () => props.modelValue,
  (value) => {
    if (value === null) {
      choice.value = NONE;
      customValue.value = '';
      return;
    }
    const v = normalizeRegistry(value);
    if (!v) {
      choice.value = CUSTOM;
      return;
    }
    if (choice.value !== CUSTOM && candidates.value.some((c) => normalizeRegistry(c.host) === v)) {
      choice.value = v;
      return;
    }
    choice.value = CUSTOM;
    if (value !== customValue.value) customValue.value = value;
  },
  { immediate: true },
);

/** 候选变化（上游 registry 变了）：已选主机名不再是候选时回落「不使用」 */
watch(candidates, (list) => {
  if (choice.value === NONE || choice.value === CUSTOM) return;
  if (!list.some((c) => normalizeRegistry(c.host) === normalizeRegistry(choice.value))) {
    choice.value = NONE;
    emit('update:modelValue', null);
  }
});

function onChoice(value: string): void {
  choice.value = value;
  if (value === NONE) {
    emit('update:modelValue', null);
  } else if (value === CUSTOM) {
    // 空输入用 `''` 表示「手动输入态但未填写」，避免被父组件当作重置
    emit('update:modelValue', customValue.value.trim());
  } else {
    emit('update:modelValue', value);
  }
}

/** 手动输入变化 → 同步外部值（保持手动输入态） */
watch(customValue, (value) => {
  if (choice.value !== CUSTOM) return;
  emit('update:modelValue', value.trim());
});

/** a-select change 事件适配（值可能为任意 SelectValue） */
function onChoiceChange(value: unknown): void {
  onChoice(String(value));
}

const customInvalid = computed(
  () => choice.value === CUSTOM && customValue.value.trim() !== '' && !isValidMirrorHost(customValue.value),
);
</script>

<template>
  <div class="mirror-select">
    <a-select
      :value="choice"
      :options="options"
      style="width: 340px"
      @change="onChoiceChange"
    />
    <a-input
      v-if="choice === CUSTOM"
      v-model:value="customValue"
      :placeholder="t('mirror.customPlaceholder')"
      style="width: 340px; margin-top: 8px"
      allow-clear
    />
    <div v-if="customInvalid" class="mirror-hint error">{{ t('mirror.invalid') }}</div>
    <div v-else class="mirror-hint">
      {{ t('mirror.hint1') }}<b>{{ t('mirror.hintStrong') }}</b>{{ t('mirror.hint2') }}
    </div>
  </div>
</template>

<style scoped>
.mirror-select {
  display: flex;
  flex-direction: column;
}
.mirror-hint {
  margin-top: 6px;
  font-size: 12px;
  color: var(--ih-text-secondary);
  line-height: 1.6;
}
.mirror-hint.error {
  color: var(--ih-danger);
}
</style>