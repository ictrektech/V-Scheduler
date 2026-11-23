<script setup lang="ts">
/**
 * 终端风格文本块：深色等宽展示脚本 stdout/stderr / 容器日志，
 * 头部提供「复制 / 下载」与可选操作插槽（如暂停 / 清空 / 自动滚动）。
 */
import { computed, ref } from 'vue';
import { message } from 'ant-design-vue';
import { useI18n } from 'vue-i18n';
import { CopyOutlined, DownloadOutlined } from '@ant-design/icons-vue';
import { copyToClipboard } from '@/utils/clipboard';

const { t } = useI18n();

const props = withDefaults(
  defineProps<{
    text?: string | null;
    label?: string;
    tone?: 'default' | 'error';
    maxHeight?: string;
    empty?: string;
    /** 下载文件名（不含扩展名） */
    filename?: string;
  }>(),
  { tone: 'default', maxHeight: '360px', empty: undefined, label: '', filename: 'output' },
);

const emptyText = computed(() => props.empty ?? t('common.empty'));
const preRef = ref<HTMLElement | null>(null);

/** 滚动到底部（供流式日志自动跟随） */
function scrollToEnd() {
  const el = preRef.value;
  if (el) el.scrollTop = el.scrollHeight;
}
defineExpose({ scrollToEnd });

async function copy() {
  const ok = await copyToClipboard(props.text ?? '');
  if (ok) message.success(t('common.copied'));
  else message.error(t('common.copyFailed'));
}

function download() {
  const text = props.text ?? '';
  if (!text) return;
  const blob = new Blob([text], { type: 'text/plain;charset=utf-8' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = `${props.filename}.txt`;
  document.body.appendChild(a);
  a.click();
  document.body.removeChild(a);
  URL.revokeObjectURL(url);
}
</script>

<template>
  <div class="mi-term" :class="`mi-term--${tone}`">
    <div class="mi-term__head">
      <span class="mi-term__label">{{ label }}</span>
      <span class="mi-term__actions">
        <slot name="actions" />
        <a-tooltip :title="t('common.copy')">
          <a class="mi-term__btn" @click="copy"><CopyOutlined /></a>
        </a-tooltip>
        <a-tooltip :title="t('common.download')">
          <a class="mi-term__btn" @click="download"><DownloadOutlined /></a>
        </a-tooltip>
      </span>
    </div>
    <pre ref="preRef" class="mi-term__pre" :style="{ maxHeight }">{{ text || emptyText }}</pre>
  </div>
</template>

<style scoped>
.mi-term {
  border-radius: var(--mi-radius-md);
  overflow: hidden;
  background: #1f1f23;
}
.mi-term--error {
  background: #2b1414;
}
.mi-term__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 6px 10px;
  background: rgba(255, 255, 255, 0.06);
  border-bottom: 1px solid rgba(255, 255, 255, 0.08);
}
.mi-term__label {
  font-size: 12px;
  color: #b8bcc4;
  font-family: 'SFMono-Regular', 'JetBrains Mono', Menlo, Consolas, monospace;
}
.mi-term__actions {
  display: inline-flex;
  align-items: center;
  gap: 10px;
}
.mi-term__btn {
  color: #b8bcc4;
  font-size: 13px;
}
.mi-term__btn:hover {
  color: #fff;
}
.mi-term__pre {
  margin: 0;
  padding: 10px 12px;
  color: #e6e6e6;
  font-size: 12px;
  line-height: 1.55;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-all;
  font-family: 'SFMono-Regular', 'JetBrains Mono', Menlo, Consolas, monospace;
}
.mi-term--error .mi-term__pre {
  color: #ffb3b3;
}
</style>