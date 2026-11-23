<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { useRouter } from 'vue-router';
import { message } from 'ant-design-vue';
import type { TableColumnsType } from 'ant-design-vue';
import PageShell from '@/components/PageShell.vue';
import AccessGuideModal from '@/components/AccessGuideModal.vue';
import CopyImageModal from '@/components/CopyImageModal.vue';
import ImportImageModal from '@/components/ImportImageModal.vue';
import PullImageModal from '@/components/PullImageModal.vue';
import RegistryGcModal from '@/components/RegistryGcModal.vue';
import RegistrySettingsModal from '@/components/RegistrySettingsModal.vue';
import SyncImageModal from '@/components/SyncImageModal.vue';
import TaskProgressDrawer from '@/components/TaskProgressDrawer.vue';
import { deleteImage, fetchImages, type ImageTag, type RepositoryImages } from '@/api/imageHub';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import { formatBytes, formatTime } from '@/utils/format';
import type { CopyMode } from '@/utils/copy';
import { useRegistryAddress } from '@/composables/useRegistryAddress';
import { useTasks } from '@/composables/useTasks';

const { t } = useI18n();
const router = useRouter();
const { port, addresses, selectedIp, registryHost, registryUrl, reload: reloadAddress } = useRegistryAddress();

const loading = ref(false);
const deleting = ref(false);
const images = ref<RepositoryImages[]>([]);
const keyword = ref('');

const {
  tasks,
  loading: taskLoading,
  reload: reloadTasks,
  track: trackTask,
  remove: removeTask,
  cancel: cancelTask,
} = useTasks(() => void load());

interface RepoRow {
  repository: string;
  tags: ImageTag[];
  tagCount: number;
  totalBytes: number | null;
  latestCreated: string | null;
}

const rows = computed<RepoRow[]>(() => {
  const kw = keyword.value.trim().toLowerCase();
  return images.value
    .filter((repo) => !kw || repo.repository.toLowerCase().includes(kw))
    .map((repo) => {
      const sizes = repo.tags.map((t) => t.size_bytes).filter((s): s is number => typeof s === 'number');
      const created = repo.tags
        .map((t) => t.created)
        .filter((c): c is string => !!c)
        .sort();
      return {
        repository: repo.repository,
        tags: repo.tags,
        tagCount: repo.tags.length,
        totalBytes: sizes.length > 0 ? sizes.reduce((a, b) => a + b, 0) : null,
        latestCreated: created.length > 0 ? created[created.length - 1] : null,
      };
    });
});

const columns = computed<TableColumnsType>(() => [
  { title: t('images.columns.repository'), key: 'repository' },
  { title: t('images.columns.tags'), key: 'tags', width: 300 },
  { title: t('images.columns.size'), key: 'size', width: 110 },
  { title: t('images.columns.created'), key: 'created', width: 170 },
  { title: t('images.columns.action'), key: 'action', width: 300 },
]);

async function load(): Promise<void> {
  loading.value = true;
  try {
    const data = await fetchImages();
    images.value = data.images ?? [];
  } catch (e) {
    images.value = [];
    message.error(
      t('images.messages.loadFailed', { error: resolveApiErrorMessage(e) }),
    );
  } finally {
    loading.value = false;
  }
}

async function onRefresh(): Promise<void> {
  await Promise.all([load(), reloadAddress(), reloadTasks()]);
}

function openDetail(repository: string): void {
  void router.push({ name: 'image-detail', query: { repository } });
}

async function onDeleteRepo(repository: string): Promise<void> {
  deleting.value = true;
  try {
    await deleteImage(repository);
    message.success(t('images.messages.deleteSuccess', { repository }));
    await load();
  } catch (e) {
    message.error(t('images.messages.deleteFailed', { error: resolveApiErrorMessage(e) }));
  } finally {
    deleting.value = false;
  }
}

// ---- 同步（可选镜像源） ----
const syncOpen = ref(false);
const syncRepo = ref('');

function onSync(repository: string): void {
  syncRepo.value = repository;
  syncOpen.value = true;
}

function onSyncStarted(taskId: string): void {
  taskPanelOpen.value = true;
  trackTask(taskId);
}

async function onCancelTask(id: string): Promise<void> {
  try {
    await cancelTask(id);
    message.success(t('images.messages.cancelRequested'));
  } catch (e) {
    message.error(t('images.messages.cancelFailed', { error: resolveApiErrorMessage(e) }));
  }
}

// ---- 克隆 / 改名（整仓；tag 级在详情页） ----
const copyOpen = ref(false);
const copyMode = ref<CopyMode>('clone');
const copyRepository = ref('');

function onCopy(repository: string, mode: CopyMode): void {
  copyRepository.value = repository;
  copyMode.value = mode;
  copyOpen.value = true;
}

function onCopyStarted(taskId: string): void {
  taskPanelOpen.value = true;
  trackTask(taskId);
}

// ---- 弹窗 / 抽屉 ----
const guideOpen = ref(false);
const pullOpen = ref(false);
const importOpen = ref(false);
const gcOpen = ref(false);
const settingsOpen = ref(false);
const taskPanelOpen = ref(false);

function onImportStarted(taskId: string): void {
  taskPanelOpen.value = true;
  trackTask(taskId);
}

function onGcStarted(taskId: string): void {
  taskPanelOpen.value = true;
  trackTask(taskId);
}

async function copyAddress(): Promise<void> {
  try {
    await navigator.clipboard.writeText(registryUrl.value);
    message.success(t('images.registry.copySuccess'));
  } catch {
    message.error(t('common.copyFailed'));
  }
}

function onPullStarted(taskId: string): void {
  taskPanelOpen.value = true;
  trackTask(taskId);
}

// 打开任务抽屉时即时拉取任务（避免页面刷新后显示「暂无任务」）
watch(taskPanelOpen, (open) => {
  if (open) void reloadTasks();
});

onMounted(load);
</script>

<template>
  <PageShell :title="t('images.title')" :subtitle="t('images.subtitle')">
    <template #actions>
      <a-button type="primary" @click="pullOpen = true">{{ t('images.actions.pull') }}</a-button>
      <a-button @click="importOpen = true">{{ t('images.actions.import') }}</a-button>
      <a-button @click="gcOpen = true">{{ t('images.actions.gc') }}</a-button>
      <a-button @click="settingsOpen = true">{{ t('images.actions.settings') }}</a-button>
      <a-button @click="guideOpen = true">{{ t('images.actions.guide') }}</a-button>
      <a-button @click="taskPanelOpen = true">
        <template v-if="tasks.length">
          {{ t('images.actions.tasksWithCount', { count: tasks.length }) }}
        </template>
        <template v-else>{{ t('images.actions.tasks') }}</template>
      </a-button>
      <a-button :loading="loading" @click="onRefresh">{{ t('images.actions.refresh') }}</a-button>
    </template>

    <!-- Registry 地址 -->
    <div class="ih-card" style="margin-bottom: var(--ih-space-lg)">
      <div class="addr-row">
        <div class="addr-label">
          <span>{{ t('images.registry.label') }}</span>
          <span class="addr-hint">{{ t('images.registry.hint') }}</span>
        </div>
        <div class="addr-controls">
          <a-select
            v-model:value="selectedIp"
            :options="addresses.map((ip) => ({ label: ip, value: ip }))"
            :disabled="addresses.length === 0"
            style="width: 220px"
            :placeholder="t('images.registry.placeholder')"
          />
          <a-input :value="registryUrl" readonly style="width: 300px">
            <template #addonAfter>
              <a @click="copyAddress">{{ t('images.registry.copy') }}</a>
            </template>
          </a-input>
        </div>
      </div>
      <div class="addr-note ih-muted">{{ t('images.registry.note') }}</div>
    </div>

    <!-- 搜索 -->
    <div class="toolbar">
      <a-input-search
        v-model:value="keyword"
        :placeholder="t('images.search.placeholder')"
        style="width: 360px"
        allow-clear
      />
      <span class="ih-muted">{{ t('images.search.count', { count: rows.length }) }}</span>
    </div>

    <a-table
      :columns="columns"
      :data-source="rows"
      :loading="loading"
      :pagination="false"
      row-key="repository"
    >
      <template #bodyCell="{ column, record }">
        <template v-if="column.key === 'repository'">
          <a class="repo-name" @click="openDetail(record.repository)">{{ record.repository }}</a>
          <div class="ih-muted repo-sub">
            {{ t('images.row.tagCount', { count: record.tagCount }) }}
          </div>
        </template>
        <template v-else-if="column.key === 'tags'">
          <a-tag v-for="tag in record.tags.slice(0, 6)" :key="tag.tag">{{ tag.tag }}</a-tag>
          <span v-if="record.tags.length > 6" class="ih-muted">
            +{{ record.tags.length - 6 }}
          </span>
        </template>
        <template v-else-if="column.key === 'size'">
          {{ formatBytes(record.totalBytes) }}
        </template>
        <template v-else-if="column.key === 'created'">
          {{ formatTime(record.latestCreated) }}
        </template>
        <template v-else-if="column.key === 'action'">
          <a-button type="link" size="small" @click="openDetail(record.repository)">
            {{ t('images.row.detail') }}
          </a-button>
          <a-button type="link" size="small" @click="onSync(record.repository)">
            {{ t('images.row.sync') }}
          </a-button>
          <a-button type="link" size="small" @click="onCopy(record.repository, 'clone')">
            {{ t('images.row.clone') }}
          </a-button>
          <a-button type="link" size="small" @click="onCopy(record.repository, 'rename')">
            {{ t('images.row.rename') }}
          </a-button>
          <a-popconfirm
            :title="t('images.confirm.deleteRepo', { repository: record.repository })"
            :ok-text="t('common.delete')"
            :cancel-text="t('common.cancel')"
            @confirm="onDeleteRepo(record.repository)"
          >
            <a-button type="link" danger size="small" :loading="deleting">
              {{ t('images.row.deleteRepo') }}
            </a-button>
          </a-popconfirm>
        </template>
      </template>

      <template #emptyText>
        <div class="ih-muted" style="padding: 24px 0">{{ t('images.empty') }}</div>
      </template>
    </a-table>

    <PullImageModal
      v-model:open="pullOpen"
      :dest-host="registryHost"
      @started="onPullStarted"
    />

    <ImportImageModal v-model:open="importOpen" @started="onImportStarted" />

    <RegistryGcModal v-model:open="gcOpen" @started="onGcStarted" />

    <CopyImageModal
      v-model:open="copyOpen"
      :mode="copyMode"
      :repository="copyRepository"
      @started="onCopyStarted"
    />

    <RegistrySettingsModal v-model:open="settingsOpen" />

    <SyncImageModal
      v-model:open="syncOpen"
      :repository="syncRepo"
      @started="onSyncStarted"
    />

    <AccessGuideModal
      v-model:open="guideOpen"
      v-model:selected-ip="selectedIp"
      :addresses="addresses"
      :port="port"
      :registry-url="registryUrl"
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
.addr-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: var(--ih-space-lg);
  flex-wrap: wrap;
}
.addr-label {
  font-size: 14px;
  font-weight: 500;
  color: var(--ih-text);
}
.addr-hint {
  font-size: 12px;
  font-weight: 400;
  color: var(--ih-text-tertiary);
  margin-left: var(--ih-space-xs);
}
.addr-controls {
  display: flex;
  align-items: center;
  gap: var(--ih-space-sm);
}
.addr-note {
  margin-top: var(--ih-space-sm);
  font-size: 12px;
}
.toolbar {
  display: flex;
  align-items: center;
  gap: var(--ih-space-lg);
  margin-bottom: var(--ih-space);
}
.repo-name {
  font-family: 'SFMono-Regular', Consolas, Menlo, monospace;
  font-size: 13px;
  font-weight: 600;
  word-break: break-all;
  cursor: pointer;
}
.repo-name:hover {
  color: var(--ih-primary);
}
.repo-sub {
  font-size: 12px;
  margin-top: 2px;
}
</style>