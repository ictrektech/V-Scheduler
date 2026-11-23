<script setup lang="ts">
import { getSchedulerRuntime } from '@scheduler-auth';
import { computed, onBeforeUnmount, onMounted, onUnmounted, reactive, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { message, Modal } from 'ant-design-vue';
import { DownOutlined, PlusOutlined, ReloadOutlined, SearchOutlined } from '@ant-design/icons-vue';
import PageShell from '@/components/PageShell.vue';
import StatPill from '@/components/StatPill.vue';
import StatusTag from '@/components/StatusTag.vue';
import TimeText from '@/components/TimeText.vue';
import CopyText from '@/components/CopyText.vue';
import ModelAvatar from '@/components/ModelAvatar.vue';
import StartResourcesModal from '@/components/StartResourcesModal.vue';
import { missingStartResources, type MissingStartResources } from '@/utils/startResources';
import {
  createDeployment,
  deleteDeployment,
  listDeployments,
  startDeployment,
  stopDeployment,
  updateDeployment,
  type DeploymentItem,
  type DeploymentCreateReq,
  type DeploymentStats,
  type ImageEntry,
} from '@/api/deployments';
import { listPlazaModels, displayText, type LocalModelGroup, type LocalModelVersion } from '@/api/plaza';
import { fetchModelHubInfo, matchModelVariants, type ModelHubVariant } from '@/api/modelHub';
import { listNodes, type NodeItem } from '@/api/nodes';
import { DEFAULT_PAGE_SIZE, PAGE_SIZE_OPTIONS } from '@/api/paging';
import { formatBytes, formatTime } from '@/utils/format';
import { modelFamilyIcon } from '@/utils/modelFamily';
import { modelIconUrl } from '@/utils/modelIconAssets';
import { createSeqGuard, debounce } from '@/utils/async';
import {
  IMAGE_ARCH_VALUES,
  MAX_EXTRA_ENV,
  MAX_IMAGES_OVERRIDE,
  envMapToRows,
  envRowsToMap,
  fromEditableImages,
  imageEntryLabel,
  imagesOverflow,
  parseWeightOverrideValue,
  toEditableImages,
  validateEnvRows,
  weightOptions,
  weightOverrideValue,
  type EditableImage,
  type EnvKeyError,
  type EnvRow,
} from '@/utils/runtimeOverrides';
import {
  DEFAULT_FAIL_MINUTES,
  MAX_FAIL_MINUTES,
  MIN_FAIL_MINUTES,
  allVersionsLackHealthProbe,
  clampMinutes,
  minutesToSecs,
  secsToMinutes,
} from '@/utils/autoRestart';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import {
  MISSING_NODE_STATUS,
  fallbackHead,
  hasMissingNode,
  missingNodeIds,
  splitNodeSelection,
} from '@/utils/nodeRefs';
import { localizedTitle } from '@/platform/localized-title';
import { openDetailInNewTab } from '@/platform/open-tab';

const loading = ref(false);
/** 当前页运行配置（服务端分页） */
const rows = ref<DeploymentItem[]>([]);
/** 未筛选全量统计（页头统计条） */
const stats = ref<DeploymentStats>({ total: 0, running: 0, stopped: 0, failed: 0 });
/** 服务端分页元数据（`total` 为筛选后条数） */
const page = reactive({ page_num: 1, page_size: DEFAULT_PAGE_SIZE, total: 0 });
const seqGuard = createSeqGuard();
/** 最近一次列表刷新时间（供「最后更新」提示） */
const lastUpdated = ref('');
let timer: number | undefined;
const router = useRouter();
const { t, locale } = useI18n();

/** 工具条筛选 / 排序 */
const filters = reactive({
  keyword: '',
  state: 'all',
  card: 'all',
  sort: 'name' as 'name' | 'recent' | 'state',
});

const modalOpen = ref(false);
const saving = ref(false);
const startingIds = ref(new Set<string>());
const resourcesOpen = ref(false);
const resourcesRun = ref<DeploymentItem | null>(null);
const missingResources = ref<MissingStartResources | null>(null);
/** 编辑中的配置 id（null = 新建） */
const editingId = ref<string | null>(null);
/** 本地模型卡片（按 card_id 聚合，含全部版本） */
const cards = ref<LocalModelGroup[]>([]);
const nodes = ref<NodeItem[]>([]);
const variants = ref<ModelHubVariant[]>([]);
const variantsLoading = ref(false);
/** model-hub 已下载变体（权重覆盖下拉） */
const hubModels = ref<ModelHubVariant[]>([]);

const form = reactive({
  name: '',
  /** 模型卡片 id（聚合分组） */
  card_id: '',
  /** 选定版本 */
  version: '',
  hub_source: '',
  node_ids: [] as string[],
  head_node_id: '',
  configs: {} as Record<string, any>,
  /** 权重覆盖下拉值（`source:model_id`；空 = 使用卡片默认权重） */
  weight_override_value: '' as string,
  /** 镜像覆盖行（空 = 使用卡片 manifest.images） */
  images_override: [] as EditableImage[],
  /** 额外环境变量行（在卡片 configs 之后注入） */
  extra_env: [] as EnvRow[],
  /** 标准接入参数（留空 = 平台自动推断） */
  serve_model_name: '',
  serve_api_key: '',
  serve_port: undefined as number | undefined,
  serve_url_prefix: '',
  /** 本地反代（本机 TCP 端口 → head vLLM，快速调试；默认关闭） */
  local_proxy_enabled: false,
  local_proxy_port: undefined as number | undefined,
  /** 探活失败自动重启（默认关闭）与连续失败阈值（分钟，默认 5） */
  auto_restart_enabled: false,
  auto_restart_fail_minutes: DEFAULT_FAIL_MINUTES,
});

const workspace = Boolean(getSchedulerRuntime());
const columns = computed(() => {
  void locale.value;
  return [
    { title: t('runs.columns.name'), key: 'name', width: workspace ? 160 : 210 },
    { title: t('runs.columns.model'), key: 'model', width: workspace ? 205 : 290 },
    { title: t('runs.columns.nodeCount'), key: 'node_count', width: workspace ? 68 : 90 },
    { title: t('runs.columns.headNode'), key: 'head_node', width: workspace ? 120 : 150 },
    { title: t('runs.columns.state'), key: 'state', width: workspace ? 90 : 110 },
    { title: t('runs.columns.health'), key: 'health_status', width: workspace ? 84 : 100 },
    { title: t('runs.columns.updatedAt'), key: 'updated_at', width: workspace ? 130 : 150 },
    { title: t('runs.columns.action'), key: 'action', width: workspace ? 204 : 230 },
  ];
});

const stateOptions = computed(() => {
  void locale.value;
  return [
    { label: t('runs.state.all'), value: 'all' },
    { label: t('runs.state.running'), value: 'running' },
    { label: t('runs.state.starting'), value: 'starting' },
    { label: t('runs.state.stopping'), value: 'stopping' },
    { label: t('runs.state.stopped'), value: 'stopped' },
    { label: t('runs.state.degraded'), value: 'degraded' },
    { label: t('runs.state.failed'), value: 'failed' },
  ];
});

const sortOptions = computed(() => {
  void locale.value;
  return [
    { label: t('runs.sort.name'), value: 'name' },
    { label: t('runs.sort.recent'), value: 'recent' },
    { label: t('runs.sort.state'), value: 'state' },
  ];
});

/** 提交给后端的已登记模型主键 `<card_id>@<version>` */
const plazaModelId = computed(() =>
  form.card_id && form.version ? `${form.card_id}@${form.version}` : '',
);
const selectedCard = computed(() => cards.value.find((c) => c.model_id === form.card_id) || null);
/** 卡片是否未声明权重（如 ComfyUI）：无需选择模型变体 */
const isWeightless = computed(() => !(selectedCard.value?.model || '').trim());
const selectedVersion = computed<LocalModelVersion | null>(() => {
  const g = selectedCard.value;
  if (!g) return null;
  return g.versions.find((v) => v.version === form.version) || null;
});
const cardOptions = computed(() => {
  void locale.value;
  return cards.value.map((c) => ({
    label: t('runs.cardOption', {
      name: c.name || c.model_id,
      model: c.model || '-',
      count: c.versions.length,
    }),
    value: c.model_id,
  }));
});
const versionOptions = computed(() => {
  void locale.value;
  return (selectedCard.value?.versions ?? [])
    .filter((v) => v.state === 'available')
    .map((v) => ({
      label: t('runs.versionOption', { version: v.version, size: formatBytes(v.size_bytes) }),
      value: v.version,
    }));
});
const configSchema = computed<Array<Record<string, unknown>>>(() => {
  const card = selectedVersion.value;
  if (!card) return [];
  return configSchemaSpecs(card);
});
/** 模型卡片 `caps.max-nodes`：单次部署最大节点数（0 = 不限制） */
const capsMaxNodes = computed<number>(() => capsMaxNodesOf(selectedVersion.value));
const nodeLimitHint = computed(() => {
  void locale.value;
  if (capsMaxNodes.value <= 0) return '';
  return t('runs.nodeLimitHint', { max: capsMaxNodes.value, selected: form.node_ids.length });
});
/** 所选卡片全部版本都未声明 health-probe 时提示自动重启不会生效 */
const cardLacksHealthProbe = computed(() =>
  allVersionsLackHealthProbe(selectedCard.value?.versions ?? []),
);

/** 卡片默认权重 id（manifest `model`） */
const cardDefaultWeight = computed(() => (selectedCard.value?.model || '').trim());
/** 卡片默认镜像（从所选版本 manifest 解析） */
const cardDefaultImages = computed<ImageEntry[]>(() => {
  const v = selectedVersion.value;
  if (!v) return [];
  try {
    const manifest = JSON.parse(v.manifest_json || '{}');
    return Array.isArray(manifest.images) ? (manifest.images as ImageEntry[]) : [];
  } catch {
    return [];
  }
});
/** 权重覆盖下拉：model-hub 已下载变体（`source:model_id`，按来源前缀区分 hf / ms） */
const weightOverrideOptions = computed(() => {
  void locale.value;
  return weightOptions(hubModels.value, (source, model) =>
    t('runs.form.weightOption', { source, model }),
  );
});
/** 额外环境变量校验（与后端同规则；后端为权威层） */
const extraEnvErrors = computed(() => validateEnvRows(form.extra_env, cardConfigNames.value));
const imagesOverflowed = computed(() => imagesOverflow(form.images_override));
const imageArchOptions = computed(() => {
  void locale.value;
  return IMAGE_ARCH_VALUES.map((v) => ({
    value: v,
    label: v ? v : t('runs.form.archAny'),
  }));
});
/** 卡片 `configs` 参数名（用于额外环境变量的同名冲突校验） */
const cardConfigNames = computed(() =>
  configSchema.value.map((s) => String(s.name || '')).filter(Boolean),
);
const cardDefaultImageLabels = computed(() => cardDefaultImages.value.map((e) => imageEntryLabel(e)));

/** 额外环境变量名错误 → 本地化文案 */
function envErrorText(err?: EnvKeyError): string {
  switch (err) {
    case 'empty':
      return t('runs.form.extraEnvErrors.empty');
    case 'invalid':
      return t('runs.form.extraEnvErrors.invalid');
    case 'reserved':
      return t('runs.form.extraEnvErrors.reserved');
    case 'system':
      return t('runs.form.extraEnvErrors.system');
    case 'conflict':
      return t('runs.form.extraEnvErrors.conflict');
    case 'duplicate':
      return t('runs.form.extraEnvErrors.duplicate');
    case 'overflow':
      return t('runs.form.extraEnvErrors.overflow', { max: MAX_EXTRA_ENV });
    default:
      return '';
  }
}

function addImageRow() {
  if (form.images_override.length >= MAX_IMAGES_OVERRIDE) {
    message.warning(t('runs.form.imagesOverflow', { max: MAX_IMAGES_OVERRIDE }));
    return;
  }
  form.images_override.push({ name: '', arch: '' });
}

function removeImageRow(idx: number) {
  form.images_override.splice(idx, 1);
}

/** 清除镜像覆盖：回到卡片 `manifest.images` */
function resetImagesOverride() {
  form.images_override = [];
}

/** 清除权重覆盖：回到卡片 `manifest.model` */
function clearWeightOverride() {
  form.weight_override_value = '';
}

function addEnvRow() {
  form.extra_env.push({ key: '', value: '' });
}

function removeEnvRow(idx: number) {
  form.extra_env.splice(idx, 1);
}

function resetOverrideForm() {
  form.weight_override_value = '';
  form.images_override = [];
  form.extra_env = [];
}

const nodeOptions = computed(() => {
  void locale.value;
  return nodes.value.map((n) => {
    const selected = form.node_ids.includes(n.id);
    const reached = capsMaxNodes.value > 0 && form.node_ids.length >= capsMaxNodes.value;
    return {
      label: t('runs.nodeHostLabel', { name: n.hostname || n.name, host: n.host }),
      value: n.id,
      disabled: !selected && reached,
    };
  });
});
const headOptions = computed(() => {
  void locale.value;
  return nodes.value
    .filter((n) => form.node_ids.includes(n.id))
    .map((n) => ({ label: t('runs.nodeHostLabel', { name: n.hostname || n.name, host: n.host }), value: n.id }));
});
const modalTitle = computed(() => {
  void locale.value;
  return editingId.value ? t('runs.editConfig') : t('runs.newConfig');
});

/** 节点是否已被删除（历史悬挂引用） */
function isMissingNode(node: { status: string }): boolean {
  return node.status === MISSING_NODE_STATUS;
}

/** 运行配置中已删除的节点 id（兼容旧后端：按节点状态回退） */
function deletedNodeIds(row: DeploymentItem): string[] {
  return missingNodeIds(row);
}

/** 某行是否可启动：主操作为「启动」且无失效节点；节点可被多个运行配置共享 */
function canStartRow(row: DeploymentItem): boolean {
  return primaryAction(row) === 'start' && !hasMissingNode(row);
}

/** 启动按钮 tooltip：仅「存在已删除节点」时需要提示 */
function startHint(row: DeploymentItem): string {
  return deletedNodeIds(row).length ? t('runs.startBlockedByMissingNode') : '';
}

// ---------- 工具条（筛选走服务端；卡片下拉取自已加载的本地卡片） ----------
const cardFilterOptions = computed(() => {
  void locale.value;
  return [
    { label: t('runs.cardFilter.all'), value: 'all' },
    ...cards.value.map((c) => ({ label: c.name || c.model_id, value: c.model_id })),
  ];
});

const isFiltering = computed(
  () => !!filters.keyword.trim() || filters.state !== 'all' || filters.card !== 'all',
);

/** 分页器配置（服务端分页） */
const pagination = computed(() => {
  void locale.value;
  return {
    current: page.page_num,
    pageSize: page.page_size,
    total: page.total,
    showSizeChanger: true,
    pageSizeOptions: PAGE_SIZE_OPTIONS,
    showTotal: (total: number) => t('common.totalCount', { total }),
  };
});

function onTableChange(p: { current?: number; pageSize?: number }) {
  if (p.pageSize && p.pageSize !== page.page_size) {
    page.page_size = p.pageSize;
    page.page_num = 1;
  } else if (p.current) {
    page.page_num = p.current;
  }
  void load();
}

/** 表格中的主操作：活跃态给「停止」，其余给「启动」 */
function primaryAction(row: DeploymentItem): 'start' | 'stop' {
  return ['starting', 'running', 'stopping', 'degraded'].includes(row.state) ? 'stop' : 'start';
}

/** head 节点（优先 role=head，否则首个节点） */
function headNodeOf(row: DeploymentItem) {
  return row.nodes.find((n) => n.role === 'head') || row.nodes[0] || null;
}

/** 每行右侧背景水印：该行模型的浅色家族大图标 */
function rowMarkStyle(row: DeploymentItem): Record<string, string> {
  return { backgroundImage: `url("${modelIconUrl(modelFamilyIcon(row.family))}")` };
}

function onMoreMenu(e: { key: string | number }, row: DeploymentItem) {
  switch (String(e.key)) {
    case 'edit':
      onEdit(row);
      break;
    case 'force-stop':
      onForceStop(row);
      break;
    case 'delete':
      onDelete(row);
      break;
  }
}

/** 从版本的 manifest_json 提取 configs schema */
function configSchemaSpecs(version: LocalModelVersion): Array<Record<string, unknown>> {
  try {
    const manifest = JSON.parse(version.manifest_json || '{}');
    return Array.isArray(manifest.configs) ? manifest.configs : [];
  } catch {
    return [];
  }
}

/**
 * 从版本的 manifest_json 提取 `caps.max-nodes`（0 / 缺失 / 非法 = 不限制）。
 * 与后端 `deployments::validate::caps_max_nodes` 语义一致；后端为权威校验层。
 */
function capsMaxNodesOf(version: LocalModelVersion | null): number {
  if (!version) return 0;
  try {
    const manifest = JSON.parse(version.manifest_json || '{}');
    const raw = manifest?.caps?.['max-nodes'];
    if (typeof raw === 'number' && Number.isInteger(raw) && raw >= 0) return raw;
  } catch {
    /* manifest 解析失败按不限制处理（后端仍会强校验） */
  }
  return 0;
}

/** 按所选版本的 caps.max-nodes 裁剪既有节点选择（切换卡片 / 版本 / 编辑回填时调用） */
function trimNodesToLimit() {
  const max = capsMaxNodes.value;
  if (max <= 0 || form.node_ids.length <= max) return;
  form.node_ids = form.node_ids.slice(0, max);
  if (!form.node_ids.includes(form.head_node_id)) form.head_node_id = form.node_ids[0] || '';
  message.warning(t('runs.maxNodesTrimmed', { max }));
}

/** 应用所选版本的默认参数 */
function resetConfigs() {
  const out: Record<string, any> = {};
  const v = selectedVersion.value;
  if (v) {
    for (const spec of configSchemaSpecs(v)) {
      if (spec.default !== undefined) out[spec.name as string] = spec.default;
    }
  }
  form.configs = out;
}

/** 是否可编辑：仅非活跃态（未启动 / 启动失败） */
function isRowEditable(row: DeploymentItem): boolean {
  return ['stopped', 'failed'].includes(row.state) && row.desired_state !== 'stopping';
}

async function load() {
  const seq = seqGuard.next();
  try {
    const data = await listDeployments({
      page_num: page.page_num,
      page_size: page.page_size,
      keyword: filters.keyword.trim(),
      state: filters.state,
      card_id: filters.card,
      sort: filters.sort,
    });
    if (!seqGuard.isCurrent(seq)) return; // 丢弃过期响应
    rows.value = data.data;
    stats.value = data.stats;
    page.total = data.total;
    lastUpdated.value = new Date().toISOString();
    if (data.pages > 0 && page.page_num > data.pages) {
      page.page_num = data.pages;
      void load();
    }
  } catch (e) {
    if (seqGuard.isCurrent(seq)) message.error(resolveApiErrorMessage(e));
  }
}

/** 筛选 / 排序变更：回到第 1 页后重新加载 */
function reloadFromFirstPage() {
  page.page_num = 1;
  void load();
}

const debouncedKeywordReload = debounce(reloadFromFirstPage, 300);
watch(() => filters.keyword, () => debouncedKeywordReload());
watch(() => [filters.state, filters.card, filters.sort], reloadFromFirstPage);
onBeforeUnmount(() => debouncedKeywordReload.cancel());

async function loadRefs() {
  loading.value = true;
  try {
    // 下拉选项需要全量：节点以最大页大小拉取（上限 100，台账规模远小于此）
    const [c, n, hub] = await Promise.all([
      listPlazaModels(),
      listNodes({ page_size: 100 }),
      // model-hub 信息失败不阻塞表单（权重覆盖下拉为空，仍可使用卡片默认）
      fetchModelHubInfo().catch(() => null),
    ]);
    hubModels.value = hub?.models || [];
    // 仅保留存在可用版本的卡片
    cards.value = c.data.filter((g) => g.versions.some((v) => v.state === 'available'));
    nodes.value = n.data;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    loading.value = false;
  }
}

/** 按权重 id 匹配 model-hub 变体（缺省预选唯一变体） */
async function loadVariants(weightId: string) {
  form.hub_source = '';
  variants.value = [];
  if (!weightId) return;
  variantsLoading.value = true;
  try {
    const data = await matchModelVariants(weightId);
    variants.value = data.data;
    if (variants.value.length === 1) form.hub_source = variants.value[0].source;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    variantsLoading.value = false;
  }
}

/** 选择模型卡片：默认选中最新可用版本 */
async function onSelectCard(cardId: string) {
  const group = cards.value.find((c) => c.model_id === cardId);
  const latest = group?.versions.find((v) => v.state === 'available');
  form.version = latest?.version || '';
  resetConfigs();
  resetOverrideForm();
  trimNodesToLimit();
  await loadVariants(group?.model || '');
}

/** 切换版本：参数表单按该版本 manifest 重算 */
async function onSelectVersion() {
  resetConfigs();
  resetOverrideForm();
  trimNodesToLimit();
  await loadVariants(selectedCard.value?.model || '');
}

function openCreate() {
  editingId.value = null;
  form.name = '';
  form.card_id = '';
  form.version = '';
  form.hub_source = '';
  form.node_ids = [];
  form.head_node_id = '';
  form.configs = {};
  resetOverrideForm();
  form.serve_model_name = '';
  form.serve_api_key = '';
  form.serve_port = undefined;
  form.serve_url_prefix = '';
  form.local_proxy_enabled = false;
  form.local_proxy_port = undefined;
  form.auto_restart_enabled = false;
  form.auto_restart_fail_minutes = DEFAULT_FAIL_MINUTES;
  variants.value = [];
  modalOpen.value = true;
  void loadRefs();
}

/** 编辑未启动（非活跃）配置：由 plaza_model_id 反解卡片与版本并预填 */
async function openEdit(row: DeploymentItem) {
  editingId.value = row.id;
  const at = row.plaza_model_id.indexOf('@');
  const cardId = at > 0 ? row.plaza_model_id.slice(0, at) : row.card_id;
  const version = at > 0 ? row.plaza_model_id.slice(at + 1) : '';
  form.name = row.name;
  form.card_id = cardId;
  form.version = version;
  form.hub_source = row.hub_source;
  form.node_ids = [...row.node_ids];
  form.head_node_id = row.head_node_id || row.node_ids[0] || '';
  form.configs = { ...(row.configs as Record<string, any>) };
  // 覆盖回填：权重覆盖 → 下拉值；镜像覆盖 → 可编辑行；额外环境变量 → KV 行
  form.weight_override_value = weightOverrideValue(row.weight_override);
  form.images_override = toEditableImages(row.images_override);
  form.extra_env = envMapToRows(row.extra_env);
  // 标准接入参数回填（API Key 不回显，留空 = 沿用现值）
  form.serve_model_name = row.serve_model_name || '';
  form.serve_api_key = '';
  form.serve_port = row.serve_port || undefined;
  form.serve_url_prefix = row.serve_url_prefix || '';
  // 本地反代回填
  form.local_proxy_enabled = !!row.local_proxy_enabled;
  form.local_proxy_port = row.local_proxy_port || undefined;
  // 自动重启回填（秒 → 分钟；关闭时保留阈值）
  form.auto_restart_enabled = !!row.auto_restart_enabled;
  form.auto_restart_fail_minutes = secsToMinutes(row.auto_restart_fail_secs);
  variants.value = [];
  modalOpen.value = true;
  await loadRefs();
  // 失效节点（已被删除）：从选择中剔除并提示重新选择；否则保存必然失败（20702）
  // 见 openspec/changes/modelinfer-dangling-node-reference/
  if (nodes.value.length) {
    const { kept, stale } = splitNodeSelection(row.node_ids, nodes.value.map((n) => n.id));
    if (stale.length) {
      form.node_ids = kept;
      form.head_node_id = fallbackHead(form.head_node_id, kept);
      message.warning(
        t('runs.missingNodesReplaced', { nodes: stale.join(t('runs.nodeJoin')) }),
      );
    }
  }
  await loadVariants(row.weight_id || selectedCard.value?.model || '');
  trimNodesToLimit();
}

function onEdit(row: DeploymentItem) {
  if (!isRowEditable(row)) {
    message.warning(t('runs.editNotAllowed'));
    return;
  }
  void openEdit(row);
}

function onNodesChange(ids: string[]) {
  let next = ids;
  const max = capsMaxNodes.value;
  if (max > 0 && next.length > max) {
    message.warning(t('runs.maxNodesTruncated', { max }));
    next = next.slice(0, max);
  }
  form.node_ids = next;
  if (!next.includes(form.head_node_id)) form.head_node_id = next[0] || '';
}

async function submit() {
  if (!form.name) {
    message.warning(t('runs.requireName'));
    return;
  }
  if (!form.card_id) {
    message.warning(t('runs.requireCard'));
    return;
  }
  if (!form.version) {
    message.warning(t('runs.requireVersion'));
    return;
  }
  const weightOverride = parseWeightOverrideValue(form.weight_override_value);
  // 仅声明了权重的卡片需要选择模型变体（无权重卡片如 ComfyUI 跳过；已指定权重覆盖时跳过）
  if (!isWeightless.value && !weightOverride && !form.hub_source) {
    message.warning(t('runs.requireVariant'));
    return;
  }
  if (!form.node_ids.length) {
    message.warning(t('runs.requireNodes'));
    return;
  }
  if (capsMaxNodes.value > 0 && form.node_ids.length > capsMaxNodes.value) {
    message.warning(t('runs.maxNodesExceeded', { max: capsMaxNodes.value }));
    return;
  }
  if (!form.head_node_id) {
    message.warning(t('runs.requireHead'));
    return;
  }
  if (imagesOverflowed.value) {
    message.warning(t('runs.form.imagesOverflow', { max: MAX_IMAGES_OVERRIDE }));
    return;
  }
  if (extraEnvErrors.value.size) {
    message.warning(t('runs.form.extraEnvInvalid'));
    return;
  }
  // head 必须是所选节点之一，且放到首位（后端约定首元素为 head）
  const ordered = [form.head_node_id, ...form.node_ids.filter((id) => id !== form.head_node_id)];
  const payload: DeploymentCreateReq = {
    name: form.name,
    plaza_model_id: plazaModelId.value,
    hub_source: isWeightless.value ? '' : form.hub_source,
    node_ids: ordered,
    configs: form.configs,
    // 覆盖：权重覆盖为空 → null（清除）；镜像覆盖为空数组 → 回到卡片；额外变量为空对象 → 清空
    weight_override: parseWeightOverrideValue(form.weight_override_value),
    images_override: fromEditableImages(form.images_override),
    extra_env: envRowsToMap(form.extra_env),
  };
  // 标准接入参数：仅提交用户显式填写的值（留空 = 平台自动推断 / 沿用现值）
  if (form.serve_model_name) payload.serve_model_name = form.serve_model_name;
  if (form.serve_api_key) payload.serve_api_key = form.serve_api_key;
  if (form.serve_port !== undefined && form.serve_port !== null) payload.serve_port = form.serve_port;
  if (form.serve_url_prefix) payload.serve_url_prefix = form.serve_url_prefix;
  // 本地反代：布尔与端口始终提交（端口 0 = 自动分配）
  payload.local_proxy_enabled = form.local_proxy_enabled;
  payload.local_proxy_port =
    form.local_proxy_port && form.local_proxy_port > 0 ? form.local_proxy_port : 0;
  // 自动重启：布尔与阈值始终提交（分钟 → 秒；关闭时保留阈值供下次开启）
  payload.auto_restart_enabled = form.auto_restart_enabled;
  payload.auto_restart_fail_secs = minutesToSecs(
    clampMinutes(form.auto_restart_fail_minutes),
  );
  saving.value = true;
  try {
    if (editingId.value) {
      await updateDeployment(editingId.value, payload);
      message.success(t('runs.updated'));
    } else {
      await createDeployment(payload);
      message.success(t('runs.created'));
    }
    modalOpen.value = false;
    editingId.value = null;
    await load();
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    saving.value = false;
  }
}

async function onStart(row: DeploymentItem) {
  if (startingIds.value.has(row.id)) return;
  startingIds.value.add(row.id);
  try {
    await startDeployment(row.id);
    if (resourcesRun.value?.id === row.id) resourcesOpen.value = false;
    message.success(t('runs.startSubmitted', { name: row.name }));
    await load();
  } catch (e) {
    const resources = missingStartResources(e);
    if (resources) {
      resourcesRun.value = row;
      missingResources.value = resources;
      resourcesOpen.value = true;
      return;
    }
    Modal.error({
      title: t('runs.startFailed', { name: row.name }),
      content: resolveApiErrorMessage(e),
      okText: t('common.confirm'),
    });
  } finally {
    startingIds.value.delete(row.id);
  }
}

async function onStop(row: DeploymentItem) {
  Modal.confirm({
    title: t('runs.stopTitle'),
    content: t('runs.stopContent', { name: row.name }),
    okText: t('runs.action.stop'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      try {
        await stopDeployment(row.id);
        message.success(t('runs.stopSubmitted'));
        await load();
      } catch (e) {
        message.error(resolveApiErrorMessage(e));
      }
    },
  });
}

function onForceStop(row: DeploymentItem) {
  Modal.confirm({
    title: t('runs.forceStopTitle'),
    content: t('runs.forceStopContent', { name: row.name }),
    okText: t('runs.action.forceStop'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      try {
        await stopDeployment(row.id, true);
        message.success(t('runs.forceStopSubmitted'));
        await load();
      } catch (e) {
        message.error(resolveApiErrorMessage(e));
      }
    },
  });
}

function onDelete(row: DeploymentItem) {
  Modal.confirm({
    title: t('runs.deleteTitle'),
    content: t('runs.deleteContent', { name: row.name }),
    okText: t('common.delete'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      try {
        await deleteDeployment(row.id);
        message.success(t('runs.deleted'));
        await load();
      } catch (e) {
        message.error(resolveApiErrorMessage(e));
      }
    },
  });
}

/** 进入运行配置详情：平台内优先打开独立 Vben 标签页，桥接缺失 / 失败回退当前页 */
async function onDetail(row: DeploymentItem) {
  const handled = await openDetailInNewTab({
    hashPath: `#/runs/${row.id}`,
    title: localizedTitle('routes.runDetail', row.name),
  });
  if (!handled) router.push({ name: 'run-detail', params: { id: row.id } });
}

/** 表格「版本」从 plaza_model_id 取 `@` 之后的部分 */
function versionOfRow(row: DeploymentItem): string {
  const at = row.plaza_model_id.indexOf('@');
  return at > 0 ? row.plaza_model_id.slice(at + 1) : '-';
}

onMounted(async () => {
  await load();
  timer = window.setInterval(load, 5000);
});
onUnmounted(() => {
  if (timer) window.clearInterval(timer);
});
</script>

<template>
  <PageShell
    :title="t('runs.title')"
    :subtitle="t('runs.subtitle')"
    :hide-heading="workspace"
    :loading="loading"
  >
    <template #stats>
      <StatPill :label="t('runs.title')" :value="stats.total" tone="primary" />
      <StatPill :label="t('runs.state.running')" :value="stats.running" />
      <StatPill :label="t('runs.state.stopped')" :value="stats.stopped" />
      <StatPill :label="t('runs.state.failed')" :value="stats.failed" />
    </template>

    <template #actions>
      <a-button @click="load">
        <ReloadOutlined />
        {{ t('common.refresh') }}
      </a-button>
      <a-button type="primary" @click="openCreate">
        <PlusOutlined />
        {{ t('runs.newConfig') }}
      </a-button>
    </template>

    <div class="mi-toolbar">
      <a-input
        v-model:value="filters.keyword"
        :placeholder="t('runs.searchPlaceholder')"
        allow-clear
        style="width: 280px"
      >
        <template #prefix><SearchOutlined /></template>
      </a-input>
      <a-select v-model:value="filters.state" :options="stateOptions" style="width: 140px" />
      <a-select v-model:value="filters.card" :options="cardFilterOptions" style="width: 220px" />
      <a-select
        v-model:value="filters.sort"
        style="width: 150px; margin-left: auto"
        :options="sortOptions"
      />
      <span v-if="lastUpdated" class="mi-hint">{{
        t('common.lastUpdated', { time: formatTime(lastUpdated) })
      }}</span>
    </div>

    <a-table
      row-key="id"
      :columns="columns"
      :data-source="rows"
      :pagination="pagination"
      :scroll="{ x: workspace ? 1061 : 1330 }"
      @change="onTableChange"
    >
      <template #bodyCell="{ column, record }">
        <template v-if="column.key === 'name'">
          <div class="mi-name-cell">
            <ModelAvatar :family="record.family" :size="36" />
            <a class="mi-name-cell__link" :title="record.name" @click="onDetail(record)">
              {{ record.name }}
            </a>
          </div>
        </template>

        <template v-else-if="column.key === 'model'">
          <div class="mi-model-cell__body">
            <div class="mi-model-cell__name">{{ record.card_name || record.card_id || '-' }}</div>
            <div class="mi-model-cell__meta">
              <a-tag class="mi-tag-soft">v{{ versionOfRow(record) }}</a-tag>
              <a-tag
                v-if="record.effective_hub_source || record.hub_source"
                class="mi-tag-soft"
                >{{ record.effective_hub_source || record.hub_source }}</a-tag
              >
              <a-tag v-if="record.weight_overridden" color="orange">
                {{ t('runs.overridden') }}
              </a-tag>
              <CopyText :text="record.effective_weight_id || record.weight_id || '-'" secondary />
            </div>
          </div>
        </template>

        <template v-else-if="column.key === 'node_count'">
          <span class="mi-mono">{{ record.nodes.length }}</span>
          <div v-if="deletedNodeIds(record).length" class="mi-hint mi-error">
            {{ t('runs.deletedNode') }} {{ deletedNodeIds(record).length }}
          </div>
        </template>

        <template v-else-if="column.key === 'head_node'">
          <template v-if="headNodeOf(record)">
            <a-tag v-if="isMissingNode(headNodeOf(record)!)" color="red">
              {{ t('runs.deletedNode') }}
            </a-tag>
            <a-tag v-else color="gold">{{ headNodeOf(record)!.hostname || headNodeOf(record)!.name }}</a-tag>
            <div class="mi-hint">
              {{ isMissingNode(headNodeOf(record)!) ? headNodeOf(record)!.id : headNodeOf(record)!.host }}
            </div>
          </template>
          <span v-else>-</span>
        </template>

        <template v-else-if="column.key === 'state'">
          <StatusTag kind="deployment" :status="record.state" />
        </template>

        <template v-else-if="column.key === 'health_status'">
          <StatusTag kind="health" :status="record.health_status" />
        </template>

        <template v-else-if="column.key === 'updated_at'">
          <TimeText :value="record.updated_at" />
        </template>

        <template v-else-if="column.key === 'action'">
          <div class="mi-action">
            <div class="mi-row-mark" :style="rowMarkStyle(record)" />
            <a-space :size="4" class="mi-action__btns">
              <a-tooltip v-if="primaryAction(record) === 'start'" :title="startHint(record)">
                <a-button
                  size="small"
                  type="primary"
                  :disabled="!canStartRow(record)"
                  :loading="startingIds.has(record.id)"
                  @click="onStart(record)"
                >
                  {{ t(startingIds.has(record.id) ? 'runs.resources.checking' : 'runs.action.start') }}
                </a-button>
              </a-tooltip>
              <a-button
                v-else
                size="small"
                danger
                :disabled="record.state === 'stopping'"
                @click="onStop(record)"
              >
                {{ t('runs.action.stop') }}
              </a-button>
              <a-button size="small" @click="onDetail(record)">{{ t('common.detail') }}</a-button>
              <a-dropdown :trigger="['click']">
                <a-button size="small">
                  {{ t('common.more') }}
                  <DownOutlined />
                </a-button>
                <template #overlay>
                  <a-menu @click="(e: any) => onMoreMenu(e, record)">
                    <a-menu-item key="edit" :disabled="!isRowEditable(record)">{{ t('common.edit') }}</a-menu-item>
                    <a-menu-item key="force-stop">{{ t('runs.action.forceStop') }}</a-menu-item>
                    <a-menu-divider />
                    <a-menu-item key="delete" danger>{{ t('common.delete') }}</a-menu-item>
                  </a-menu>
                </template>
              </a-dropdown>
            </a-space>
          </div>
        </template>
      </template>
      <template #emptyText>
        <a-empty :description="t('runs.empty')">
          <a-button type="primary" @click="openCreate">{{ t('runs.newConfig') }}</a-button>
        </a-empty>
      </template>
    </a-table>

    <div v-if="stats.total && isFiltering" class="mi-footer-hint">
      {{ t('runs.filteredFooter', { shown: page.total, total: stats.total }) }}
    </div>

    <StartResourcesModal
      v-model:open="resourcesOpen"
      :name="resourcesRun?.name || ''"
      :resources="missingResources"
      :checking="!!resourcesRun && startingIds.has(resourcesRun.id)"
      @retry="resourcesRun && onStart(resourcesRun)"
    />

    <a-modal
      v-model:open="modalOpen"
      :title="modalTitle"
      :confirm-loading="saving"
      :ok-text="editingId ? t('common.save') : t('common.create')"
      :cancel-text="t('common.cancel')"
      width="760px"
      @ok="submit"
    >
      <a-form layout="vertical">
        <a-form-item :label="t('runs.form.name')" required>
          <a-input v-model:value="form.name" :placeholder="t('runs.form.namePlaceholder')" />
        </a-form-item>

        <div class="mi-step"><span class="mi-step__no">1</span>{{ t('runs.form.step1') }}</div>
        <a-form-item :label="t('runs.form.card')" required>
          <a-select
            v-model:value="form.card_id"
            :placeholder="t('runs.form.cardPlaceholder')"
            :options="cardOptions"
            :loading="loading"
            show-search
            option-filter-prop="label"
            @change="onSelectCard"
          />
        </a-form-item>
        <a-form-item :label="t('runs.form.version')" required>
          <a-select
            v-model:value="form.version"
            :placeholder="t('runs.form.versionPlaceholder')"
            :options="versionOptions"
            :disabled="!form.card_id"
            @change="onSelectVersion"
          />
        </a-form-item>
        <a-form-item v-if="versionOptions.length === 0" label=" ">
          <span class="mi-hint">{{ t('runs.form.noAvailableVersion') }}</span>
        </a-form-item>

        <div class="mi-step"><span class="mi-step__no">2</span>{{ t('runs.form.step2') }}</div>
        <div v-if="isWeightless" class="mi-hint" style="margin-bottom: 12px">
          {{ t('runs.form.weightlessHint') }}
        </div>
        <a-spin v-else :spinning="variantsLoading">
          <a-alert
            v-if="!variants.length && form.weight_override_value"
            type="success"
            show-icon
            :message="t('runs.form.noVariantOverridden')"
            style="margin-bottom: 8px"
          />
          <a-empty v-else-if="!variants.length" :description="t('runs.form.noVariant')" />
          <a-radio-group v-else v-model:value="form.hub_source">
            <a-radio v-for="v in variants" :key="v.source" :value="v.source" style="display: block">
              {{ t('runs.form.variantLabel', { source: v.source, path: v.rel_path }) }}
            </a-radio>
          </a-radio-group>
        </a-spin>

        <div class="mi-step"><span class="mi-step__no">3</span>{{ t('runs.form.step3') }}</div>
        <a-form-item
          :label="capsMaxNodes > 0 ? t('runs.form.nodesMaxLabel', { max: capsMaxNodes }) : t('runs.form.nodesLabel')"
          required
        >
          <a-checkbox-group :value="form.node_ids" :options="nodeOptions" @change="(v: unknown) => onNodesChange(v as string[])" />
          <div v-if="nodeLimitHint" class="mi-hint">{{ nodeLimitHint }}</div>
        </a-form-item>
        <a-form-item :label="t('runs.form.headNode')" required>
          <a-select v-model:value="form.head_node_id" :options="headOptions" :placeholder="t('runs.form.headPlaceholder')" />
        </a-form-item>

        <div class="mi-step"><span class="mi-step__no">4</span>{{ t('runs.form.step4') }}</div>
        <a-form-item :label="t('runs.form.serveModelName')">
          <a-input v-model:value="form.serve_model_name" :placeholder="t('runs.form.serveModelNamePlaceholder')" />
        </a-form-item>
        <a-form-item :label="editingId ? t('runs.form.serveApiKeyEdit') : t('runs.form.serveApiKeyCreate')">
          <a-input-password v-model:value="form.serve_api_key" :placeholder="t('runs.form.serveApiKeyPlaceholder')" />
        </a-form-item>
        <a-space style="display: flex" :size="16">
          <a-form-item :label="t('runs.form.servePort')" style="flex: 1">
            <a-input-number v-model:value="form.serve_port" :min="1" :max="65535" placeholder="8000" style="width: 100%" />
          </a-form-item>
          <a-form-item :label="t('runs.form.urlPrefix')" style="flex: 1">
            <a-input v-model:value="form.serve_url_prefix" placeholder="/v1" />
          </a-form-item>
        </a-space>
        <a-alert
          type="info"
          show-icon
          :message="t('runs.form.serveAlert')"
          style="margin-bottom: 12px"
        />

        <div class="mi-step mi-step--sub"><span class="mi-step__no">4.1</span>{{ t('runs.form.step41') }}</div>
        <a-space style="display: flex" :size="16" align="baseline">
          <a-form-item :label="t('runs.form.localProxyEnabled')">
            <a-switch v-model:checked="form.local_proxy_enabled" />
          </a-form-item>
          <a-form-item :label="t('runs.form.localProxyPort')" style="flex: 1">
            <a-input-number
              v-model:value="form.local_proxy_port"
              :min="1"
              :max="65535"
              :placeholder="t('runs.form.localProxyPortPlaceholder')"
              style="width: 100%"
              :disabled="!form.local_proxy_enabled"
            />
          </a-form-item>
        </a-space>
        <div class="mi-hint" style="margin-bottom: 12px">
          {{ t('runs.form.localProxyHint') }}
        </div>

        <div class="mi-step mi-step--sub">
          <span class="mi-step__no">4.2</span>{{ t('runs.form.step42') }}
        </div>
        <a-space style="display: flex" :size="16" align="baseline">
          <a-form-item :label="t('runs.form.autoRestartEnabled')">
            <a-switch v-model:checked="form.auto_restart_enabled" />
          </a-form-item>
          <a-form-item :label="t('runs.form.autoRestartFailMinutes')" style="flex: 1">
            <a-input-number
              v-model:value="form.auto_restart_fail_minutes"
              :min="MIN_FAIL_MINUTES"
              :max="MAX_FAIL_MINUTES"
              :step="1"
              :precision="0"
              :disabled="!form.auto_restart_enabled"
              style="width: 100%"
            >
              <template #suffix>{{ t('runs.form.autoRestartMinutesUnit') }}</template>
            </a-input-number>
          </a-form-item>
        </a-space>
        <div class="mi-hint" style="margin-bottom: 12px">
          {{ t('runs.form.autoRestartHint', { minutes: clampMinutes(form.auto_restart_fail_minutes) }) }}
        </div>
        <a-alert
          v-if="form.auto_restart_enabled && cardLacksHealthProbe"
          type="warning"
          show-icon
          :message="t('runs.form.autoRestartNoHealthProbe')"
          style="margin-bottom: 12px"
        />

        <div class="mi-step"><span class="mi-step__no">5</span>{{ t('runs.form.step5') }}</div>
        <a-collapse>
          <a-collapse-panel key="configs" :header="t('runs.form.configPanelHeader')">
            <template v-if="configSchema.length">
              <a-form-item
                v-for="spec in configSchema"
                :key="String(spec.name)"
                :label="t('runs.form.configLabel', { title: displayText(spec.title) || spec.name, name: spec.name })"
              >
                <a-input-number
                  v-if="spec.type === 'integer' || spec.type === 'number'"
                  v-model:value="form.configs[String(spec.name)]"
                  style="width: 100%"
                />
                <a-switch
                  v-else-if="spec.type === 'boolean'"
                  v-model:checked="form.configs[String(spec.name)]"
                />
                <a-input
                  v-else
                  v-model:value="form.configs[String(spec.name)]"
                />
              </a-form-item>
            </template>
            <a-empty v-else :description="t('runs.form.noConfigParams')" />
          </a-collapse-panel>
        </a-collapse>

        <div class="mi-step"><span class="mi-step__no">6</span>{{ t('runs.form.step6') }}</div>
        <a-collapse>
          <a-collapse-panel key="overrides" :header="t('runs.form.overridePanelHeader')">
            <a-alert
              type="info"
              show-icon
              :message="t('runs.form.overrideAlert')"
              style="margin-bottom: 12px"
            />
            <a-form-item :label="t('runs.form.weightOverride')">
              <a-space style="display: flex; width: 100%" align="baseline">
                <a-select
                  v-model:value="form.weight_override_value"
                  :options="weightOverrideOptions"
                  :placeholder="
                    cardDefaultWeight
                      ? t('runs.form.weightOverridePlaceholder', { weight: cardDefaultWeight })
                      : t('runs.form.weightOverridePlaceholderEmpty')
                  "
                  allow-clear
                  show-search
                  option-filter-prop="label"
                  style="flex: 1; min-width: 260px"
                />
                <a-button :disabled="!form.weight_override_value" @click="clearWeightOverride">
                  {{ t('runs.form.restoreDefault') }}
                </a-button>
              </a-space>
              <div class="mi-hint">{{ t('runs.form.weightOverrideHint') }}</div>
            </a-form-item>

            <a-form-item :label="t('runs.form.imagesOverride')">
              <div class="mi-override-list">
                <div
                  v-for="(row, idx) in form.images_override"
                  :key="idx"
                  class="mi-override-row"
                >
                  <a-input
                    v-model:value="row.name"
                    :placeholder="t('runs.form.imageNamePlaceholder')"
                    style="flex: 1; min-width: 220px"
                  />
                  <a-select
                    v-model:value="row.arch"
                    :options="imageArchOptions"
                    style="width: 110px"
                  />
                  <a-button danger size="small" @click="removeImageRow(idx)">
                    {{ t('common.delete') }}
                  </a-button>
                </div>
                <a-space>
                  <a-button size="small" @click="addImageRow">＋ {{ t('runs.form.addImage') }}</a-button>
                  <a-button size="small" :disabled="!form.images_override.length" @click="resetImagesOverride">
                    {{ t('runs.form.restoreDefault') }}
                  </a-button>
                </a-space>
              </div>
              <div class="mi-hint">
                {{
                  form.images_override.length
                    ? t('runs.form.imagesOverrideActive', { count: form.images_override.length })
                    : t('runs.form.imagesOverrideDefault', {
                        images: cardDefaultImageLabels.length
                          ? cardDefaultImageLabels.join('、')
                          : t('runs.form.imagesOverrideNone'),
                      })
                }}
              </div>
            </a-form-item>
          </a-collapse-panel>

          <a-collapse-panel key="extraEnv" :header="t('runs.form.extraEnvPanelHeader')">
            <div class="mi-override-list">
              <div v-for="(row, idx) in form.extra_env" :key="idx" class="mi-override-row">
                <a-input
                  v-model:value="row.key"
                  :placeholder="t('runs.form.extraEnvKeyPlaceholder')"
                  style="flex: 1; min-width: 160px"
                />
                <a-input
                  v-model:value="row.value"
                  :placeholder="t('runs.form.extraEnvValuePlaceholder')"
                  style="flex: 2; min-width: 200px"
                />
                <a-button danger size="small" @click="removeEnvRow(idx)">
                  {{ t('common.delete') }}
                </a-button>
                <div v-if="extraEnvErrors.has(idx)" class="mi-error mi-override-error">
                  {{ envErrorText(extraEnvErrors.get(idx)) }}
                </div>
              </div>
              <a-button size="small" @click="addEnvRow">＋ {{ t('runs.form.addExtraEnv') }}</a-button>
            </div>
            <div class="mi-hint">{{ t('runs.form.extraEnvHint') }}</div>
          </a-collapse-panel>
        </a-collapse>
      </a-form>
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
.mi-tag-soft {
  color: var(--mi-text-secondary);
  background: var(--mi-surface-2);
  border-color: var(--mi-border);
}
.mi-error {
  color: var(--mi-error);
  font-size: 12px;
}
.mi-override-list {
  display: flex;
  flex-direction: column;
  gap: 8px;
  width: 100%;
}
.mi-override-row {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.mi-override-error {
  flex-basis: 100%;
}
.mi-footer-hint {
  margin-top: 12px;
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
  text-align: right;
}

/* ---------- 名称列（家族图标 + 可点击名称） ---------- */
.mi-name-cell {
  display: flex;
  align-items: center;
  gap: 10px;
}
.mi-name-cell__link {
  font-weight: 600;
  color: var(--mi-text);
  cursor: pointer;
  word-break: break-all;
}
.mi-name-cell__link:hover {
  color: var(--mi-primary);
}

/* ---------- 模型列 ---------- */
.mi-model-cell__body {
  min-width: 0;
}
.mi-model-cell__name {
  font-weight: 600;
  color: var(--mi-text);
  line-height: 1.35;
}
.mi-model-cell__meta {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 3px;
  flex-wrap: wrap;
}

/* ---------- 操作列 + 行右侧浅色大图标 ---------- */
.mi-action {
  position: relative;
  display: flex;
  justify-content: flex-end;
}
.mi-action__btns {
  position: relative;
  z-index: 1;
}
.mi-row-mark {
  position: absolute;
  right: -6px;
  top: 50%;
  transform: translateY(-50%);
  width: 132px;
  height: 132px;
  background: center / contain no-repeat;
  opacity: 0.09;
  pointer-events: none;
  z-index: 0;
}

/* ---------- 向导步骤标题 ---------- */
.mi-step {
  display: flex;
  align-items: center;
  gap: 8px;
  margin: 4px 0 12px;
  font-size: 14px;
  font-weight: 600;
  color: var(--mi-text);
}
.mi-step__no {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  min-width: 22px;
  height: 22px;
  padding: 0 6px;
  border-radius: 999px;
  font-size: 12px;
  color: #fff;
  background: var(--mi-primary-gradient);
}
.mi-step--sub {
  font-size: 13px;
  font-weight: 500;
}
.mi-step--sub .mi-step__no {
  background: var(--mi-neutral);
}
</style>
