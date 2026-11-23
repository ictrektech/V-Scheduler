<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import { cloneImage, renameImage } from '@/api/imageHub';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import RetrySelect from '@/components/RetrySelect.vue';
import TimeoutSelect from '@/components/TimeoutSelect.vue';
import { copyPreview, copyVerbKey, sourceRefParts, type CopyMode } from '@/utils/copy';
import { DEFAULT_RETRY_TIMES, DEFAULT_TIMEOUT_MINUTES } from '@/utils/task';

/**
 * 克隆 / 改名弹窗（克隆与改名共用）。
 *
 * - 克隆：复制到新名称，源保持不变；
 * - 改名：复制到新名称后删除源引用（单 tag 仅摘除该标签；整仓删除源仓库）；
 * - 新名称为用户输入，按镜像引用规则归一（缺省 docker.io、单段名补 library/）；
 * - 目标已存在 / 与源相同 / 名非法 → 不可提交（后端对应 20801 / 20809）。
 */
const { t } = useI18n();

const props = defineProps<{
  open: boolean;
  mode: CopyMode;
  /** 源仓库名（registry 中的名字） */
  repository: string;
  /** 源 tag；缺省 = 整仓 */
  tag?: string;
}>();

const emit = defineEmits<{
  'update:open': [boolean];
  started: [taskId: string];
}>();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

const targetRepository = ref('');
const targetTag = ref('');
const timeoutMinutes = ref<number>(DEFAULT_TIMEOUT_MINUTES);
const retryTimes = ref<number>(DEFAULT_RETRY_TIMES);
const submitting = ref(false);

const verb = computed(() => t(copyVerbKey(props.mode)));
const scope = computed(() => (props.tag ? 'tag' : 'repository'));
const introKey = computed(() =>
  props.mode === 'clone'
    ? scope.value === 'tag'
      ? 'copy.intro.cloneTag'
      : 'copy.intro.cloneRepo'
    : scope.value === 'tag'
      ? 'copy.intro.renameTag'
      : 'copy.intro.renameRepo',
);
const source = computed(() => sourceRefParts(props.repository, props.tag));
const preview = computed(() =>
  copyPreview(props.repository, props.tag, targetRepository.value, targetTag.value),
);
const canSubmit = computed(() => preview.value.errorKey === null && !submitting.value);

watch(
  () => props.open,
  (open) => {
    if (open) {
      targetRepository.value = '';
      targetTag.value = '';
      timeoutMinutes.value = DEFAULT_TIMEOUT_MINUTES;
      retryTimes.value = DEFAULT_RETRY_TIMES;
    }
  },
);

async function submit(): Promise<void> {
  if (preview.value.errorKey) {
    message.warning(t(preview.value.errorKey));
    return;
  }
  submitting.value = true;
  try {
    const payload = {
      repository: props.repository,
      tag: props.tag,
      target_repository: targetRepository.value.trim() || undefined,
      target_tag: targetTag.value.trim() || undefined,
      timeout_minutes: timeoutMinutes.value,
      retry_times: retryTimes.value,
    };
    const res = props.mode === 'rename' ? await renameImage(payload) : await cloneImage(payload);
    message.success(t('copy.messages.started', { verb: verb.value }));
    emit('started', res.task_id);
    visible.value = false;
  } catch (e) {
    message.error(
      t('copy.messages.failed', { verb: verb.value, error: resolveApiErrorMessage(e) }),
    );
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <a-modal v-model:open="visible" :title="t(`copy.titles.${mode}`)" width="680px" :footer="null">
    <p class="ih-muted" style="margin-top: 0">{{ t(introKey) }}</p>

    <div class="form-row">
      <span class="label">{{ t('copy.source') }}</span>
      <span class="mono">
        {{ source.ref }}<template v-if="source.allTags">{{ t('copy.allTags') }}</template>
      </span>
    </div>

    <div class="form-row">
      <span class="label">{{ t('copy.targetRepo') }}</span>
      <a-input
        v-model:value="targetRepository"
        :placeholder="
          scope === 'tag'
            ? t('copy.targetRepoPlaceholderTag', { repository })
            : t('copy.targetRepoPlaceholderRepo')
        "
        style="width: 380px"
        allow-clear
      />
    </div>

    <div v-if="scope === 'tag'" class="form-row">
      <span class="label">{{ t('copy.targetTag') }}</span>
      <a-input
        v-model:value="targetTag"
        :placeholder="t('copy.targetTagPlaceholder', { tag })"
        style="width: 220px"
        allow-clear
      />
    </div>
    <div v-else class="hint">{{ t('copy.allTagsHint') }}</div>

    <div class="form-row">
      <span class="label">{{ t('copy.preview') }}</span>
      <span v-if="preview.errorKey" class="error">{{ t(preview.errorKey) }}</span>
      <span v-else class="mono">{{ preview.targetRef }}</span>
    </div>
    <div v-if="scope === 'tag'" class="hint">
      {{ t('copy.normalizeHint1') }}<code>docker.io</code>{{ t('copy.normalizeHint2') }}<code>library/</code>{{ t('copy.normalizeHint3') }}
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('copy.timeoutLabel') }}</span>
      <TimeoutSelect v-model="timeoutMinutes" />
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('copy.retryLabel') }}</span>
      <RetrySelect v-model="retryTimes" />
    </div>

    <div class="footer">
      <a-button @click="visible = false">{{ t('common.cancel') }}</a-button>
      <a-button type="primary" :loading="submitting" :disabled="!canSubmit" @click="submit">
        {{ t(`copy.start.${mode}`) }}
      </a-button>
    </div>
  </a-modal>
</template>

<style scoped>
.form-row {
  display: flex;
  align-items: center;
  gap: 12px;
  margin-bottom: 12px;
}
.form-row.align-top {
  align-items: flex-start;
}
.label {
  width: 84px;
  flex: none;
  font-size: 13px;
  color: var(--ih-text-secondary);
}
.mono {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  font-size: 12.5px;
  word-break: break-all;
}
.hint {
  margin: -4px 0 12px 96px;
  font-size: 12px;
  color: var(--ih-text-secondary);
  line-height: 1.6;
}
.error {
  color: var(--ih-danger);
  font-size: 12.5px;
}
.footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
}
code {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  background: var(--ih-surface-alt);
  border: 1px solid var(--ih-border);
  border-radius: 4px;
  padding: 0 4px;
  font-size: 12px;
}
</style>