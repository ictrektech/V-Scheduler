<script setup lang="ts">
/** 可复制文本：等宽展示 id / 权重 / 路径 / 端点，一键复制（含非安全上下文兜底）。 */
import { message } from 'ant-design-vue';
import { useI18n } from 'vue-i18n';
import { CopyOutlined } from '@ant-design/icons-vue';
import { copyToClipboard } from '@/utils/clipboard';

const { t } = useI18n();

const props = withDefaults(
  defineProps<{ text?: string | null; mono?: boolean; secondary?: boolean }>(),
  { mono: true, secondary: false },
);

async function copy() {
  const text = props.text ?? '';
  if (!text) return;
  const ok = await copyToClipboard(text);
  if (ok) message.success(t('common.copied'));
  else message.error(t('common.copyFailed'));
}
</script>

<template>
  <span class="mi-copy" :class="{ 'mi-copy--secondary': secondary }">
    <span class="mi-copy__text" :class="{ 'mi-mono': mono }">{{ text || '-' }}</span>
    <a-tooltip v-if="text" :title="t('common.copy')">
      <a class="mi-copy__btn" @click.stop="copy"><CopyOutlined /></a>
    </a-tooltip>
  </span>
</template>

<style scoped>
.mi-copy {
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  min-width: 0;
}
.mi-copy__text {
  word-break: break-all;
  color: var(--mi-text);
}
.mi-copy--secondary .mi-copy__text {
  color: var(--mi-text-secondary);
}
.mi-copy__btn {
  color: var(--mi-text-tertiary);
  font-size: 12px;
  flex: none;
  opacity: 0;
  transition: opacity 0.15s;
}
.mi-copy:hover .mi-copy__btn {
  opacity: 1;
}
</style>