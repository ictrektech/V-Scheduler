<script setup lang="ts">
import { computed, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import { pullImage } from '@/api/imageHub';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import MirrorSelect from '@/components/MirrorSelect.vue';
import RetrySelect from '@/components/RetrySelect.vue';
import TimeoutSelect from '@/components/TimeoutSelect.vue';
import { useMirrors } from '@/composables/useMirrors';
import {
  ARCH_OPTIONS,
  DEFAULT_SPECIFIC_ARCHS,
  archRequestValue,
  normalizeArchList,
  type ArchMode,
} from '@/utils/arch';
import { localImageRef, parseImageRef, upstreamRepository } from '@/utils/image';
import { resolveMirrorSourceRef } from '@/utils/mirror';
import { DEFAULT_RETRY_TIMES, DEFAULT_TIMEOUT_MINUTES } from '@/utils/task';

/**
 * 拉取镜像弹窗：输入完整上游镜像名，可选镜像源（mirror）、架构、单次传输超时、
 * 失败重试次数与 insecure，经后端 skopeo 拉取并推入本地 registry。
 *
 * 关键语义：镜像源**仅改变实际拉取来源**，落地仓库名恒为上游完整镜像名
 * （如 `ubuntu:22.04` + `docker.m.daocloud.io` → 落地 `docker.io/library/ubuntu:22.04`）。
 */
const { t } = useI18n();

const props = defineProps<{
  open: boolean;
  /** 本地 registry 目标 host（用于预览本地引用），如 `127.0.0.1:19913` */
  destHost: string;
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

const image = ref('');
const mirror = ref<string | null>(null);
const timeoutMinutes = ref<number>(DEFAULT_TIMEOUT_MINUTES);
const retryTimes = ref<number>(DEFAULT_RETRY_TIMES);
const archMode = ref<ArchMode>('all');
/** 「指定架构」多选（默认预选常用架构） */
const specificArchs = ref<string[]>([...DEFAULT_SPECIFIC_ARCHS]);
const insecure = ref(true);
const submitting = ref(false);

/** 常见架构候选（docker 标准完整平台名；skopeo 平台列表不支持 variant） */
const archSelectOptions = ARCH_OPTIONS.map((value) => ({ value, label: value }));

/** 「指定架构」模式是否已选至少一个架构 */
const hasSpecificArch = computed(() => normalizeArchList(specificArchs.value).length > 0);

const parsed = computed(() => parseImageRef(image.value));
const upstream = computed(() => (parsed.value ? `${upstreamRepository(parsed.value)}:${parsed.value.tag}` : ''));
const localPreview = computed(() =>
  parsed.value ? localImageRef(props.destHost, parsed.value) : '',
);
/** 落地仓库名（上游完整镜像名，不含镜像源主机名） */
const upstreamRepoName = computed(() => (parsed.value ? upstreamRepository(parsed.value) : ''));
/** 实际拉取源（选择镜像源时使用镜像源主机名） */
const sourcePreview = computed(() =>
  parsed.value ? resolveMirrorSourceRef(mirror.value, parsed.value) : '',
);
const archValue = computed(() => archRequestValue(archMode.value, specificArchs.value));

const canSubmit = computed(() => {
  if (!parsed.value || submitting.value || !props.destHost) return false;
  if (archMode.value === 'specific' && !hasSpecificArch.value) return false;
  return true;
});

watch(
  () => props.open,
  (open) => {
    if (open) {
      image.value = '';
      mirror.value = null;
      timeoutMinutes.value = DEFAULT_TIMEOUT_MINUTES;
      retryTimes.value = DEFAULT_RETRY_TIMES;
      archMode.value = 'all';
      specificArchs.value = [...DEFAULT_SPECIFIC_ARCHS];
      insecure.value = true;
      void ensureLoaded();
    }
  },
);

// 上游 registry 改变时，原先选中的镜像源不再适用 → 重置为「不使用」
watch(
  () => parsed.value?.registry,
  () => {
    mirror.value = null;
  },
);

async function submit(): Promise<void> {
  if (!parsed.value) {
    message.error(t('pull.messages.invalidRef'));
    return;
  }
  if (archMode.value === 'specific' && !hasSpecificArch.value) {
    message.error(t('pull.archRequired'));
    return;
  }
  submitting.value = true;
  try {
    const res = await pullImage(
      image.value.trim(),
      archValue.value,
      insecure.value,
      mirror.value,
      timeoutMinutes.value,
      retryTimes.value,
    );
    message.success(t('pull.messages.started'));
    emit('started', res.task_id);
    visible.value = false;
  } catch (e) {
    message.error(t('pull.messages.failed', { error: resolveApiErrorMessage(e) }));
  } finally {
    submitting.value = false;
  }
}
</script>

<template>
  <a-modal v-model:open="visible" :title="t('pull.title')" width="680px" :footer="null">
    <p class="ih-muted" style="margin-top: 0">
      {{ t('pull.intro1') }}<b>{{ t('pull.introStrong') }}</b>{{ t('pull.intro2') }}
    </p>

    <div class="form-row">
      <span class="label">{{ t('pull.imageLabel') }}</span>
      <a-input
        v-model:value="image"
        style="width: 420px"
        :placeholder="t('pull.placeholder')"
        @press-enter="submit"
      />
    </div>
    <div v-if="image && !parsed" class="hint error">
      {{ t('pull.invalid') }}
    </div>
    <div v-else-if="parsed" class="hint">
      {{ t('pull.normalized') }} <code>{{ upstream }}</code>
      <template v-if="localPreview">
        {{ t('pull.localRef') }} <code>{{ localPreview }}</code>
      </template>
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('pull.mirrorLabel') }}</span>
      <MirrorSelect
        v-model="mirror"
        :catalog="catalog"
        :upstream-registry="parsed?.registry ?? ''"
      />
    </div>
    <div v-if="parsed" class="hint">
      {{ t('pull.sourcePreview') }} <code>{{ sourcePreview }}</code>
      <template v-if="mirror">
        {{ t('pull.sourceKeep', { repo: upstreamRepoName }) }}
      </template>
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('pull.timeoutLabel') }}</span>
      <TimeoutSelect v-model="timeoutMinutes" />
    </div>

    <div class="form-row align-top">
      <span class="label">{{ t('pull.retryLabel') }}</span>
      <RetrySelect v-model="retryTimes" />
    </div>

    <div class="form-row">
      <span class="label">{{ t('pull.archLabel') }}</span>
      <a-radio-group v-model:value="archMode" button-style="solid">
        <a-radio-button value="all">{{ t('pull.archAll') }}</a-radio-button>
        <a-radio-button value="host">{{ t('pull.archHost') }}</a-radio-button>
        <a-radio-button value="specific">{{ t('pull.archSpecific') }}</a-radio-button>
      </a-radio-group>
    </div>
    <div v-if="archMode === 'specific'" class="form-row align-top">
      <span class="label"></span>
      <a-select
        v-model:value="specificArchs"
        mode="multiple"
        :options="archSelectOptions"
        style="width: 420px"
        :placeholder="t('pull.archPlaceholder')"
      />
    </div>
    <div v-if="archMode === 'specific' && !hasSpecificArch" class="hint error">
      {{ t('pull.archRequired') }}
    </div>
    <div class="hint">
      {{ t('pull.archHint1') }}<code>linux/amd64</code> + <code>linux/arm64</code>{{ t('pull.archHint2') }}<code>--strip-removed-platforms</code>{{ t('pull.archHint3') }}
    </div>

    <div class="form-row">
      <span class="label">{{ t('pull.insecure') }}</span>
      <a-switch v-model:checked="insecure" />
      <span class="hint inline">{{ t('pull.insecureHint') }}</span>
    </div>

    <div class="footer">
      <a-button @click="visible = false">{{ t('common.cancel') }}</a-button>
      <a-button type="primary" :disabled="!canSubmit" :loading="submitting" @click="submit">
        {{ t('pull.start') }}
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
.hint.inline {
  margin: 0;
}
.hint.error {
  color: var(--ih-danger);
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