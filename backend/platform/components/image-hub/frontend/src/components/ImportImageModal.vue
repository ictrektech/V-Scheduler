<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import { importArchive } from '@/api/imageHub';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import RetrySelect from '@/components/RetrySelect.vue';
import TimeoutSelect from '@/components/TimeoutSelect.vue';
import { formatBytes } from '@/utils/format';
import { parseImageRef, upstreamReference } from '@/utils/image';
import { DEFAULT_RETRY_TIMES, DEFAULT_TIMEOUT_MINUTES } from '@/utils/task';

/**
 * 导入镜像归档弹窗。
 *
 * - 接受 docker-archive / OCI 归档（`tar` / `tar.gz` / `tar.xz` / 其它后缀均可，按内容识别）；
 * - 填写目标镜像名 → 使用该名称；留空 → 保留归档内的原始名称；
 * - 上传过程中展示上传进度，上传完成后由后端异步导入（可在「任务进度」中查看）。
 */
const { t } = useI18n();

const props = defineProps<{
  open: boolean;
}>();

const emit = defineEmits<{
  'update:open': [boolean];
  started: [taskId: string];
}>();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

const picked = ref<File | null>(null);
const repository = ref('');
const tag = ref('');
const timeoutMinutes = ref<number>(DEFAULT_TIMEOUT_MINUTES);
const retryTimes = ref<number>(DEFAULT_RETRY_TIMES);
const submitting = ref(false);
const uploadPercent = ref(0);
const uploading = ref(false);

/** 目标名归一预览（留空时提示保留原始名称） */
const targetPreview = computed<{
  kind: 'default' | 'explicit';
  ref: string;
} | null>(() => {
  const repo = repository.value.trim();
  if (!repo) return null;
  const trimmedTag = tag.value.trim();
  const probe = trimmedTag ? `${repo}:${trimmedTag}` : repo;
  const parsed = parseImageRef(probe);
  if (!parsed) return null;
  if (!trimmedTag) {
    return { kind: 'default', ref: `${parsed.registry}/${parsed.repository}` };
  }
  return { kind: 'explicit', ref: upstreamReference(parsed) };
});

const targetInvalid = computed(() => {
  const repo = repository.value.trim();
  if (!repo) return false;
  const probe = tag.value.trim() ? `${repo}:${tag.value.trim()}` : repo;
  return !parseImageRef(probe);
});

const canSubmit = computed(() => !!picked.value && !submitting.value && !targetInvalid.value);

watch(
  () => props.open,
  (open) => {
    if (open) {
      picked.value = null;
      repository.value = '';
      tag.value = '';
      timeoutMinutes.value = DEFAULT_TIMEOUT_MINUTES;
      retryTimes.value = DEFAULT_RETRY_TIMES;
      uploadPercent.value = 0;
      uploading.value = false;
    }
  },
);

function onBeforeUpload(file: File): boolean {
  picked.value = file;
  return false;
}

async function submit(): Promise<void> {
  if (!picked.value) {
    message.warning(t('import.messages.noFile'));
    return;
  }
  if (targetInvalid.value) {
    message.warning(t('import.targetInvalid'));
    return;
  }
  submitting.value = true;
  uploading.value = true;
  uploadPercent.value = 0;
  try {
    const res = await importArchive(
      picked.value,
      {
        repository: repository.value.trim() || null,
        tag: tag.value.trim() || null,
        timeoutMinutes: timeoutMinutes.value,
        retryTimes: retryTimes.value,
      },
      (percent) => {
        uploadPercent.value = percent;
        if (percent >= 100) uploading.value = false;
      },
    );
    message.success(t('import.messages.started'));
    emit('started', res.task_id);
    visible.value = false;
  } catch (e) {
    message.error(t('import.messages.failed', { error: resolveApiErrorMessage(e) }));
  } finally {
    submitting.value = false;
    uploading.value = false;
  }
}
</script>

<template>
  <a-modal v-model:open="visible" :title="t('import.title')" width="720px" :footer="null">
    <p class="ih-muted" style="margin-top: 0">
      {{ t('import.intro1') }}<b>{{ t('import.introDocker') }}</b>{{ t('import.intro2') }}<b>{{ t('import.introOci') }}</b>{{ t('import.intro3') }}
    </p>

    <div class="form-row">
      <span class="label">{{ t('import.fileLabel') }}</span>
      <a-upload :before-upload="onBeforeUpload" :show-upload-list="false" :max-count="1">
        <a-button>{{ t('import.chooseFile') }}</a-button>
      </a-upload>
      <span v-if="picked" class="file-name">
        {{ t('import.fileMeta', { name: picked.name, size: formatBytes(picked.size) }) }}
      </span>
      <span v-else class="ih-muted">{{ t('import.noFile') }}</span>
    </div>

    <div class="form-row">
      <span class="label">{{ t('import.repoLabel') }}</span>
      <a-input
        v-model:value="repository"
        :placeholder="t('import.repoPlaceholder')"
        style="width: 320px"
        allow-clear
      />
      <a-input v-model:value="tag" :placeholder="t('import.tagPlaceholder')" style="width: 160px" allow-clear />
    </div>
    <div class="hint">
      <template v-if="targetInvalid">
        <span class="error">{{ t('import.targetInvalid') }}</span>
      </template>
      <template v-else-if="targetPreview">
        <template v-if="targetPreview.kind === 'default'">
          {{ t('import.defaultLatest', { repo: targetPreview.ref }) }}
        </template>
        <template v-else>
          {{ t('import.targetPreview', { ref: targetPreview.ref }) }}
        </template>
      </template>
      <template v-else>
        {{ t('import.keepOriginal1') }}<b>{{ t('import.keepOriginalStrong') }}</b>{{ t('import.keepOriginal2') }}
      </template>
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('import.timeoutLabel') }}</span>
      <TimeoutSelect v-model="timeoutMinutes" />
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('import.retryLabel') }}</span>
      <RetrySelect v-model="retryTimes" />
    </div>

    <div v-if="submitting" class="progress">
      <a-progress :percent="uploadPercent" :status="uploading ? 'active' : 'success'" />
      <div class="ih-muted progress-hint">
        <template v-if="uploading">{{ t('import.uploading', { percent: uploadPercent }) }}</template>
        <template v-else>{{ t('import.importing') }}</template>
      </div>
    </div>

    <div class="footer">
      <a-button @click="visible = false">{{ t('common.cancel') }}</a-button>
      <a-button type="primary" :loading="submitting" :disabled="!canSubmit" @click="submit">
        {{ t('import.start') }}
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
.hint {
  margin: -4px 0 12px 96px;
  font-size: 12px;
  color: var(--ih-text-secondary);
  line-height: 1.6;
}
.hint .error {
  color: var(--ih-danger);
}
.file-name {
  font-size: 12.5px;
}
.progress {
  margin-bottom: 12px;
}
.progress-hint {
  margin-top: 4px;
  font-size: 12px;
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