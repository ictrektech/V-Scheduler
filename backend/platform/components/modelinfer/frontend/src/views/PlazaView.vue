<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { useI18n } from 'vue-i18n';
import { message, Modal } from 'ant-design-vue';
import {
  DeleteOutlined,
  EditOutlined,
  PlusOutlined,
  ReloadOutlined,
  SearchOutlined,
  SettingOutlined,
} from '@ant-design/icons-vue';
import PageShell from '@/components/PageShell.vue';
import StatPill from '@/components/StatPill.vue';
import TimeText from '@/components/TimeText.vue';
import CopyText from '@/components/CopyText.vue';
import ModelAvatar from '@/components/ModelAvatar.vue';
import {
  createPlazaSource,
  deletePlazaSource,
  displayText,
  fetchCardManifest,
  listPlazaCards,
  listPlazaSources,
  registerPlazaModel,
  updatePlazaSource,
  type PlazaCardGroup,
  type PlazaCardStats,
  type PlazaCardVersion,
  type PlazaSourceItem,
} from '@/api/plaza';
import { GRID_PAGE_SIZE, PAGE_SIZE_OPTIONS } from '@/api/paging';
import { formatBytes, formatSeconds } from '@/utils/format';
import { hexToRgba, modelFamilyIcon, modelFamilyTheme } from '@/utils/modelFamily';
import { modelIconUrl } from '@/utils/modelIconAssets';
import { parseManifest, type ManifestView } from '@/utils/manifest';
import { copyToClipboard } from '@/utils/clipboard';
import { createSeqGuard, debounce } from '@/utils/async';
import { reportApiError } from '@/i18n/api-error';

const { t } = useI18n();

const loading = ref(false);
const sources = ref<PlazaSourceItem[]>([]);
const activeSourceId = ref<string>('');
const cards = ref<PlazaCardGroup[]>([]);
/** 未筛选统计（页头统计条） */
const cardStats = ref<PlazaCardStats>({ cards: 0, downloaded: 0 });
/** 服务端分页元数据（`total` 为筛选后卡片数） */
const page = reactive({ page_num: 1, page_size: GRID_PAGE_SIZE, total: 0 });
const seqGuard = createSeqGuard();
const cardsLoading = ref(false);
/** 正在下载登记的版本主键 */
const registering = ref<string>('');
/** 工具条筛选 */
const filters = reactive({ keyword: '', registeredOnly: false });

/** 来源管理弹窗（列表不常驻页面） */
const sourceManagerOpen = ref(false);
/** 新增 / 编辑来源表单弹窗 */
const sourceModalOpen = ref(false);
const editingSourceId = ref<string | null>(null);
const saving = ref(false);
const sourceForm = reactive({ name: '', base_url: '', enabled: true });

/** 版本选择弹窗（下载时选择具体版本，避免卡片纵向膨胀） */
const versionModalOpen = ref(false);
const versionCard = ref<PlazaCardGroup | null>(null);
const versionSelected = ref<string>('');

const manifestModalOpen = ref(false);
const manifestText = ref('');
const manifestTitle = ref('');
/** manifest 结构化视图（解析失败为 null，回落展示原文） */
const manifestView = ref<ManifestView | null>(null);

const imageColumns = computed(() => [
  { title: t('plaza.manifest.columns.image.name'), key: 'name' },
  { title: t('plaza.manifest.columns.image.arch'), key: 'arch', width: 120 },
]);
const configColumns = computed(() => [
  { title: t('plaza.manifest.columns.config.name'), key: 'name', width: 180 },
  { title: t('plaza.manifest.columns.config.title'), key: 'title', width: 150 },
  { title: t('plaza.manifest.columns.config.type'), key: 'type', width: 90 },
  { title: t('plaza.manifest.columns.config.defaultValue'), key: 'defaultValue', width: 160 },
  { title: t('plaza.manifest.columns.config.constraint'), key: 'constraint', width: 200 },
  { title: t('plaza.manifest.columns.config.description'), key: 'description' },
]);
const lifecycleColumns = computed(() => [
  { title: t('plaza.manifest.columns.lifecycle.stage'), key: 'stage', width: 150 },
  { title: t('plaza.manifest.columns.lifecycle.script'), key: 'script' },
  { title: t('plaza.manifest.columns.lifecycle.order'), key: 'order', width: 180 },
  { title: t('plaza.manifest.columns.lifecycle.timeout'), key: 'timeout', width: 100 },
]);

const activeSource = computed(() => sources.value.find((s) => s.id === activeSourceId.value) || null);

const sourceColumns = computed(() => [
  { title: t('plaza.sourceManager.columns.current'), key: 'current', width: 70 },
  { title: t('plaza.sourceManager.columns.name'), dataIndex: 'name', key: 'name', width: 160 },
  { title: t('plaza.sourceManager.columns.address'), key: 'base_url' },
  { title: t('plaza.sourceManager.columns.cardCount'), dataIndex: 'card_count', key: 'card_count', width: 90 },
  { title: t('plaza.sourceManager.columns.lastSynced'), key: 'last_synced_at', width: 150 },
  { title: t('plaza.sourceManager.columns.action'), key: 'action', width: 110, align: 'right' as const },
]);

// ---------- 统计（来源数取来源列表；卡片 / 已下载取服务端未筛选 stats） ----------
const stats = computed(() => ({
  sources: sources.value.length,
  cards: cardStats.value.cards,
  downloaded: cardStats.value.downloaded,
}));

// ---------- 筛选（服务端） ----------
const isFiltering = computed(() => !!filters.keyword.trim() || filters.registeredOnly);

/** 最新版本条目 */
function latestOf(card: PlazaCardGroup): PlazaCardVersion | null {
  return card.versions.find((v) => v.version === card.latest_version) || card.versions[0] || null;
}

/** 是否所有版本都已下载 */
function allRegistered(card: PlazaCardGroup): boolean {
  return card.versions.length > 0 && card.versions.every((v) => v.registered);
}

/** 卡片家族主题（品牌色渐变 + 家族图标水印；家族标识来自后端） */
function cardStyle(card: PlazaCardGroup): Record<string, string> {
  const accent = modelFamilyTheme(card.family).accent;
  return {
    '--mi-card-accent': accent,
    '--mi-card-tint': hexToRgba(accent, 0.05),
    '--mi-card-watermark': `url("${modelIconUrl(modelFamilyIcon(card.family))}")`,
  };
}

async function loadSources() {
  try {
    const data = await listPlazaSources();
    sources.value = data.data;
    if (!activeSourceId.value && sources.value.length) {
      activeSourceId.value = sources.value[0].id;
      await loadCards();
    } else if (activeSourceId.value && !sources.value.some((s) => s.id === activeSourceId.value)) {
      activeSourceId.value = sources.value[0]?.id || '';
      await loadCards();
    }
  } catch (e) {
    reportApiError(e);
  }
}

/**
 * 加载当前来源的卡片（服务端分页 + 筛选）。
 * `refresh = true` 时绕过后端 TTL 缓存强制重拉上游索引（「刷新卡片」按钮）。
 */
async function loadCards(refresh = false) {
  if (!activeSourceId.value) {
    cards.value = [];
    cardStats.value = { cards: 0, downloaded: 0 };
    page.total = 0;
    return;
  }
  const seq = seqGuard.next();
  cardsLoading.value = true;
  try {
    const data = await listPlazaCards(activeSourceId.value, {
      page_num: page.page_num,
      page_size: page.page_size,
      keyword: filters.keyword.trim(),
      registered_only: filters.registeredOnly,
      refresh,
    });
    if (!seqGuard.isCurrent(seq)) return; // 丢弃过期响应
    cards.value = data.data;
    cardStats.value = data.stats;
    page.total = data.total;
    if (data.pages > 0 && page.page_num > data.pages) {
      page.page_num = data.pages;
      void loadCards(refresh);
    }
  } catch (e) {
    if (seqGuard.isCurrent(seq)) reportApiError(e);
  } finally {
    if (seqGuard.isCurrent(seq)) cardsLoading.value = false;
  }
}

/** 筛选变更：回到第 1 页后重新加载 */
function reloadFromFirstPage() {
  page.page_num = 1;
  void loadCards();
}

const debouncedKeywordReload = debounce(reloadFromFirstPage, 300);
watch(() => filters.keyword, () => debouncedKeywordReload());
watch(() => filters.registeredOnly, reloadFromFirstPage);
onBeforeUnmount(() => debouncedKeywordReload.cancel());

function onTableChange(p: { current?: number; pageSize?: number }) {
  if (p.pageSize && p.pageSize !== page.page_size) {
    page.page_size = p.pageSize;
    page.page_num = 1;
  } else if (p.current) {
    page.page_num = p.current;
  }
  void loadCards();
}

async function load() {
  loading.value = true;
  await loadSources();
  loading.value = false;
}

/** 在来源管理弹窗中选择当前来源（切换来源时回到第 1 页） */
async function selectSource(s: PlazaSourceItem) {
  activeSourceId.value = s.id;
  sourceManagerOpen.value = false;
  page.page_num = 1;
  await loadCards(true);
}

function openCreateSource() {
  editingSourceId.value = null;
  sourceForm.name = '';
  sourceForm.base_url = '';
  sourceForm.enabled = true;
  sourceModalOpen.value = true;
}

function openEditSource(s: PlazaSourceItem) {
  editingSourceId.value = s.id;
  sourceForm.name = s.name;
  sourceForm.base_url = s.base_url;
  sourceForm.enabled = s.enabled;
  sourceModalOpen.value = true;
}

async function submitSource() {
  if (!sourceForm.name || !sourceForm.base_url) {
    message.warning(t('plaza.sourceForm.nameAddressRequired'));
    return;
  }
  saving.value = true;
  try {
    if (editingSourceId.value) {
      await updatePlazaSource(editingSourceId.value, { ...sourceForm });
      message.success(t('plaza.sourceForm.updated'));
    } else {
      const created = await createPlazaSource({ ...sourceForm });
      message.success(t('plaza.sourceForm.created'));
      activeSourceId.value = created.id;
    }
    sourceModalOpen.value = false;
    await loadSources();
    await loadCards();
  } catch (e) {
    reportApiError(e);
  } finally {
    saving.value = false;
  }
}

function onDeleteSource(s: PlazaSourceItem) {
  Modal.confirm({
    title: t('plaza.deleteSource.title'),
    content: t('plaza.deleteSource.content', { name: s.name, url: s.base_url }),
    okText: t('common.delete'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      await deletePlazaSource(s.id);
      message.success(t('plaza.deleteSource.deleted'));
      await loadSources();
      await loadCards();
    },
  });
}

/** 打开版本选择弹窗：默认选中最新的未下载版本（都下载了则选最新） */
function openVersions(card: PlazaCardGroup) {
  versionCard.value = card;
  const firstUnregistered = card.versions.find((v) => !v.registered);
  versionSelected.value = (firstUnregistered || latestOf(card))?.version || '';
  versionModalOpen.value = true;
}

/** 下载登记所选版本 */
async function confirmDownload() {
  const card = versionCard.value;
  const version = card?.versions.find((v) => v.version === versionSelected.value);
  if (!card || !version || !activeSourceId.value || version.registered) return;
  registering.value = `${card.id}@${version.version}`;
  try {
    await registerPlazaModel({
      source_id: activeSourceId.value,
      model_id: card.id,
      version: version.version,
    });
    message.success(t('plaza.download.registered', { model: card.id, version: version.version }));
    versionModalOpen.value = false;
    await loadCards();
  } catch (e) {
    reportApiError(e);
  } finally {
    registering.value = '';
  }
}

async function onShowManifest(card: PlazaCardGroup, version: PlazaCardVersion) {
  try {
    const data = await fetchCardManifest(activeSourceId.value, version.manifest);
    manifestText.value = data.manifest;
    manifestView.value = parseManifest(data.manifest);
    manifestTitle.value = `${card.id}@${version.version}`;
    manifestModalOpen.value = true;
  } catch (e) {
    reportApiError(e);
  }
}

async function copyManifest() {
  const ok = await copyToClipboard(manifestText.value);
  if (ok) message.success(t('common.copied'));
  else message.error(t('common.copyFailed'));
}

onMounted(load);
</script>

<template>
  <PageShell
    :title="t('plaza.title')"
    :subtitle="t('plaza.subtitle')"
    :loading="loading"
  >
    <template #stats>
      <StatPill :label="t('plaza.stats.sources')" :value="stats.sources" tone="primary" />
      <StatPill :label="t('plaza.stats.cards')" :value="stats.cards" />
      <StatPill :label="t('plaza.stats.downloadedVersions')" :value="stats.downloaded" />
    </template>

    <template #actions>
      <a-button @click="loadCards(true)">
        <ReloadOutlined />
        {{ t('plaza.actions.refreshCards') }}
      </a-button>
      <a-button type="primary" @click="sourceManagerOpen = true">
        <SettingOutlined />
        {{ t('plaza.actions.manageSources') }}
      </a-button>
    </template>

    <!-- 当前来源（仅展示当前使用的一个） -->
    <div v-if="sources.length" class="mi-current">
      <span class="mi-current__label">{{ t('plaza.currentSource.label') }}</span>
      <template v-if="activeSource">
        <span class="mi-strong">{{ activeSource.name }}</span>
        <CopyText :text="activeSource.base_url" secondary />
        <a-tag class="mi-tag-soft">{{ t('plaza.currentSource.cardsCount', { count: activeSource.card_count }) }}</a-tag>
        <span class="mi-hint mi-inline">{{ t('plaza.currentSource.lastSynced') }} <TimeText :value="activeSource.last_synced_at" /></span>
      </template>
      <span v-else class="mi-hint">{{ t('plaza.currentSource.none') }}</span>
      <a-button size="small" class="mi-current__btn" @click="sourceManagerOpen = true">{{ t('plaza.actions.switchSource') }}</a-button>
    </div>

    <a-card
      :title="activeSource ? t('plaza.cards.titleWithSource', { source: activeSource.name }) : t('plaza.cards.title')"
      size="small"
    >
      <template #extra>
        <a-input
          v-model:value="filters.keyword"
          :placeholder="t('plaza.cards.searchPlaceholder')"
          allow-clear
          size="small"
          style="width: 200px"
        >
          <template #prefix><SearchOutlined /></template>
        </a-input>
        <a-checkbox v-model:checked="filters.registeredOnly" style="margin-left: 12px">
          {{ t('plaza.cards.registeredOnly') }}
        </a-checkbox>
      </template>

      <a-spin :spinning="cardsLoading">
        <div v-if="cards.length" class="mi-card-grid">
          <div v-for="c in cards" :key="c.id" class="mi-plaza-card" :style="cardStyle(c)">
            <div class="mi-plaza-card__watermark" />
            <div class="mi-plaza-card__inner">
              <div class="mi-plaza-card__head">
                <ModelAvatar :family="c.family" :size="44" />
                <div class="mi-plaza-card__title">
                  <div class="mi-plaza-card__name" :title="displayText(c.name) || c.id">
                    {{ displayText(c.name) || c.id }}
                  </div>
                  <CopyText :text="c.id" secondary />
                </div>
                <a-tag v-if="c.registered_count" color="green">
                  {{ c.registered_count }}/{{ c.versions.length }}
                </a-tag>
              </div>

              <div class="mi-plaza-card__desc">{{ displayText(c.description) || t('plaza.cards.noDescription') }}</div>

              <div class="mi-plaza-card__meta">
                <a-tag v-for="tag in c.tags" :key="tag" class="mi-tag-soft">{{ tag }}</a-tag>
              </div>

              <div class="mi-plaza-card__version">
                <span class="mi-plaza-card__label">{{ t('plaza.cards.latestVersion') }}</span>
                <span class="mi-mono">{{ latestOf(c)?.version || '-' }}</span>
                <span class="mi-hint">{{ formatBytes(latestOf(c)?.size_bytes) }}</span>
                <a-tag :color="latestOf(c)?.registered ? 'green' : 'default'">
                  {{ latestOf(c)?.registered ? t('plaza.cards.registered') : t('plaza.cards.unregistered') }}
                </a-tag>
                <a-tag v-if="c.versions.length > 1" class="mi-tag-soft">{{ t('plaza.cards.versionsCount', { count: c.versions.length }) }}</a-tag>
              </div>

              <div class="mi-plaza-card__actions">
                <a-button size="small" :disabled="!latestOf(c)" @click="onShowManifest(c, latestOf(c)!)">
                  {{ t('common.detail') }}
                </a-button>
                <a-button
                  size="small"
                  type="primary"
                  :disabled="allRegistered(c)"
                  @click="openVersions(c)"
                >
                  {{ allRegistered(c) ? t('plaza.cards.allDownloaded') : t('plaza.cards.selectVersionDownload') }}
                </a-button>
              </div>
            </div>
          </div>
        </div>
        <a-empty
          v-else
          :description="
            !activeSourceId
              ? t('plaza.cards.emptyNoSource')
              : cardStats.cards
                ? t('plaza.cards.emptyNoMatch')
                : t('plaza.cards.emptyNoCards')
          "
        />
      </a-spin>

      <div v-if="page.total > 0" class="mi-list-pagination">
        <a-pagination
          :current="page.page_num"
          :page-size="page.page_size"
          :total="page.total"
          :show-size-changer="true"
          :page-size-options="PAGE_SIZE_OPTIONS"
          :show-total="(total: number) => t('plaza.cards.total', { total })"
          @change="(c: number, ps: number) => onTableChange({ current: c, pageSize: ps })"
        />
        <span v-if="isFiltering" class="mi-hint">
          {{ t('plaza.cards.filtered', { shown: page.total, total: cardStats.cards }) }}
        </span>
      </div>
    </a-card>

    <!-- 来源管理弹窗 -->
    <a-modal v-model:open="sourceManagerOpen" :title="t('plaza.sourceManager.title')" :footer="null" width="760px">
      <a-table
        row-key="id"
        size="small"
        :columns="sourceColumns"
        :data-source="sources"
        :pagination="false"
      >
        <template #bodyCell="{ column, record }">
          <template v-if="column.key === 'current'">
            <a-radio :checked="record.id === activeSourceId" @change="selectSource(record)" />
          </template>
          <template v-else-if="column.key === 'base_url'">
            <CopyText :text="record.base_url" secondary />
          </template>
          <template v-else-if="column.key === 'last_synced_at'">
            <TimeText :value="record.last_synced_at" />
          </template>
          <template v-else-if="column.key === 'action'">
            <a-space :size="2">
              <a-tooltip :title="t('common.edit')">
                <a-button size="small" type="text" @click="openEditSource(record)">
                  <EditOutlined />
                </a-button>
              </a-tooltip>
              <a-tooltip :title="t('common.delete')">
                <a-button size="small" type="text" danger @click="onDeleteSource(record)">
                  <DeleteOutlined />
                </a-button>
              </a-tooltip>
            </a-space>
          </template>
        </template>
        <template #emptyText><a-empty :description="t('plaza.sourceManager.empty')" /></template>
      </a-table>
      <div class="mi-source-footer">
        <a-button type="primary" @click="openCreateSource">
          <PlusOutlined />
          {{ t('plaza.sourceManager.add') }}
        </a-button>
        <a-button @click="sourceManagerOpen = false">{{ t('common.close') }}</a-button>
      </div>
    </a-modal>

    <!-- 版本选择弹窗（下载） -->
    <a-modal
      v-model:open="versionModalOpen"
      :title="t('plaza.versionModal.title', { id: versionCard?.id || '' })"
      :confirm-loading="!!registering"
      :ok-text="t('plaza.versionModal.downloadRegister')"
      :cancel-text="t('common.cancel')"
      width="620px"
      @ok="confirmDownload"
    >
      <div class="mi-hint" style="margin-bottom: 8px">
        {{ t('plaza.versionModal.summary', { count: versionCard?.versions.length || 0 }) }}
      </div>
      <a-radio-group v-model:value="versionSelected" class="mi-vm">
        <a-radio
          v-for="v in versionCard?.versions || []"
          :key="v.version"
          :value="v.version"
          :disabled="v.registered"
          class="mi-vm__item"
        >
          <span class="mi-vm__row">
            <span class="mi-mono">{{ v.version }}</span>
            <span class="mi-hint">{{ formatBytes(v.size_bytes) }}</span>
            <a-tag :color="v.registered ? 'green' : 'default'">{{ v.registered ? t('plaza.cards.registered') : t('plaza.cards.unregistered') }}</a-tag>
            <a class="mi-vm__detail" @click.stop="versionCard && onShowManifest(versionCard, v)">{{ t('common.detail') }}</a>
          </span>
        </a-radio>
      </a-radio-group>
      <a-empty v-if="!(versionCard?.versions || []).length" :description="t('plaza.versionModal.empty')" />
    </a-modal>

    <!-- 新增 / 编辑来源 -->
    <a-modal
      v-model:open="sourceModalOpen"
      :title="editingSourceId ? t('plaza.sourceForm.editTitle') : t('plaza.sourceForm.addTitle')"
      :confirm-loading="saving"
      :ok-text="t('common.confirm')"
      :cancel-text="t('common.cancel')"
      @ok="submitSource"
    >
      <a-form layout="vertical">
        <a-form-item :label="t('plaza.sourceForm.name')" required>
          <a-input v-model:value="sourceForm.name" :placeholder="t('plaza.sourceForm.namePlaceholder')" />
        </a-form-item>
        <a-form-item :label="t('plaza.sourceForm.address')" required>
          <a-input v-model:value="sourceForm.base_url" :placeholder="t('plaza.sourceForm.addressPlaceholder')" />
        </a-form-item>
        <a-form-item :label="t('plaza.sourceForm.enabled')">
          <a-switch v-model:checked="sourceForm.enabled" />
        </a-form-item>
      </a-form>
    </a-modal>

    <a-modal
      v-model:open="manifestModalOpen"
      :title="t('plaza.manifest.title', { id: manifestTitle })"
      :footer="null"
      width="920px"
    >
      <div class="mi-manifest-toolbar">
        <a-button size="small" @click="copyManifest">{{ t('plaza.manifest.copyRaw') }}</a-button>
      </div>

      <template v-if="manifestView">
        <div class="mi-section-title">{{ t('plaza.manifest.basicInfo') }}</div>
        <a-descriptions size="small" :column="2" bordered>
          <a-descriptions-item :label="t('plaza.manifest.fields.id')" :span="2">
            <CopyText :text="manifestView.id || '-'" />
          </a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.version')">{{ manifestView.version || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.name')">{{ manifestView.name || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.author')">{{ manifestView.author || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.arch')">{{ manifestView.arch || 'any' }}</a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.weightId')" :span="2">
            <CopyText :text="manifestView.model || '-'" />
          </a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.category')">
            <a-tag v-for="c in manifestView.category" :key="c" class="mi-tag-soft">{{ c }}</a-tag>
            <span v-if="!manifestView.category.length">-</span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.tags')">
            <a-tag v-for="tag in manifestView.tags" :key="tag" class="mi-tag-soft">{{ tag }}</a-tag>
            <span v-if="!manifestView.tags.length">-</span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('plaza.manifest.fields.maxNodes')">
            <a-tag :color="manifestView.maxNodes === t('common.noLimit') ? 'default' : 'blue'">
              {{ manifestView.maxNodes === t('common.noLimit') ? manifestView.maxNodes : t('plaza.manifest.maxNodes', { count: manifestView.maxNodes }) }}
            </a-tag>
          </a-descriptions-item>
        </a-descriptions>

        <div class="mi-section-title" style="margin-top: 16px">{{ t('plaza.manifest.imagesSection') }}</div>
        <a-table
          row-key="name"
          size="small"
          :columns="imageColumns"
          :data-source="manifestView.images"
          :pagination="false"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">
              <CopyText :text="record.name" />
            </template>
          </template>
          <template #emptyText><a-empty :description="t('plaza.manifest.imagesEmpty')" /></template>
        </a-table>

        <div class="mi-section-title" style="margin-top: 16px">
          {{ t('plaza.manifest.configsSection', { count: manifestView.configs.length }) }}
        </div>
        <a-table
          row-key="name"
          size="small"
          :columns="configColumns"
          :data-source="manifestView.configs"
          :pagination="false"
          :scroll="{ x: 960 }"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">
              <span class="mi-mono">{{ record.name }}</span>
            </template>
            <template v-else-if="column.key === 'title'">
              {{ record.title || '-' }}
              <a-tag v-if="record.required" color="orange" class="mi-req">{{ t('plaza.manifest.required') }}</a-tag>
            </template>
            <template v-else-if="column.key === 'defaultValue'">
              <span class="mi-mono">{{ record.defaultValue }}</span>
            </template>
            <template v-else-if="column.key === 'constraint'">
              <span v-if="record.constraint">{{ record.constraint }}</span>
              <span v-else class="mi-hint">-</span>
            </template>
            <template v-else-if="column.key === 'description'">
              <a-tooltip v-if="record.description" :title="record.description">
                <span class="mi-ellipsis">{{ record.description }}</span>
              </a-tooltip>
              <span v-else class="mi-hint">-</span>
            </template>
          </template>
          <template #emptyText><a-empty :description="t('plaza.manifest.configsEmpty')" /></template>
        </a-table>

        <div class="mi-section-title" style="margin-top: 16px">
          {{ t('plaza.manifest.lifecycleSection', { count: manifestView.lifecycle.length }) }}
        </div>
        <a-table
          row-key="stage"
          size="small"
          :columns="lifecycleColumns"
          :data-source="manifestView.lifecycle"
          :pagination="false"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'script'">
              <a-tooltip v-if="record.script" :title="record.script">
                <span class="mi-mono mi-ellipsis">{{ record.script }}</span>
              </a-tooltip>
              <span v-else class="mi-hint">-</span>
            </template>
            <template v-else-if="column.key === 'order'">
              <a-tag class="mi-tag-soft">{{ record.order }}</a-tag>
            </template>
            <template v-else-if="column.key === 'timeout'">
              <span v-if="record.timeout" class="mi-mono">{{ formatSeconds(record.timeout) }}</span>
              <span v-else class="mi-hint">{{ t('plaza.manifest.defaultTimeout') }}</span>
            </template>
          </template>
          <template #emptyText><a-empty :description="t('plaza.manifest.lifecycleEmpty')" /></template>
        </a-table>
      </template>

      <template v-else>
        <a-alert type="warning" show-icon :message="t('plaza.manifest.parseFailed')" style="margin-bottom: 8px" />
        <pre class="mi-manifest">{{ manifestText }}</pre>
      </template>
    </a-modal>
  </PageShell>
</template>

<style scoped>
.mi-strong {
  font-weight: 600;
}
.mi-inline {
  display: inline-flex;
  align-items: center;
  gap: 4px;
}
.mi-tag-soft {
  color: var(--mi-text-secondary);
  background: var(--mi-surface-2);
  border-color: var(--mi-border);
}

/* ---------- 当前来源条 ---------- */
.mi-current {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-bottom: 16px;
  padding: 12px 16px;
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-lg);
  background: var(--mi-surface);
  box-shadow: var(--mi-shadow-sm);
}
.mi-current__label {
  font-size: 12px;
  color: var(--mi-text-secondary);
}
.mi-current__btn {
  margin-left: auto;
}

/* ---------- 卡片网格 ---------- */
.mi-card-grid {
  display: grid;
  grid-template-columns: repeat(auto-fit, minmax(320px, 1fr));
  gap: 12px;
}
.mi-plaza-card {
  position: relative;
  overflow: hidden;
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-lg);
  background: var(--mi-surface);
  box-shadow: var(--mi-shadow-sm);
  transition: box-shadow 0.18s ease, border-color 0.18s ease;
}
.mi-plaza-card:hover {
  box-shadow: var(--mi-shadow-md);
  border-color: var(--mi-border-strong);
}
.mi-plaza-card__watermark {
  position: absolute;
  right: -26px;
  bottom: -30px;
  width: 168px;
  height: 168px;
  background: var(--mi-card-watermark) center / contain no-repeat;
  opacity: 0.08;
  pointer-events: none;
}
.mi-plaza-card__inner {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  padding: 14px;
  background: linear-gradient(135deg, var(--mi-card-tint) 0%, transparent 55%);
  min-height: 100%;
}
.mi-plaza-card__head {
  display: flex;
  align-items: center;
  gap: 10px;
}
.mi-plaza-card__title {
  flex: 1;
  min-width: 0;
}
.mi-plaza-card__name {
  font-weight: 600;
  color: var(--mi-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}
.mi-plaza-card__desc {
  font-size: 13px;
  color: var(--mi-text-secondary);
  min-height: 38px;
  margin: 10px 0 8px;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}
.mi-plaza-card__meta {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  margin-bottom: 8px;
}
.mi-plaza-card__version {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  border-top: 1px dashed var(--mi-border);
  padding-top: 8px;
}
.mi-plaza-card__label {
  color: var(--mi-text-secondary);
  font-size: 12px;
}
.mi-plaza-card__actions {
  display: flex;
  gap: 8px;
  justify-content: flex-end;
  margin-top: 10px;
}

/* ---------- 版本选择弹窗 ---------- */
.mi-vm {
  display: flex;
  flex-direction: column;
  gap: 4px;
  width: 100%;
}
.mi-vm__item {
  display: flex;
  align-items: center;
  width: 100%;
  padding: 6px 8px;
  border-radius: var(--mi-radius-sm);
}
.mi-vm__row {
  display: inline-flex;
  align-items: center;
  gap: 10px;
}
.mi-vm__detail {
  margin-left: auto;
}

.mi-source-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 16px;
}

.mi-manifest-toolbar {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 8px;
}
.mi-section-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--mi-text);
  margin-bottom: 8px;
}
.mi-ellipsis {
  display: inline-block;
  max-width: 100%;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  vertical-align: bottom;
}
.mi-req {
  margin-left: 6px;
}
.mi-manifest {
  white-space: pre-wrap;
  word-break: break-all;
  max-height: 480px;
  overflow: auto;
  background: var(--mi-surface-2);
  border: 1px solid var(--mi-border);
  padding: 12px;
  border-radius: var(--mi-radius-md);
  font-family: 'SFMono-Regular', 'JetBrains Mono', Menlo, Consolas, monospace;
  font-size: 12px;
  line-height: 1.55;
}
</style>