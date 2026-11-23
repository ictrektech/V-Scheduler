<script setup lang="ts">
import { computed, ref } from 'vue';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import type { Job } from '@/api/imageHub';
import { resolveTaskErrorMessage } from '@/i18n/api-error';
import {
  isStopping,
  phaseKey,
  progressParts,
  statusColor,
  statusKey,
  stripRegistryHost,
  timeoutParts,
  type TaskProgressDetail,
} from '@/utils/task';
import {
  gcHeadlineKey,
  gcMarkedCount,
  gcResultCounts,
  isGcTask,
  kindKey,
} from '@/utils/gc';
import { formatBytes, formatTime } from '@/utils/format';
import { formatArch } from '@/utils/arch';

/**
 * 任务进度抽屉：展示拉取/同步/克隆/改名/导入/GC 任务的状态、**友好进度**
 * （进度条 + 本地化摘要 + 计数明细）、可停止运行中任务；skopeo 原始输出收纳在折叠区
 * 供排障（运行中自动轮询由父页驱动）。
 *
 * 本地化策略（D-IH-93）：标题 / 进度 / 失败原因优先由结构化字段
 * （kind / source / dest / phase / 计数 / attempt / retry_in_secs / error_code）组合，
 * 仅当结构化字段缺失或阶段未知时回退后端 `title` / `progress` / `error` 原文。
 */
const { t } = useI18n();

const props = defineProps<{
  open: boolean;
  tasks: Job[];
  loading: boolean;
}>();

const emit = defineEmits<{
  'update:open': [boolean];
  refresh: [];
  remove: [id: string];
  cancel: [id: string];
}>();

const visible = computed({
  get: () => props.open,
  set: (v: boolean) => emit('update:open', v),
});

/** 正在请求停止的任务 id（按钮 loading） */
const stopping = ref<Set<string>>(new Set());

function isRunning(job: Job): boolean {
  return job.status === 'running' || job.status === 'pending';
}

/** 是否展示定量进度条（百分比可推导） */
function hasPercent(job: Job): boolean {
  return typeof job.progress_detail?.percent === 'number';
}

function percentOf(job: Job): number {
  return job.progress_detail?.percent ?? 0;
}

function isGc(job: Job): boolean {
  return isGcTask(job.kind);
}

/** 任务标题（结构化组合，回退后端 title） */
function taskTitle(job: Job): string {
  const dest = stripRegistryHost(job.dest);
  const source = stripRegistryHost(job.source);
  const items = job.progress_detail?.items_total ?? 1;
  switch (job.kind) {
    case 'pull':
      return dest ? t('tasks.titles.pull', { ref: dest }) : job.title;
    case 'sync':
      return dest
        ? items > 1
          ? t('tasks.titles.syncMany', { ref: dest, count: items })
          : t('tasks.titles.syncOne', { ref: dest })
        : job.title;
    case 'clone':
    case 'rename':
      return source && dest
        ? t(`tasks.titles.${job.kind}`, { source, dest })
        : job.title;
    case 'import': {
      const file = job.source.includes(':')
        ? job.source.slice(job.source.indexOf(':') + 1)
        : job.source;
      return dest
        ? items > 1
          ? t('tasks.titles.importMany', { file, count: items })
          : t('tasks.titles.importOne', { file, dest })
        : job.title;
    }
    case 'gc':
      return t(job.gc?.dry_run ? 'tasks.titles.gcDry' : 'tasks.titles.gc');
    default:
      return job.title || t('tasks.kind.task');
  }
}

/** 阶段文案（结构化组合，回退后端 progress） */
function phaseText(job: Job): string {
  const detail: TaskProgressDetail | undefined = job.progress_detail;
  const phase = detail?.phase ?? '';
  if (job.cancel_requested && job.status === 'running') return t('tasks.phase.stopping');
  if (phase === 'queued') {
    const total = detail?.items_total ?? 1;
    return total > 1
      ? t('tasks.phase.queuedMany', { count: total })
      : t('tasks.phase.queued');
  }
  if (phase === 'login') return t('tasks.phase.loginRunning');
  if (phase === 'preparing') return t('tasks.phase.preparingRunning');
  if (phase === 'copying') {
    const parts = progressParts(detail);
    if (parts.sub) {
      return t('tasks.phase.copyingSubLayers', {
        current: parts.sub.current,
        total: parts.sub.total,
        layers: parts.layers,
      });
    }
    if (parts.layers > 0) return t('tasks.phase.copyingLayers', { count: parts.layers });
    return t('tasks.phase.copying');
  }
  if (phase === 'writing') return t('tasks.phase.writingRunning');
  if (phase === 'gc') {
    const counts = gcResultCounts(job.gc);
    if (counts && (counts.blobs > 0 || counts.manifests > 0)) {
      return t('tasks.phase.gcProgress', {
        blobs: counts.blobs,
        manifests: counts.manifests,
      });
    }
    return t(job.gc?.dry_run ? 'tasks.phase.gcDryStart' : 'tasks.phase.gcStart');
  }
  if (phase === 'done') return t('tasks.phase.doneRunning');
  if (phase === 'cancelled') return t('tasks.phase.cancelledRunning');
  if (phase === 'failed') return t('tasks.phase.failedRunning');
  return job.progress || t('tasks.phase.running');
}

/** 进度主文案：重试退避 > 阶段文案（尝试序号前缀） */
function progressText(job: Job): string {
  if (job.status === 'cancelled') return t('tasks.phase.cancelledRunning');
  if (job.status === 'succeeded') return t('tasks.phase.doneRunning');
  if (job.status === 'failed') return t('tasks.phase.failedRunning');
  const parts = progressParts(job.progress_detail);
  if (job.cancel_requested) return t('tasks.phase.stopping');
  if (parts.retryInSecs != null && parts.attempt) {
    return t('tasks.attempt.retry', {
      attempt: parts.attempt.current,
      max: parts.attempt.max,
      seconds: parts.retryInSecs,
    });
  }
  const base = phaseText(job);
  const phase = job.progress_detail?.phase ?? '';
  if (parts.attempt && phase !== 'queued' && phase !== '') {
    return `${t('tasks.attempt.running', {
      attempt: parts.attempt.current,
      max: parts.attempt.max,
    })} · ${base}`;
  }
  return base;
}

/** 计数明细（标签 / 子镜像 / 已复制层数） */
function summaryOf(job: Job): string {
  const parts = progressParts(job.progress_detail);
  const list: string[] = [];
  if (parts.items) {
    list.push(t('tasks.counts.items', { done: parts.items.done, total: parts.items.total }));
  }
  if (parts.sub) {
    list.push(
      t('tasks.counts.sub', { current: parts.sub.current, total: parts.sub.total }),
    );
  }
  if (parts.layers > 0) list.push(t('tasks.counts.layers', { count: parts.layers }));
  return list.join(' · ');
}

/** 失败原因（结构化 error_code 优先，旧记录回退后端 error 原文） */
function errorText(job: Job): string | null {
  return resolveTaskErrorMessage(job.error_code, job.error);
}

/** 失败建议（按 error_code + 尝试次数推导，不依赖中文文本匹配） */
function hintOf(job: Job): string | null {
  if (job.status !== 'failed') return null;
  const detail = job.progress_detail;
  const exhausted =
    (detail?.max_attempts ?? 1) > 1 && (detail?.attempt ?? 0) >= (detail?.max_attempts ?? 1);
  if (exhausted) return t('tasks.hint.retries');
  if (job.error_code === 20807 || job.error_code === 20811) {
    return t('tasks.hint.transferFailed');
  }
  return null;
}

/** 架构展示（结构化 → 本地化） */
function archText(job: Job): string {
  const arch = formatArch(job.arch);
  switch (arch.kind) {
    case 'all':
      return t('arch.all');
    case 'host':
      return t('arch.host');
    case 'list':
      return arch.value;
    default:
      return t('common.none');
  }
}

/** 超时展示（结构化 → 本地化） */
function timeoutTextOf(job: Job): string {
  const parts = timeoutParts(job.timeout_minutes);
  if (parts.kind === 'unlimited') return t('timeout.presets.unlimited');
  return t(parts.kind === 'hours' ? 'timeout.hours' : 'timeout.minutes', {
    count: parts.value,
  });
}

/** 重试次数展示 */
function retryTextOf(job: Job): string {
  return t('retry.count', { count: job.retry_times });
}

/** GC 结果标题 */
function gcHeadline(job: Job): string {
  const key = gcHeadlineKey(job.gc);
  return key ? t(key) : '';
}

/** GC 结果计数 */
function gcResultText(job: Job): string {
  const counts = gcResultCounts(job.gc);
  if (!counts) return '';
  return t(counts.dryRun ? 'gc.result.textDry' : 'gc.result.text', {
    blobs: counts.blobs,
    manifests: counts.manifests,
  });
}

/** GC 仍被引用 blob */
function gcMarkedText(job: Job): string {
  const count = gcMarkedCount(job.gc);
  return count == null ? '' : t('gc.result.marked', { count });
}

function gcFreedText(job: Job): string {
  const freed = job.gc?.freed_bytes;
  return typeof freed === 'number' ? formatBytes(freed) : t('common.none');
}

function onCancel(job: Job): void {
  stopping.value = new Set(stopping.value).add(job.id);
  emit('cancel', job.id);
  // 兜底：2s 后清除 loading（任务状态由轮询刷新驱动）
  window.setTimeout(() => {
    const next = new Set(stopping.value);
    next.delete(job.id);
    stopping.value = next;
  }, 2000);
}

function onRemove(id: string): void {
  emit('remove', id);
  message.success(t('tasks.messages.cleaned'));
}
</script>

<template>
  <a-drawer
    :open="visible"
    :title="t('tasks.title')"
    width="680px"
    @update:open="(v: boolean) => emit('update:open', v)"
  >
    <template #extra>
      <a-button size="small" :loading="loading" @click="emit('refresh')">
        {{ t('tasks.refresh') }}
      </a-button>
    </template>

    <div v-if="tasks.length === 0" class="ih-muted" style="padding: 24px 0; text-align: center">
      {{ t('tasks.empty') }}
    </div>

    <div v-for="job in tasks" :key="job.id" class="ih-card task">
      <div class="task-head">
        <span class="task-title">
          <a-tag :color="statusColor(job.status)">{{ t(statusKey(job.status)) }}</a-tag>
          <a-tag>{{ t(kindKey(job.kind)) }}</a-tag>
          <span class="task-title-text">{{ taskTitle(job) }}</span>
        </span>
        <span class="task-actions">
          <a-popconfirm
            v-if="isRunning(job)"
            :title="t('tasks.confirm.stop')"
            :ok-text="t('tasks.confirm.stopOk')"
            :cancel-text="t('common.cancel')"
            @confirm="onCancel(job)"
          >
            <a-button
              type="link"
              danger
              size="small"
              :disabled="job.cancel_requested"
              :loading="stopping.has(job.id) || job.cancel_requested"
            >
              {{ job.cancel_requested ? t('tasks.action.stopping') : t('tasks.action.stop') }}
            </a-button>
          </a-popconfirm>
          <a-popconfirm
            v-if="!isRunning(job)"
            :title="t('tasks.confirm.clean')"
            :ok-text="t('tasks.confirm.cleanOk')"
            :cancel-text="t('common.cancel')"
            @confirm="onRemove(job.id)"
          >
            <a-button type="link" size="small">{{ t('tasks.action.clean') }}</a-button>
          </a-popconfirm>
        </span>
      </div>

      <!-- 进度区：可推导时定量，否则不定量（不伪造百分比）；已取消不再展示进度条 -->
      <div class="task-progress-area">
        <div v-if="job.status !== 'cancelled'" class="task-progress-line">
          <a-progress
            v-if="hasPercent(job)"
            :percent="percentOf(job)"
            :status="job.status === 'failed' ? 'exception' : job.status === 'succeeded' ? 'success' : 'active'"
            size="small"
          />
          <a-progress
            v-else
            :percent="0"
            :show-info="false"
            :status="job.status === 'failed' ? 'exception' : job.status === 'succeeded' ? 'success' : 'active'"
            size="small"
          />
        </div>
        <div class="task-progress-text">
          <a-tag :color="job.status === 'cancelled' ? 'default' : 'blue'">
            {{ t(phaseKey(job.progress_detail?.phase ?? '')) }}
          </a-tag>
          <span class="task-progress-msg">{{ progressText(job) }}</span>
        </div>
        <div v-if="summaryOf(job)" class="task-progress-counts ih-muted">
          {{ summaryOf(job) }}
        </div>
      </div>

      <div class="task-meta ih-muted">
        <template v-if="isGc(job)">
          <span>{{ t('tasks.meta.storageDir', { dir: job.source || t('common.none') }) }}</span>
        </template>
        <template v-else>
          <span v-if="job.mirror">{{ t('tasks.meta.mirror', { mirror: job.mirror }) }}</span>
          <span>{{ t('tasks.meta.arch', { arch: archText(job) }) }}</span>
          <span>{{ t('tasks.meta.timeout', { timeout: timeoutTextOf(job) }) }}</span>
          <span>{{ t('tasks.meta.retry', { retry: retryTextOf(job) }) }}</span>
        </template>
        <span>{{ t('tasks.meta.created', { time: formatTime(job.created_at) }) }}</span>
        <span v-if="job.finished_at">
          {{ t('tasks.meta.finished', { time: formatTime(job.finished_at) }) }}
        </span>
      </div>

      <!-- GC 结果摘要（非 gc 任务 / 无摘要时不展示） -->
      <div v-if="job.gc" class="task-gc">
        <a-tag :color="job.gc.dry_run ? 'default' : 'green'">{{ gcHeadline(job) }}</a-tag>
        <span>{{ gcResultText(job) }}</span>
        <span>{{ t('gc.result.freed', { size: gcFreedText(job) }) }}</span>
        <span class="ih-muted">{{ gcMarkedText(job) }}</span>
      </div>

      <div v-if="!isGc(job)" class="task-refs">
        <div>
          <span class="ref-label">{{ t('tasks.refs.source') }}</span><code>{{ job.source }}</code>
        </div>
        <div>
          <span class="ref-label">{{ t('tasks.refs.dest') }}</span><code>{{ job.dest }}</code>
        </div>
      </div>

      <div v-if="errorText(job)" class="task-error">{{ errorText(job) }}</div>
      <div v-if="hintOf(job)" class="task-hint">{{ hintOf(job) }}</div>

      <a-collapse v-if="job.logs.length > 0" ghost size="small">
        <a-collapse-panel key="logs" :header="t('tasks.logs.header', { count: job.logs.length })">
          <pre class="ih-cmd">{{ job.logs.join('\n') }}</pre>
        </a-collapse-panel>
      </a-collapse>
    </div>
  </a-drawer>
</template>

<style scoped>
.task {
  margin-bottom: var(--ih-space);
}
.task-head {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: var(--ih-space);
  margin-bottom: var(--ih-space-sm);
}
.task-title {
  font-size: 13.5px;
  font-weight: 600;
  word-break: break-all;
  color: var(--ih-text);
}
.task-title-text {
  margin-left: 4px;
}
.task-actions {
  flex: none;
}
.task-progress-area {
  margin-bottom: var(--ih-space-sm);
}
.task-progress-line {
  margin-bottom: 6px;
}
.task-progress-text {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 12.5px;
  color: var(--ih-text-secondary);
}
.task-progress-msg {
  word-break: break-all;
}
.task-progress-counts {
  margin-top: 4px;
  font-size: 12px;
}
.task-meta {
  display: flex;
  flex-wrap: wrap;
  gap: var(--ih-space-sm) var(--ih-space-lg);
  font-size: 12px;
  margin-top: var(--ih-space-xs);
}
.task-gc {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: var(--ih-space-sm);
  margin-top: var(--ih-space-sm);
  font-size: 12.5px;
  color: var(--ih-text-secondary);
}
.task-refs {
  margin-top: var(--ih-space-sm);
  font-size: 12px;
  color: var(--ih-text-secondary);
}
.task-refs .ref-label {
  display: inline-block;
  width: 48px;
  color: var(--ih-text-tertiary);
}
.task-refs code {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  word-break: break-all;
}
.task-error {
  margin-top: var(--ih-space-sm);
  font-size: 12.5px;
  color: var(--ih-danger);
  word-break: break-all;
}
.task-hint {
  margin-top: 4px;
  font-size: 12px;
  color: var(--ih-warning);
}
</style>