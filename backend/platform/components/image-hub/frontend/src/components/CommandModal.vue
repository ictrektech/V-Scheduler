<script setup lang="ts">
import { computed } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import type { CommandStep } from '@/utils/commands';
import { buildPlainText, commandStepTitle } from '@/utils/commands';

/**
 * 通用命令弹窗：推荐一键命令（经 image-hub.sh）+ 手动分步命令。
 * 步骤标题为 i18n key，命令本体语言中立。
 */
const { t } = useI18n();

const props = defineProps<{
  open: boolean;
  title: string;
  description?: string;
  steps: CommandStep[];
  /** 推荐的一键命令（经 image-hub.sh）；提供时置顶展示 */
  oneLiner?: string;
  /** 一键命令的说明文案 */
  oneLinerHint?: string;
  /** 「先下载再执行」的等价命令（可选） */
  downloadLine?: string;
}>();

const emit = defineEmits<{ 'update:open': [boolean] }>();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

/** 供纯函数使用的翻译闭包（保持 utils 零 i18n 依赖） */
function translate(key: string, params?: Record<string, unknown>): string {
  return t(key, params as Record<string, never>);
}

async function copy(text: string): Promise<void> {
  try {
    await navigator.clipboard.writeText(text);
    message.success(t('common.copied'));
  } catch {
    message.error(t('common.copyFailed'));
  }
}

function copyAll(): void {
  void copy(buildPlainText(props.steps, translate));
}
</script>

<template>
  <a-modal
    :open="visible"
    :title="title"
    width="780px"
    :footer="null"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <p v-if="description" class="ih-muted" style="margin-top: 0">{{ description }}</p>

    <!-- 推荐一键命令 -->
    <div v-if="oneLiner" class="oneliner">
      <div class="step-head">
        <span class="step-title">{{ oneLinerHint || t('detail.download.oneLinerHint') }}</span>
        <a-button type="link" size="small" @click="copy(oneLiner)">{{ t('common.copy') }}</a-button>
      </div>
      <pre class="ih-cmd">{{ oneLiner }}</pre>
      <div v-if="downloadLine" class="ih-muted oneliner-alt">
        {{ t('guide.downloadAlt') }}<code>{{ downloadLine }}</code>
      </div>
    </div>

    <a-divider v-if="oneLiner && steps.length" style="margin: 12px 0">{{ t('guide.mode.manual') }}</a-divider>

    <div v-for="(step, index) in steps" :key="index" class="step">
      <div class="step-head">
        <span class="step-title">{{ index + 1 }}. {{ commandStepTitle(step, translate) }}</span>
        <a-button type="link" size="small" @click="copy(step.command)">{{ t('common.copy') }}</a-button>
      </div>
      <pre class="ih-cmd">{{ step.command }}</pre>
    </div>
    <div style="text-align: right; margin-top: 12px">
      <a-button @click="copyAll">{{ t('guide.copyAll') }}</a-button>
    </div>
  </a-modal>
</template>

<style scoped>
.oneliner {
  margin-bottom: 4px;
}
.step {
  margin-bottom: 12px;
}
.step-head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 4px;
}
.step-title {
  font-size: 12.5px;
  color: var(--ih-text-secondary);
}
.oneliner-alt {
  margin-top: 4px;
  font-size: 12px;
  word-break: break-all;
}
code {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  background: var(--ih-surface-alt);
  border: 1px solid var(--ih-border);
  border-radius: 4px;
  padding: 0 4px;
  font-size: 12px;
  word-break: break-all;
}
</style>