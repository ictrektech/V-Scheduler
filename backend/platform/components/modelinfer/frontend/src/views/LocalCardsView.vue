<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message, Modal } from 'ant-design-vue';
import { DeleteOutlined, ReloadOutlined, UploadOutlined } from '@ant-design/icons-vue';
import PageShell from '@/components/PageShell.vue';
import StatPill from '@/components/StatPill.vue';
import ModelAvatar from '@/components/ModelAvatar.vue';
import StatusTag from '@/components/StatusTag.vue';
import TimeText from '@/components/TimeText.vue';
import CopyText from '@/components/CopyText.vue';
import {
  deletePlazaModel,
  listPlazaModelGroups,
  uploadLocalCard,
  type LocalModelGroup,
  type LocalModelStats,
  type LocalModelVersion,
} from '@/api/plaza';
import { GRID_PAGE_SIZE, PAGE_SIZE_OPTIONS } from '@/api/paging';
import { formatBytes } from '@/utils/format';
import {
  hexToRgba,
  modelFamilyIcon,
  modelFamilyLabel,
  modelFamilyTheme,
  type ModelFamilyDef,
} from '@/utils/modelFamily';
import { modelIconUrl } from '@/utils/modelIconAssets';
import { createSeqGuard, debounce } from '@/utils/async';
import { reportApiError } from '@/i18n/api-error';

const { t, locale } = useI18n();

/** 卡片 + 家族呈现主题（家族标识由后端推断并随列表返回） */
interface EnrichedCard extends LocalModelGroup {
  theme: ModelFamilyDef;
}

const loading = ref(false);
const uploading = ref(false);
/** 当前页卡片（服务端分页） */
const models = ref<LocalModelGroup[]>([]);
const plazaServer = ref('');
/** 未筛选全量统计（页头统计条 / 家族下拉） */
const stats = ref<LocalModelStats>({ cards: 0, versions: 0, size_bytes: 0, families: [] });
/** 服务端分页元数据（`total` 为筛选后卡片数） */
const page = reactive({ page_num: 1, page_size: GRID_PAGE_SIZE, total: 0 });
const seqGuard = createSeqGuard();

/** 工具条状态 */
const filters = reactive({
  keyword: '',
  family: 'all',
  source: 'all',
  sort: 'name' as 'name' | 'size' | 'versions' | 'latest',
});

const versionColumns = computed(() => {
  void locale.value;
  return [
    { title: t('cards.columns.version'), dataIndex: 'version', key: 'version', width: 130 },
    { title: t('cards.columns.state'), key: 'state', width: 100 },
    { title: t('cards.columns.size'), key: 'size', width: 110 },
    { title: t('cards.columns.createdAt'), key: 'created_at', width: 150 },
    { title: t('cards.columns.action'), key: 'action', width: 90, align: 'right' as const },
  ];
});

/** 版本管理弹窗（版本不在卡片上全量展开，避免卡片纵向过大） */
const versionModalOpen = ref(false);
const versionGroup = ref<LocalModelGroup | null>(null);

function openVersions(group: LocalModelGroup) {
  versionGroup.value = group;
  versionModalOpen.value = true;
}

/** 最新版本条目 */
function latestOf(group: LocalModelGroup): LocalModelVersion | null {
  return group.versions.find((v) => v.latest) || group.versions[0] || null;
}

/** 从最新版本的 manifest_json 解析 tags（用于家族推断，tags 优先级最高） */
function manifestTags(group: LocalModelGroup): string[] {
  const latest = group.versions.find((v) => v.latest) ?? group.versions[0];
  if (!latest?.manifest_json) return [];
  try {
    const raw = JSON.parse(latest.manifest_json) as { tags?: unknown };
    return Array.isArray(raw.tags) ? raw.tags.map((t) => String(t)) : [];
  } catch {
    return [];
  }
}

function enrich(group: LocalModelGroup): EnrichedCard {
  return { ...group, theme: modelFamilyTheme(group.family) };
}

const cards = computed<EnrichedCard[]>(() => models.value.map(enrich));

/** 家族筛选下拉：取响应 `stats.families`（未筛选的全量家族集合） */
const familyOptions = computed(() => {
  void locale.value;
  return [
    { label: t('cards.allFamilies'), value: 'all' },
    ...[...stats.value.families]
      .map((f) => ({ family: f, label: modelFamilyLabel(f, (k) => t(k)) }))
      .sort((a, b) => a.label.localeCompare(b.label, locale.value))
      .map((o) => ({ label: o.label, value: o.family })),
  ];
});

/** 来源筛选下拉 */
const sourceOptions = computed(() => {
  void locale.value;
  return [
    { label: t('cards.allSources'), value: 'all' },
    { label: t('cards.sourceLocal'), value: 'local' },
    { label: t('cards.sourcePlaza'), value: 'plaza' },
  ];
});

/** 排序筛选下拉 */
const sortOptions = computed(() => {
  void locale.value;
  return [
    { label: t('cards.sortByName'), value: 'name' },
    { label: t('cards.sortBySize'), value: 'size' },
    { label: t('cards.sortByVersions'), value: 'versions' },
    { label: t('cards.sortByLatest'), value: 'latest' },
  ];
});

/** 分页器配置（服务端分页） */
const pagination = computed(() => ({
  current: page.page_num,
  pageSize: page.page_size,
  total: page.total,
  showSizeChanger: true,
  pageSizeOptions: PAGE_SIZE_OPTIONS,
  showTotal: (total: number) => t('cards.totalCount', { total }),
}));

function onPageChange(p: { current?: number; pageSize?: number }) {
  if (p.pageSize && p.pageSize !== page.page_size) {
    page.page_size = p.pageSize;
    page.page_num = 1;
  } else if (p.current) {
    page.page_num = p.current;
  }
  void load();
}

const isFiltering = computed(
  () => !!filters.keyword.trim() || filters.family !== 'all' || filters.source !== 'all',
);

function cardStyle(card: EnrichedCard): Record<string, string> {
  const accent = card.theme.accent;
  return {
    '--mi-card-tint': hexToRgba(accent, 0.08),
    '--mi-card-accent': accent,
    '--mi-card-border-hover': hexToRgba(accent, 0.34),
    '--mi-family-tag-bg': hexToRgba(accent, 0.08),
    '--mi-family-tag-border': hexToRgba(accent, 0.3),
    '--mi-card-watermark': `url("${modelIconUrl(modelFamilyIcon(card.family))}")`,
  };
}

function sourceLabel(s: string): string {
  return s === 'local' ? t('cards.sourceLocal') : t('cards.sourcePlaza');
}

async function load() {
  const seq = seqGuard.next();
  loading.value = true;
  try {
    const data = await listPlazaModelGroups({
      page_num: page.page_num,
      page_size: page.page_size,
      keyword: filters.keyword.trim(),
      family: filters.family,
      source: filters.source,
      sort: filters.sort,
    });
    if (!seqGuard.isCurrent(seq)) return; // 丢弃过期响应
    models.value = data.data;
    stats.value = data.stats;
    page.total = data.total;
    plazaServer.value = data.plaza_server;
    if (data.pages > 0 && page.page_num > data.pages) {
      page.page_num = data.pages;
      void load();
    }
  } catch (e) {
    if (seqGuard.isCurrent(seq)) reportApiError(e);
  } finally {
    if (seqGuard.isCurrent(seq)) loading.value = false;
  }
}

/** 筛选 / 排序变更：回到第 1 页后重新加载 */
function reloadFromFirstPage() {
  page.page_num = 1;
  void load();
}

const debouncedKeywordReload = debounce(reloadFromFirstPage, 300);
watch(() => filters.keyword, () => debouncedKeywordReload());
watch(() => [filters.family, filters.source, filters.sort], reloadFromFirstPage);
onBeforeUnmount(() => debouncedKeywordReload.cancel());

/** 上传本地模型卡片包（含 model.tar.gz 的 tar.* 归档） */
function onUpload(file: File) {
  void doUpload(file);
  return false; // 阻止 antd 默认上传行为
}

async function doUpload(file: File) {
  uploading.value = true;
  try {
    const m = await uploadLocalCard(file);
    message.success(
      t('cards.uploadSuccess', { name: m.name, modelId: m.model_id, version: m.version }),
    );
    await load();
  } catch (e) {
    reportApiError(e);
  } finally {
    uploading.value = false;
  }
}

function onDelete(group: LocalModelGroup, version: LocalModelVersion) {
  Modal.confirm({
    title: t('cards.deleteTitle'),
    content: t('cards.deleteConfirm', { modelId: group.model_id, version: version.version }),
    okText: t('common.delete'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      await deletePlazaModel(version.id);
      message.success(t('cards.deleted'));
      await load();
      // 版本已被删除的卡片可能整体消失 → 同步弹窗数据或关闭
      const fresh = models.value.find((m) => m.model_id === group.model_id);
      if (fresh) {
        versionGroup.value = fresh;
      } else {
        versionGroup.value = null;
        versionModalOpen.value = false;
      }
    },
  });
}

onMounted(load);
</script>

<template>
  <PageShell
    :title="t('cards.title')"
    :subtitle="t('cards.subtitle')"
    :loading="loading"
  >
    <template #actions>
      <a-upload
        :show-upload-list="false"
        :before-upload="onUpload"
        :disabled="uploading"
        accept=".tar,.tar.gz,.tgz,.tar.xz,.txz"
      >
        <a-button type="primary" :loading="uploading">
          <UploadOutlined />
          {{ t('cards.upload') }}
        </a-button>
      </a-upload>
      <a-button @click="load">
        <ReloadOutlined />
        {{ t('common.refresh') }}
      </a-button>
    </template>

    <template #stats>
      <StatPill :label="t('cards.statCards')" :value="stats.cards" tone="primary" />
      <StatPill :label="t('cards.statVersions')" :value="stats.versions" />
      <StatPill :label="t('cards.statSize')" :value="formatBytes(stats.size_bytes)" />
    </template>

    <a-alert
      v-if="plazaServer"
      type="info"
      show-icon
      style="margin-bottom: 16px"
      :message="t('cards.plazaServerAlert', { server: plazaServer })"
    />

    <div v-if="stats.cards" class="mi-toolbar">
      <a-input-search
        v-model:value="filters.keyword"
        :placeholder="t('cards.searchPlaceholder')"
        allow-clear
        style="width: 280px"
      />
      <a-select v-model:value="filters.family" :options="familyOptions" style="width: 180px" />
      <a-select
        v-model:value="filters.source"
        style="width: 130px"
        :options="sourceOptions"
      />
      <a-select
        v-model:value="filters.sort"
        style="width: 150px; margin-left: auto"
        :options="sortOptions"
      />
    </div>

    <div v-if="cards.length" class="mi-card-grid">
      <div v-for="m in cards" :key="m.model_id" class="mi-mcard" :style="cardStyle(m)">
        <div class="mi-mcard__tint" />
        <div class="mi-mcard__watermark" />
        <div class="mi-mcard__inner">
          <div class="mi-mcard__head">
            <ModelAvatar :family="m.family" :size="44" />
            <div class="mi-mcard__titles">
              <div class="mi-mcard__name" :title="m.name || m.model_id">{{ m.name || m.model_id }}</div>
              <CopyText class="mi-mcard__id" :text="m.model_id" secondary />
            </div>
          </div>

          <div class="mi-mcard__tags">
            <a-tag class="mi-family-tag">{{ modelFamilyLabel(m.family, (k) => t(k)) }}</a-tag>
            <a-tag v-for="s in m.sources" :key="s" color="default">{{ sourceLabel(s) }}</a-tag>
            <span class="mi-mcard__weight">
              {{ t('cards.weight') }}
              <CopyText :text="m.model || '-'" />
            </span>
          </div>

          <div class="mi-mcard__stats">
            <span class="mi-mcard__stat"><b>{{ m.versions.length }}</b> {{ t('cards.versionsSuffix') }}</span>
            <a-tooltip :title="t('cards.cardSizeTooltip')">
              <span class="mi-mcard__stat">{{ t('cards.cardPackage') }} <b>{{ formatBytes(m.total_size_bytes) }}</b></span>
            </a-tooltip>
          </div>

          <div class="mi-mcard__version">
            <span class="mi-mcard__label">{{ t('cards.latestVersion') }}</span>
            <span class="mi-mono">{{ latestOf(m)?.version || '-' }}</span>
            <StatusTag kind="version" :status="latestOf(m)?.state" />
            <span class="mi-hint">{{ formatBytes(latestOf(m)?.size_bytes) }}</span>
            <TimeText :value="latestOf(m)?.created_at" />
            <a-tag v-if="m.versions.length > 1" class="mi-tag-soft">{{ t('cards.totalVersions', { count: m.versions.length }) }}</a-tag>
          </div>

          <div class="mi-mcard__actions">
            <a-button size="small" @click="openVersions(m)">{{ t('cards.manageVersions') }}</a-button>
          </div>
        </div>
      </div>
    </div>

    <a-empty v-else-if="stats.cards" :description="t('cards.noMatch')">
      <a-button @click="() => { filters.keyword = ''; filters.family = 'all'; filters.source = 'all'; }">
        {{ t('cards.clearFilters') }}
      </a-button>
    </a-empty>

    <a-empty v-else :description="t('cards.emptyTitle')">
      <template #description>
        <div>{{ t('cards.emptyTitle') }}</div>
        <div class="mi-hint">{{ t('cards.emptyHint') }}</div>
      </template>
    </a-empty>

    <a-pagination
      v-if="page.total > 0"
      class="mi-list-pagination"
      :current="page.page_num"
      :page-size="page.page_size"
      :total="page.total"
      :show-size-changer="true"
      :page-size-options="PAGE_SIZE_OPTIONS"
      :show-total="(total: number) => t('cards.totalCount', { total })"
      @change="(c: number, ps: number) => onPageChange({ current: c, pageSize: ps })"
    />

    <div v-if="stats.cards && isFiltering" class="mi-footer-hint">
      {{ t('cards.filteredHint', { shown: page.total, total: stats.cards }) }}
    </div>

    <!-- 版本管理弹窗：全量版本 + 版本级删除 -->
    <a-modal
      v-model:open="versionModalOpen"
      :title="t('cards.modalTitle', { id: versionGroup?.model_id || '' })"
      :footer="null"
      width="760px"
    >
      <a-table
        row-key="id"
        size="small"
        :columns="versionColumns"
        :data-source="versionGroup?.versions || []"
        :pagination="false"
      >
        <template #bodyCell="{ column, record }">
          <template v-if="column.key === 'version'">
            <span class="mi-mono">{{ record.version }}</span>
            <a-tag v-if="record.latest" color="gold" class="mi-version-latest">{{ t('cards.latestTag') }}</a-tag>
          </template>
          <template v-else-if="column.key === 'state'">
            <StatusTag kind="version" :status="record.state" />
          </template>
          <template v-else-if="column.key === 'size'">{{ formatBytes(record.size_bytes) }}</template>
          <template v-else-if="column.key === 'created_at'">
            <TimeText :value="record.created_at" />
          </template>
          <template v-else-if="column.key === 'action'">
            <a-button
              v-if="versionGroup"
              type="text"
              size="small"
              danger
              @click="onDelete(versionGroup, record)"
            >
              <DeleteOutlined />
              {{ t('common.delete') }}
            </a-button>
          </template>
        </template>
        <template #emptyText><a-empty :description="t('cards.noVersions')" /></template>
      </a-table>
      <div class="mi-modal-footer">
        <a-button @click="versionModalOpen = false">{{ t('common.close') }}</a-button>
      </div>
    </a-modal>
  </PageShell>
</template>

<style scoped>
.mi-toolbar {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  margin-bottom: 16px;
}
.mi-card-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
  gap: 12px;
}

/* ---------- 家族主题卡片 ---------- */
.mi-mcard {
  position: relative;
  overflow: hidden;
  border-radius: var(--mi-radius-lg);
  border: 1px solid var(--mi-border);
  background: var(--mi-surface);
  box-shadow: var(--mi-shadow-sm);
  transition: box-shadow 0.18s ease, border-color 0.18s ease;
}
.mi-mcard:hover {
  box-shadow: var(--mi-shadow-md);
  border-color: var(--mi-card-border-hover);
}
.mi-mcard__tint {
  position: absolute;
  inset: 0;
  background: linear-gradient(135deg, var(--mi-card-tint) 0%, transparent 58%);
  pointer-events: none;
}
.mi-mcard__watermark {
  position: absolute;
  right: -56px;
  bottom: -72px;
  width: 300px;
  height: 300px;
  background: var(--mi-card-watermark) center / contain no-repeat;
  opacity: 0.08;
  pointer-events: none;
}
.mi-mcard__inner {
  position: relative;
  padding: 16px;
}
/* 让卡内表格透出卡片背景（家族色渐变 + 大图标水印）；保留行 hover 反馈 */
.mi-mcard__inner :deep(.ant-table),
.mi-mcard__inner :deep(.ant-table-container),
.mi-mcard__inner :deep(.ant-table-content),
.mi-mcard__inner :deep(.ant-table-thead > tr > th),
.mi-mcard__inner :deep(.ant-table-tbody > tr > td) {
  background: transparent;
}
.mi-mcard__inner :deep(.ant-table-tbody > tr:hover > td) {
  background: var(--mi-surface-2) !important;
}
.mi-mcard__inner :deep(.ant-table-thead > tr > th) {
  border-bottom-color: var(--mi-border);
}
.mi-mcard__head {
  display: flex;
  align-items: center;
  gap: 12px;
}
.mi-mcard__titles {
  min-width: 0;
  flex: 1;
}
.mi-mcard__name {
  font-size: var(--mi-font-card-title);
  font-weight: 600;
  color: var(--mi-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mi-mcard__id {
  margin-top: 3px;
  display: inline-flex;
}
.mi-mcard__latest {
  flex: none;
  margin-inline-end: 0;
}
.mi-mcard__tags {
  display: flex;
  align-items: center;
  flex-wrap: wrap;
  gap: 6px;
  margin: 12px 0 10px;
}
.mi-family-tag {
  color: var(--mi-card-accent);
  border-color: var(--mi-family-tag-border);
  background: var(--mi-family-tag-bg);
}
.mi-mcard__weight {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--mi-text-secondary);
}
.mi-mcard__stats {
  display: flex;
  gap: 16px;
  padding: 8px 12px;
  margin-bottom: 12px;
  border-radius: var(--mi-radius-md);
  background: var(--mi-surface-2);
  border: 1px solid var(--mi-border);
  font-size: 12px;
  color: var(--mi-text-secondary);
}
.mi-mcard__stat b {
  color: var(--mi-text);
  font-weight: 600;
}
.mi-tag-soft {
  color: var(--mi-text-secondary);
  background: var(--mi-surface-2);
  border-color: var(--mi-border);
}
.mi-mcard__version {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  border-top: 1px dashed var(--mi-border);
  padding-top: 10px;
}
.mi-mcard__label {
  color: var(--mi-text-secondary);
  font-size: 12px;
}
.mi-mcard__actions {
  display: flex;
  justify-content: flex-end;
  margin-top: 10px;
}
.mi-modal-footer {
  display: flex;
  justify-content: flex-end;
  margin-top: 16px;
}
.mi-version-latest {
  margin-left: 6px;
  transform: scale(0.9);
}
.mi-footer-hint {
  margin-top: 12px;
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
  text-align: right;
}
</style>