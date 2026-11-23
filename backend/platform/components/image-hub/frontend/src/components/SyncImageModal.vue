<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import { syncImage } from '@/api/imageHub';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import MirrorSelect from '@/components/MirrorSelect.vue';
import RetrySelect from '@/components/RetrySelect.vue';
import TimeoutSelect from '@/components/TimeoutSelect.vue';
import { useMirrors } from '@/composables/useMirrors';
import { parseImageRef } from '@/utils/image';
import { resolveMirrorSourceRef } from '@/utils/mirror';
import { DEFAULT_RETRY_TIMES, DEFAULT_TIMEOUT_MINUTES } from '@/utils/task';

/**
 * 同步镜像弹窗：对本地已有镜像重新从上游拉取更新。
 *
 * 可选镜像源（mirror）：仅改变实际拉取来源，本地仓库名不变
 * （如 `docker.io/library/ubuntu` + `docker.m.daocloud.io` →
 *  源 `docker.m.daocloud.io/library/ubuntu:22.04`、本地仍为 `docker.io/library/ubuntu`）。
 */
const { t } = useI18n();

const props = defineProps<{
  open: boolean;
  /** 本地 registry 中的仓库名（即上游完整镜像名） */
  repository: string;
  /** 指定 tag；缺省同步该仓库全部标签 */
  tag?: string;
}>();

const emit = defineEmits<{
  'update:open': [boolean];
  started: [taskId: string];
}>();

const { catalog, ensureLoaded } = useMirrors();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

const mirror = ref<string | null>(null);
const timeoutMinutes = ref<number>(DEFAULT_TIMEOUT_MINUTES);
const retryTimes = ref<number>(DEFAULT_RETRY_TIMES);
const submitting = ref(false);

/** 仓库归一（用于按上游 registry 匹配候选镜像源） */
const parsedRepo = computed(() => parseImageRef(props.repository));
const upstreamRegistry = computed(() => parsedRepo.value?.registry ?? '');
const scopeText = computed(() =>
  props.tag
    ? `${props.repository}:${props.tag}`
    : t('sync.allTags', { repository: props.repository }),
);
/** 实际拉取源预览（未指定 tag 时以 `<tag>` 占位表示全部标签） */
const sourcePreview = computed(() => {
  const ref = parsedRepo.value;
  if (!ref) return props.repository;
  return resolveMirrorSourceRef(mirror.value, { ...ref, tag: props.tag || '<tag>' });
});

watch(
  () => props.open,
  (open) => {
    if (open) {
      mirror.value = null;
      timeoutMinutes.value = DEFAULT_TIMEOUT_MINUTES;
      retryTimes.value = DEFAULT_RETRY_TIMES;
      void ensureLoaded();
    }
  },
);

async function submit(): Promise<void> {
  submitting.value = true;
  try {
    const res = await syncImage(
      props.repository,
      props.tag,
      'all',
      true,
      mirror.value,
      timeoutMinutes.value,
      retryTimes.value,
    );
    message.success(t('sync.messages.started'));
    emit('started', res.task_id);
    visible.value = false;
  } catch (e) {
    message.error(t('sync.messages.failed', { error: resolveApiErrorMessage(e) }));
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <a-modal v-model:open="visible" :title="t('sync.title')" width="680px" :footer="null">
    <p class="ih-muted" style="margin-top: 0">
      {{ t('sync.intro1') }}<b>{{ t('sync.introStrong') }}</b>{{ t('sync.intro2') }}
    </p>

    <div class="form-row align-top">
      <span class="label">{{ t('sync.timeoutLabel') }}</span>
      <TimeoutSelect v-model="timeoutMinutes" />
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('sync.retryLabel') }}</span>
      <RetrySelect v-model="retryTimes" />
    </div>

    <div class="form-row">
      <span class="label">{{ t('sync.scope') }}</span>
      <span class="mono">{{ scopeText }}</span>
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('sync.mirrorLabel') }}</span>
      <MirrorSelect v-model="mirror" :catalog="catalog" :upstream-registry="upstreamRegistry" />
    </div>
    <div class="hint">
      {{ t('sync.sourcePreview') }} <code>{{ sourcePreview }}</code>
    </div>

    <div class="footer">
      <a-button @click="visible = false">{{ t('common.cancel') }}</a-button>
      <a-button type="primary" :loading="submitting" @click="submit">{{ t('sync.start') }}</a-button>
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
  word-break: break-all;
}
</style>