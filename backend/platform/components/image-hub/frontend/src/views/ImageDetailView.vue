<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRoute, useRouter } from 'vue-router';
import { message } from 'ant-design-vue';
import type { TableColumnsType } from 'ant-design-vue';
import PageShell from '@/components/PageShell.vue';
import CommandModal from '@/components/CommandModal.vue';
import AccessGuideModal from '@/components/AccessGuideModal.vue';
import CopyImageModal from '@/components/CopyImageModal.vue';
import SyncImageModal from '@/components/SyncImageModal.vue';
import TaskProgressDrawer from '@/components/TaskProgressDrawer.vue';
import { deleteImage, fetchImageDetail, type ImageTag } from '@/api/imageHub';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import { buildDownloadAndRun, buildOneLiner, buildPullCommands, scriptUrlFrom } from '@/utils/commands';
import type { CopyMode } from '@/utils/copy';
import { formatBytes, formatTime } from '@/utils/format';
import { hasPlatforms, platformColumnParts, platformEntryText } from '@/utils/platform';
import { useRegistryAddress } from '@/composables/useRegistryAddress';
import { useTasks } from '@/composables/useTasks';

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const { port, addresses, selectedIp, registryHost, registryUrl, reload: reloadAddress } = useRegistryAddress();

const repository = computed(() => String(route.query.repository ?? ''));

const loading = ref(false);
const tags = ref<ImageTag[]>([]);

const {
  tasks,
  loading: taskLoading,
  reload: reloadTasks,
  track: trackTask,
  remove: removeTask,
  cancel: cancelTask,
} = useTasks(() => void load());

async function load(): Promise<void> {
  if (!repository.value) return;
  loading.value = true;
  try {
    const data = await fetchImageDetail(repository.value);
    tags.value = data.tags ?? [];
  } catch (e) {
    tags.value = [];
    message.error(
      t('detail.messages.loadFailed', { error: resolveApiErrorMessage(e) }),
    );
  } finally {
    loading.value = false;
  }
}

async function onRefresh(): Promise<void> {
  await Promise.all([load(), reloadAddress(), reloadTasks()]);
}

function goBack(): void {
  void router.push({ name: 'images' });
}

// ---- 详情页操作 ----
const totalBytes = computed(() => {
  const sizes = tags.value.map((tag) => tag.size_bytes).filter((s): s is number => typeof s === 'number');
  return sizes.length > 0 ? sizes.reduce((a, b) => a + b, 0) : null;
});

const tagColumns = computed<TableColumnsType>(() => [
  { title: t('detail.columns.tag'), dataIndex: 'tag', key: 'tag', width: 180 },
  { title: t('detail.columns.digest'), dataIndex: 'digest', key: 'digest' },
  { title: t('detail.columns.size'), key: 'size', width: 100 },
  { title: t('detail.columns.platforms'), key: 'platforms', width: 110 },
  { title: t('detail.columns.created'), key: 'created', width: 170 },
  { title: t('detail.columns.action'), key: 'action', width: 320 },
]);

const platformColumns = computed<TableColumnsType>(() => [
  { title: t('detail.columns.platforms'), key: 'platform', width: 200 },
  { title: t('detail.columns.digest'), dataIndex: 'digest', key: 'digest' },
  { title: t('detail.columns.size'), key: 'size', width: 120 },
]);

/** 「平台」列文案（1 条 → 平台；>1 → N 个；0 → 占位） */
function platformText(tag: ImageTag): string {
  const parts = platformColumnParts(tag.platforms);
  if (parts.kind === 'single') return parts.value;
  if (parts.kind === 'count') return t('detail.platformCount', { count: parts.value });
  return t('common.none');
}

/** 「平台」列标签色：有平台信息（含单平台）为蓝，无平台信息为灰 */
function platformTagColor(tag: ImageTag): string {
  return hasPlatforms(tag.platforms) ? 'blue' : 'default';
}

async function onDeleteTag(tag: string): Promise<void> {
  try {
    await deleteImage(repository.value, tag);
    message.success(
      t('detail.messages.deleteTagSuccess', { repository: repository.value, tag }),
    );
    await load();
  } catch (e) {
    message.error(t('detail.messages.deleteFailed', { error: resolveApiErrorMessage(e) }));
  }
}

async function onDeleteRepo(): Promise<void> {
  try {
    await deleteImage(repository.value);
    message.success(
      t('detail.messages.deleteRepoSuccess', { repository: repository.value }),
    );
    goBack();
  } catch (e) {
    message.error(t('detail.messages.deleteFailed', { error: resolveApiErrorMessage(e) }));
  }
}

// ---- 同步（可选镜像源） ----
const syncOpen = ref(false);
const syncTag = ref<string | undefined>(undefined);

function onSync(tag?: string): void {
  syncTag.value = tag;
  syncOpen.value = true;
}

function onSyncStarted(taskId: string): void {
  taskPanelOpen.value = true;
  trackTask(taskId);
}

async function onCancelTask(id: string): Promise<void> {
  try {
    await cancelTask(id);
    message.success(t('detail.messages.cancelRequested'));
  } catch (e) {
    message.error(t('detail.messages.cancelFailed', { error: resolveApiErrorMessage(e) }));
  }
}

// ---- 克隆 / 改名（整仓 + 单 tag） ----
const copyOpen = ref(false);
const copyMode = ref<CopyMode>('clone');
const copyTag = ref<string | undefined>(undefined);

function onCopy(mode: CopyMode, tag?: string): void {
  copyMode.value = mode;
  copyTag.value = tag;
  copyOpen.value = true;
}

function onCopyStarted(taskId: string): void {
  taskPanelOpen.value = true;
  trackTask(taskId);
}

// ---- 下载命令弹窗 ----
const downloadOpen = ref(false);
const downloadTag = ref('');
const downloadSteps = computed(() => {
  if (!selectedIp.value) return [];
  return buildPullCommands(selectedIp.value, port.value, repository.value, downloadTag.value);
});

/** registry 地址（`<ip>:<port>`），与页面所选一致 */
const downloadIhAddr = computed(() =>
  selectedIp.value ? `${selectedIp.value}:${port.value}` : '',
);
/** 推荐一键命令（经 image-hub.sh） */
const downloadOneLiner = computed(() =>
  downloadIhAddr.value && downloadTag.value
    ? buildOneLiner(
        scriptUrlFrom(window.location.href),
        downloadIhAddr.value,
        'pull',
        `${repository.value}:${downloadTag.value}`,
      )
    : '',
);
/** 「先下载再执行」等价命令 */
const downloadLine = computed(() =>
  downloadIhAddr.value && downloadTag.value
    ? buildDownloadAndRun(
        scriptUrlFrom(window.location.href),
        downloadIhAddr.value,
        'pull',
        `${repository.value}:${downloadTag.value}`,
      )
    : '',
);

function openDownload(tag: string): void {
  downloadTag.value = tag;
  downloadOpen.value = true;
}

// ---- 接入指引 / 任务面板 ----
const guideOpen = ref(false);
const taskPanelOpen = ref(false);

async function copyReference(): Promise<void> {
  try {
    await navigator.clipboard.writeText(repository.value);
    message.success(t('detail.upstream.copySuccess'));
  } catch {
    message.error(t('common.copyFailed'));
  }
}

watch(repository, () => {
  if (repository.value) void load();
});

// 打开任务抽屉时即时拉取任务（避免页面刷新后显示「暂无任务」）
watch(taskPanelOpen, (open) => {
  if (open) void reloadTasks();
});

onMounted(() => {
  if (!repository.value) {
    goBack();
    return;
  }
  void load();
  void reloadTasks();
});
</script>

<template>
  <PageShell
    :title="repository || t('detail.titleFallback')"
    :subtitle="
      t('detail.subtitle', {
        registry: registryHost || '-',
        count: tags.length,
        size: formatBytes(totalBytes),
      })
    "
  >
    <template #actions>
      <a-button @click="goBack">{{ t('detail.actions.back') }}</a-button>
      <a-button @click="guideOpen = true">{{ t('detail.actions.guide') }}</a-button>
      <a-button @click="taskPanelOpen = true">
        <template v-if="tasks.length">
          {{ t('detail.actions.tasksWithCount', { count: tasks.length }) }}
        </template>
        <template v-else>{{ t('detail.actions.tasks') }}</template>
      </a-button>
      <a-button @click="() => onSync()">{{ t('detail.actions.syncAll') }}</a-button>
      <a-button @click="onCopy('clone')">{{ t('detail.actions.cloneRepo') }}</a-button>
      <a-button @click="onCopy('rename')">{{ t('detail.actions.renameRepo') }}</a-button>
      <a-popconfirm
        :title="t('detail.confirm.deleteRepo', { repository })"
        :ok-text="t('common.delete')"
        :cancel-text="t('common.cancel')"
        @confirm="onDeleteRepo"
      >
        <a-button danger>{{ t('detail.actions.deleteRepo') }}</a-button>
      </a-popconfirm>
      <a-button :loading="loading" @click="onRefresh">{{ t('detail.actions.refresh') }}</a-button>
    </template>

    <div class="ih-card ref-card">
      <div class="ref-row">
        <span class="ref-label">{{ t('detail.upstream.label') }}</span>
        <span class="repo-name">{{ repository }}</span>
        <a-button type="link" size="small" @click="copyReference">
          {{ t('detail.upstream.copy') }}
        </a-button>
      </div>
      <div class="ih-muted ref-hint">
        {{ t('detail.upstream.hint1') }}<code>{{ repository }}:&lt;tag&gt;</code>{{ t('detail.upstream.hint2') }}
      </div>
    </div>

    <a-table
      :columns="tagColumns"
      :data-source="tags"
      :loading="loading"
      :pagination="false"
      row-key="tag"
      style="margin-top: var(--ih-space-lg)"
    >
      <template #bodyCell="{ column, record }">
        <template v-if="column.key === 'digest'">
          <span class="mono">{{ record.digest || '-' }}</span>
        </template>
        <template v-else-if="column.key === 'size'">
          {{ formatBytes(record.size_bytes) }}
        </template>
        <template v-else-if="column.key === 'platforms'">
          <a-tag :color="platformTagColor(record)">
            {{ platformText(record) }}
          </a-tag>
        </template>
        <template v-else-if="column.key === 'created'">
          {{ formatTime(record.created) }}
        </template>
        <template v-else-if="column.key === 'action'">
          <a-button type="link" size="small" @click="openDownload(record.tag)">
            {{ t('detail.row.download') }}
          </a-button>
          <a-button type="link" size="small" @click="() => onSync(record.tag)">
            {{ t('detail.row.sync') }}
          </a-button>
          <a-button type="link" size="small" @click="onCopy('clone', record.tag)">
            {{ t('detail.row.clone') }}
          </a-button>
          <a-button type="link" size="small" @click="onCopy('rename', record.tag)">
            {{ t('detail.row.rename') }}
          </a-button>
          <a-popconfirm
            :title="t('detail.confirm.deleteTag', { repository, tag: record.tag })"
            :ok-text="t('common.delete')"
            :cancel-text="t('common.cancel')"
            @confirm="onDeleteTag(record.tag)"
          >
            <a-button type="link" danger size="small">{{ t('detail.row.delete') }}</a-button>
          </a-popconfirm>
        </template>
      </template>

      <template #expandedRowRender="{ record }">
        <div class="expand">
          <div class="expand-row">
            <span class="expand-label">{{ t('detail.expand.digest') }}</span>
            <span class="mono">{{ record.digest || '-' }}</span>
          </div>
          <div class="expand-row">
            <span class="expand-label">{{ t('detail.expand.size') }}</span>
            <span>{{ formatBytes(record.size_bytes) }}</span>
          </div>
          <div class="expand-row">
            <span class="expand-label">{{ t('detail.expand.created') }}</span>
            <span>{{ formatTime(record.created) }}</span>
          </div>
          <div class="expand-row">
            <span class="expand-label">{{ t('detail.expand.mediaType') }}</span>
            <span class="mono">{{ record.media_type || '-' }}</span>
          </div>

          <div v-if="hasPlatforms(record.platforms)" class="platforms">
            <div class="expand-label" style="margin-bottom: 6px">
              {{ t('detail.expand.platforms') }}<template v-if="(record.platforms?.length ?? 0) === 1">{{ t('detail.expand.singlePlatform') }}</template>
            </div>
            <a-table
              :columns="platformColumns"
              :data-source="record.platforms"
              :pagination="false"
              row-key="digest"
              size="small"
            >
              <template #bodyCell="{ column: pcol, record: p }">
                <template v-if="pcol.key === 'platform'">
                  {{ platformEntryText(p) }}
                </template>
                <template v-else-if="pcol.key === 'digest'">
                  <span class="mono">{{ p.digest || '-' }}</span>
                </template>
                <template v-else-if="pcol.key === 'size'">
                  {{ formatBytes(p.size_bytes) }}
                </template>
              </template>
            </a-table>
          </div>
        </div>
      </template>

      <template #emptyText>
        <div class="ih-muted" style="padding: 24px 0">{{ t('detail.empty') }}</div>
      </template>
    </a-table>

    <AccessGuideModal
      v-model:open="guideOpen"
      v-model:selected-ip="selectedIp"
      :addresses="addresses"
      :port="port"
      :registry-url="registryUrl"
    />

    <CommandModal
      v-model:open="downloadOpen"
      :title="t('detail.download.title', { repository, tag: downloadTag })"
      :description="t('detail.download.description')"
      :one-liner="downloadOneLiner"
      :one-liner-hint="t('detail.download.oneLinerHint')"
      :download-line="downloadLine"
      :steps="downloadSteps"
    />

    <SyncImageModal
      v-model:open="syncOpen"
      :repository="repository"
      :tag="syncTag"
      @started="onSyncStarted"
    />

    <CopyImageModal
      v-model:open="copyOpen"
      :mode="copyMode"
      :repository="repository"
      :tag="copyTag"
      @started="onCopyStarted"
    />

    <TaskProgressDrawer
      v-model:open="taskPanelOpen"
      :tasks="tasks"
      :loading="taskLoading"
      @refresh="reloadTasks"
      @remove="removeTask"
      @cancel="onCancelTask"
    />
  </PageShell>
</template>

<style scoped>
.ref-card {
  margin-bottom: var(--ih-space-lg);
}
.ref-row {
  display: flex;
  align-items: center;
  gap: var(--ih-space-sm);
  flex-wrap: wrap;
}
.ref-label {
  font-size: 13px;
  color: var(--ih-text-secondary);
}
.repo-name {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  font-size: 14px;
  font-weight: 600;
  word-break: break-all;
}
.ref-hint {
  margin-top: var(--ih-space-xs);
  font-size: 12px;
}
.mono {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  font-size: 12px;
  word-break: break-all;
}
.expand {
  padding: var(--ih-space-sm) var(--ih-space-lg);
}
.expand-row {
  display: flex;
  gap: var(--ih-space);
  font-size: 12.5px;
  margin-bottom: 4px;
}
.expand-label {
  color: var(--ih-text-tertiary);
  width: 92px;
  flex: none;
}
.platforms {
  margin-top: var(--ih-space);
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