<script setup lang="ts">
/**
 * JSON 展示块：对象键稳定排序 + 折叠 + 复制，用于「结构化数据」展示。
 */
import { computed, ref } from 'vue';
import { message } from 'ant-design-vue';
import { useI18n } from 'vue-i18n';
import { CopyOutlined, DownOutlined } from '@ant-design/icons-vue';
import { copyToClipboard } from '@/utils/clipboard';

const { t } = useI18n();

const props = withDefaults(defineProps<{ data: unknown; title?: string; defaultOpen?: boolean }>(), {
  title: '',
  defaultOpen: true,
});

const open = ref(props.defaultOpen);

/** 递归按 key 排序，保证同一份数据展示稳定 */
function stable(v: unknown): unknown {
  if (Array.isArray(v)) return v.map(stable);
  if (v && typeof v === 'object') {
    const obj = v as Record<string, unknown>;
    return Object.keys(obj)
      .sort()
      .reduce<Record<string, unknown>>((acc, k) => {
        acc[k] = stable(obj[k]);
        return acc;
      }, {});
  }
  return v;
}

const text = computed(() => {
  try {
    return JSON.stringify(stable(props.data), null, 2);
  } catch {
    return String(props.data);
  }
});

async function copy() {
  const ok = await copyToClipboard(text.value);
  if (ok) message.success(t('common.copied'));
  else message.error(t('common.copyFailed'));
}
</script>

<template>
  <div class="mi-json">
    <div class="mi-json__head" @click="open = !open">
      <DownOutlined class="mi-json__caret" :class="{ 'is-open': open }" />
      <span class="mi-json__title">{{ title }}</span>
      <a class="mi-json__copy" @click.stop="copy"><CopyOutlined /></a>
    </div>
    <pre v-if="open" class="mi-json__pre">{{ text }}</pre>
  </div>
</template>

<style scoped>
.mi-json {
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-md);
  overflow: hidden;
  background: var(--mi-surface);
}
.mi-json__head {
  display: flex;
  align-items: center;
  gap: 8px;
  padding: 7px 10px;
  cursor: pointer;
  background: var(--mi-surface-2);
  border-bottom: 1px solid var(--mi-border);
}
.mi-json__caret {
  font-size: 11px;
  color: var(--mi-text-tertiary);
  transition: transform 0.15s;
}
.mi-json__caret.is-open {
  transform: rotate(0deg);
}
.mi-json__caret:not(.is-open) {
  transform: rotate(-90deg);
}
.mi-json__title {
  flex: 1;
  font-size: 13px;
  font-weight: 600;
  color: var(--mi-text);
  word-break: break-all;
}
.mi-json__copy {
  color: var(--mi-text-tertiary);
  font-size: 13px;
}
.mi-json__pre {
  margin: 0;
  padding: 10px 12px;
  background: var(--mi-surface-2);
  color: var(--mi-text);
  font-size: 12px;
  line-height: 1.55;
  max-height: 420px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-all;
  font-family: 'SFMono-Regular', 'JetBrains Mono', Menlo, Consolas, monospace;
}
</style>