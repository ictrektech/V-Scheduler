<script setup lang="ts">
import { computed, nextTick, onMounted, onUnmounted, ref } from 'vue';
import { useRoute } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { message } from 'ant-design-vue';
import {
  FileTextOutlined,
  PlusOutlined,
  ReloadOutlined,
  ThunderboltOutlined,
} from '@ant-design/icons-vue';
import PageShell from '@/components/PageShell.vue';
import RunHistoryList from '@/components/RunHistoryList.vue';
import StatusTag from '@/components/StatusTag.vue';
import TimeText from '@/components/TimeText.vue';
import CopyText from '@/components/CopyText.vue';
import ModelAvatar from '@/components/ModelAvatar.vue';
import TerminalBlock from '@/components/TerminalBlock.vue';
import JsonBlock from '@/components/JsonBlock.vue';
import MetricsBoard from '@/components/MetricsBoard.vue';
import { useMetrics } from '@/composables/useMetrics';
import { inferenceProfileFor } from '@/utils/metricProfiles';
import { imageEntryLabel } from '@/utils/runtimeOverrides';
import { inRestartLoop, phaseLabelKey, secsToMinutes } from '@/utils/autoRestart';
import {
  getDeployment,
  getDeploymentAccess,
  getDeploymentMetrics,
  listComfyuiWeights,
  addComfyuiWeight,
  deleteComfyuiWeight,
  registerDeploymentVrouter,
  updateDeploymentLocalProxy,
  type ComfyuiWeightItem,
  type DeploymentAccess,
  type DeploymentItem,
} from '@/api/deployments';
import {
  fetchModelHubInfo,
  fetchVariantFiles,
  type ModelHubVariant,
  type VariantFile,
} from '@/api/modelHub';
import { embeddedRuntime } from '@/api/client';
import {
  containerLogStreamUrl,
  readContainerLogStream,
  getRun,
  getRunTemporal,
  listContainers,
  listRunLogs,
  listRuns,
  type ContainerItem,
  type RunItem,
  type RunLogItem,
  type RunStepItem,
  type GlobalDataItem,
  type NodeDataItem,
  type TemporalWorkflowItem,
} from '@/api/observability';
import { formatBytes, formatDuration, formatTime, summarizeDetail } from '@/utils/format';
import { formatClock } from '@/utils/metrics';
import { configRows } from '@/utils/configs';
import {
  buildFileTree,
  buildInstanceTree,
  collectFilePaths,
  inferTargetType,
  isValidTargetType,
} from '@/utils/comfyui';
import { resolveApiErrorMessage } from '@/i18n/api-error';

const { t, locale } = useI18n();
const route = useRoute();
const id = computed(() => String(route.params.id || ''));

const loading = ref(false);
const deployment = ref<DeploymentItem | null>(null);
const runs = ref<RunItem[]>([]);
const containers = ref<ContainerItem[]>([]);
const access = ref<DeploymentAccess | null>(null);
const registering = ref(false);
// 本地反代在线开关 / 端口编辑
const proxyEnabled = ref(false);
const proxyPort = ref<number | null>(null);
const savingProxy = ref(false);

// ---------- 推理指标（复用通用取数 composable + 指标板，与节点图表一致）----------
/**
 * 推理指标目标是否活跃（接口 `active`）。**仅提示位**：不活跃时仍展示 prometheus 中的历史指标
 * （见 design D-MI-SGH-05），不再清空图表。
 */
const metricsActive = ref(true);
/** 本次结果中最近采样时间（unix 秒；用于历史提示） */
const metricsLastSample = ref<number | null>(null);
/** 停止后首屏自动放宽区间只尝试一次（1h → 24h），避免循环 */
const historyWidenPending = ref(true);
const {
  loading: metricsLoading,
  groups: metricGroups,
  windowStart,
  windowEnd,
  rangeMode,
  customRange,
  autoRefresh,
  refreshSecs,
  lastUpdated,
  wide,
  load: loadMetrics,
} = useMetrics(
  async (params) => {
    const data = await getDeploymentMetrics(id.value, params);
    // 兼容旧后端：字段缺失时按「活跃」处理，避免误报不可达
    metricsActive.value = data.active !== false;
    metricsLastSample.value = data.last_sample ?? null;
    return data;
  },
  (msg) => message.error(msg),
);
/** 推理指标 profile（未知 / 不受支持引擎 → null） */
const inferenceProfile = computed(() => inferenceProfileFor(deployment.value?.inference_engine));

/**
 * 加载指标，并在「服务已停止 + 默认 1h 无数据」时自动放宽到 24h 重试一次，
 * 提升历史指标的可发现性（design D-MI-SGH-06）。
 */
async function loadMetricsWithWiden(silent = false) {
  await loadMetrics(silent);
  if (
    !silent &&
    historyWidenPending.value &&
    !metricsActive.value &&
    metricGroups.value.length === 0 &&
    rangeMode.value === '1h'
  ) {
    historyWidenPending.value = false;
    rangeMode.value = '24h';
  }
}
/** 自动重启连续失败阈值（分钟，用于展示；非法值回落默认 5 分钟） */
const autoRestartMinutes = computed(() => secsToMinutes(deployment.value?.auto_restart_fail_secs));
/** 是否正处于自动重启循环中（stopping / starting） */
const autoRestarting = computed(() => inRestartLoop(deployment.value?.auto_restart_phase));
/** 自动重启相位标签 */
const autoRestartPhaseLabel = computed(() => {
  void locale.value;
  return t(phaseLabelKey(deployment.value?.auto_restart_phase));
});
/** 卡片是否缺 health-probe（自动重启不会生效） */
const autoRestartIneffective = computed(
  () => !!deployment.value?.auto_restart_enabled && !deployment.value?.health_probe_declared,
);
/** 引擎展示标签 */
const engineLabel = computed(() => {
  void locale.value;
  const e = deployment.value?.inference_engine || '';
  if (!e) return t('runDetail.overview.notProbed');
  const v = deployment.value?.inference_engine_version;
  return v ? `${e} ${v}` : e;
});
/**
 * 非活跃提示文案：区分「未运行」与「目标不可达」，并在存在历史数据时给出最近采样时间
 * （此时图表继续展示历史曲线，不再清空）。
 */
const metricsInactiveMessage = computed(() => {
  void locale.value;
  const st = deployment.value?.state || '';
  const notRunning = !!st && !['running', 'degraded'].includes(st);
  const hasData = metricsLastSample.value != null;
  const at = { time: formatClock(metricsLastSample.value) };
  if (notRunning) {
    return hasData
      ? t('runDetail.metrics.historyNotRunning', at)
      : t('runDetail.metrics.historyNotRunningEmpty');
  }
  return hasData
    ? t('runDetail.metrics.historyUnreachable', at)
    : t('runDetail.metrics.historyUnreachableEmpty');
});

// 参数配置（configs）审查弹窗
const configModalOpen = ref(false);
const configRowsList = computed(() => configRows(deployment.value?.configs));
const configColumns = computed(() => {
  void locale.value;
  return [
    { title: t('runDetail.configModal.columns.name'), key: 'name', width: 240 },
    { title: t('runDetail.configModal.columns.value'), key: 'valueText' },
  ];
});

/** 运行配置名称 */
const name = computed(() => deployment.value?.name || id.value);
/** 版本：从 plaza_model_id 取 `@` 之后 */
const version = computed(() => {
  const v = deployment.value?.plaza_model_id || '';
  const at = v.indexOf('@');
  return at > 0 ? v.slice(at + 1) : '-';
});

// ---------- ComfyUI 实例权重管理（卡片 engine === 'comfyui' 时可用）----------
/** 卡片声明的引擎是否为 ComfyUI */
const isComfyui = computed(() => (deployment.value?.engine || '') === 'comfyui');

// ---------- 接入配置（按引擎裁剪）----------
/** 接入配置分区标题：ComfyUI 用「访问信息」，其余沿用「接入配置」 */
const accessTitle = computed(() =>
  isComfyui.value ? t('runDetail.access.comfyuiTitle') : t('runDetail.access.title'),
);
/** 浏览器访问地址：优先后端 `web_url`，旧后端缺失时按 host:port 兜底 */
const webUrl = computed(() => {
  const a = access.value;
  if (!a) return '';
  if (a.web_url) return a.web_url;
  return a.host ? `http://${a.host}:${a.port}` : '';
});
/** 反代访问地址（监听时）：`http://<当前主机>:<active_port>`，ComfyUI 不带路径前缀 */
const proxyAccessUrl = computed(() => {
  const p = access.value?.local_proxy?.active_port;
  if (!p) return '';
  const host = window.location.hostname;
  const prefix = isComfyui.value ? '' : access.value?.url_prefix || '';
  return `http://${host}:${p}${prefix}`;
});
const comfyuiWeights = ref<ComfyuiWeightItem[]>([]);
const comfyuiTypes = ref<string[]>([]);
const comfyuiLoading = ref(false);
const addWeightOpen = ref(false);
const addWeightSubmitting = ref(false);
const hubVariants = ref<ModelHubVariant[]>([]);
const weightFiles = ref<VariantFile[]>([]);
const filesLoading = ref(false);
/** 左栏勾选的文件相对路径（多选） */
const checkedFilePaths = ref<string[]>([]);
const addForm = ref<{
  source: string;
  weight_id: string;
  target_type: string;
  link_name: string;
  auto_infer: boolean;
}>({
  source: 'hf',
  weight_id: '',
  target_type: 'checkpoints',
  link_name: '',
  auto_infer: true,
});
const variantOptions = computed(() =>
  hubVariants.value
    .filter((v) => v.source === addForm.value.source)
    .map((v) => ({ label: v.model_id, value: v.model_id })),
);
const typeOptions = computed(() => comfyuiTypes.value.map((tp) => ({ value: tp })));
/** 左栏：源模型文件目录树（文件可勾选） */
const sourceFileTree = computed(() => buildFileTree(weightFiles.value));
/** 右栏：本实例权重目录树（按目标类型分组，只读预览） */
const instanceWeightTree = computed(() => buildInstanceTree(comfyuiWeights.value));
const sourceFileCount = computed(() => collectFilePaths(sourceFileTree.value).length);
const checkedCount = computed(() => checkedFilePaths.value.length);
/** 文件树字段名映射（TreeNode.name → antd title） */
const treeFieldNames = { title: 'name', key: 'key', children: 'children' } as const;
const comfyuiColumns = computed(() => {
  void locale.value;
  return [
    { title: t('runDetail.comfyui.columns.source'), dataIndex: 'source', key: 'source', width: 70 },
    { title: t('runDetail.comfyui.columns.weight'), key: 'weight', width: 220 },
    { title: t('runDetail.comfyui.columns.file'), dataIndex: 'file_path', key: 'file', ellipsis: true },
    { title: t('runDetail.comfyui.columns.type'), dataIndex: 'target_type', key: 'type', width: 140 },
    { title: t('runDetail.comfyui.columns.link'), dataIndex: 'link_rel', key: 'link', width: 240 },
    { title: t('runDetail.comfyui.columns.action'), key: 'action', width: 90 },
  ];
});

/** 加载实例权重台账（非 ComfyUI 卡片跳过） */
async function loadComfyuiWeights() {
  if (!isComfyui.value) return;
  comfyuiLoading.value = true;
  try {
    const data = await listComfyuiWeights(id.value);
    comfyuiWeights.value = data.weights;
    comfyuiTypes.value = data.types;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    comfyuiLoading.value = false;
  }
}

/** 打开添加弹窗并懒加载 model-hub 权重清单 */
async function openAddWeight() {
  addForm.value = {
    source: 'hf',
    weight_id: '',
    target_type: 'checkpoints',
    link_name: '',
    auto_infer: true,
  };
  weightFiles.value = [];
  checkedFilePaths.value = [];
  addWeightOpen.value = true;
  if (!hubVariants.value.length) {
    try {
      hubVariants.value = (await fetchModelHubInfo()).models;
    } catch (e) {
      message.error(resolveApiErrorMessage(e));
    }
  }
}

/** 选定来源 / 权重后加载其文件清单并构建左栏目录树 */
async function onAddWeightPick() {
  checkedFilePaths.value = [];
  weightFiles.value = [];
  if (!addForm.value.weight_id) return;
  filesLoading.value = true;
  try {
    weightFiles.value = (await fetchVariantFiles(addForm.value.source, addForm.value.weight_id)).files;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    filesLoading.value = false;
  }
}

/** 提交添加：对每个勾选文件推断类型并逐一添加，汇总结果 */
async function submitAddWeight() {
  const picked = checkedFilePaths.value.slice();
  if (!picked.length) {
    message.warning(t('runDetail.comfyui.needFile'));
    return;
  }
  const fallbackType = addForm.value.target_type.trim();
  if (!isValidTargetType(fallbackType)) {
    message.warning(t('runDetail.comfyui.needType'));
    return;
  }
  addWeightSubmitting.value = true;
  let ok = 0;
  let failed = 0;
  let lastErr = '';
  try {
    for (const relPath of picked) {
      const targetType = inferTargetType(relPath, fallbackType, addForm.value.auto_infer);
      const linkName = picked.length === 1 ? addForm.value.link_name.trim() : '';
      try {
        await addComfyuiWeight(id.value, {
          source: addForm.value.source,
          weight_id: addForm.value.weight_id,
          file_path: relPath,
          target_type: targetType,
          link_name: linkName || undefined,
        });
        ok += 1;
      } catch (e) {
        failed += 1;
        lastErr = resolveApiErrorMessage(e);
      }
    }
    if (ok > 0) await loadComfyuiWeights();
    if (failed === 0) {
      message.success(t('runDetail.comfyui.addedCount', { count: ok }));
      addWeightOpen.value = false;
    } else if (ok === 0) {
      message.error(lastErr || t('runDetail.comfyui.addFailed'));
    } else {
      message.warning(t('runDetail.comfyui.addPartial', { ok, failed }));
    }
  } finally {
    addWeightSubmitting.value = false;
  }
}

/** 删除实例权重 */
async function removeComfyuiWeight(record: ComfyuiWeightItem) {
  try {
    await deleteComfyuiWeight(id.value, record.id);
    message.success(t('runDetail.comfyui.removed'));
    await loadComfyuiWeights();
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  }
}

/** 从运行配置同步本地反代表单 */
function syncProxyForm() {
  proxyEnabled.value = !!deployment.value?.local_proxy_enabled;
  const p = deployment.value?.local_proxy_port || 0;
  proxyPort.value = p > 0 ? p : null;
}

const activeTab = ref('run');
const selectedRunId = ref('');
const runKindFilter = ref('all');
const steps = ref<RunStepItem[]>([]);
const nodeData = ref<NodeDataItem[]>([]);
const globalData = ref<GlobalDataItem[]>([]);
const logs = ref<RunLogItem[]>([]);
const detailLoading = ref(false);

// Temporal 活动弹窗（按需查询，不落库）
const temporalOpen = ref(false);
const temporalLoading = ref(false);
const temporal = ref<TemporalWorkflowItem | null>(null);

let timer: number | undefined;

const logColumns = computed(() => {
  void locale.value;
  return [
    { title: t('runDetail.columns.node'), key: 'node', width: 190 },
    { title: t('runDetail.columns.attempt'), dataIndex: 'attempt', key: 'attempt', width: 70 },
    { title: t('runDetail.columns.exitCode'), key: 'exit_code', width: 90 },
    { title: t('runDetail.columns.started'), dataIndex: 'started_at', key: 'started_at', width: 150 },
    { title: t('runDetail.columns.duration'), key: 'duration', width: 100 },
  ];
});

const temporalColumns = computed(() => {
  void locale.value;
  return [
    { title: t('runDetail.columns.activityType'), dataIndex: 'activity_type', key: 'activity_type' },
    { title: t('runDetail.columns.state'), key: 'state', width: 110 },
    { title: t('runDetail.columns.attempt'), dataIndex: 'attempt', key: 'attempt', width: 70 },
    { title: t('runDetail.columns.scheduledTime'), key: 'scheduled_time', width: 170 },
    { title: t('runDetail.columns.lastFailure'), dataIndex: 'last_failure', key: 'last_failure' },
  ];
});

const containerColumns = computed(() => {
  void locale.value;
  return [
    { title: t('runDetail.columns.node'), key: 'node', width: 200 },
    { title: t('runDetail.columns.containerName'), key: 'name' },
    { title: t('runDetail.columns.image'), key: 'image' },
    { title: t('runDetail.columns.sourceStage'), dataIndex: 'stage', key: 'stage', width: 110 },
    { title: t('runDetail.columns.updatedAt'), dataIndex: 'updated_at', key: 'updated_at', width: 150 },
    { title: t('runDetail.columns.action'), key: 'action', width: 110 },
  ];
});

// ---- 容器日志查看器（SSE 流式）----
const MAX_LOG_LINES = 2000;
const logDrawerOpen = ref(false);
const logTitle = ref('');
const logBuffer = ref('');
const logPaused = ref(false);
const logAutoScroll = ref(true);
const logBlockRef = ref<InstanceType<typeof TerminalBlock> | null>(null);
let es: EventSource | null = null;
let logStream: AbortController | null = null;
let currentLogTarget: { nodeId: string; name: string } | null = null;

const logLineCount = computed(() => (logBuffer.value ? logBuffer.value.split('\n').length : 0));

function appendLog(text: string) {
  let buf = logBuffer.value + text;
  const lines = buf.split('\n');
  if (lines.length > MAX_LOG_LINES) {
    buf = lines.slice(lines.length - MAX_LOG_LINES).join('\n');
  }
  logBuffer.value = buf;
  if (logAutoScroll.value) void nextTick(() => logBlockRef.value?.scrollToEnd());
}

function stopStream() {
  logStream?.abort();
  logStream = null;
  if (es) {
    es.close();
    es = null;
  }
}

function startStream() {
  stopStream();
  if (!currentLogTarget) return;
  const url = containerLogStreamUrl(id.value, currentLogTarget.nodeId, currentLogTarget.name, 30);
  logPaused.value = false;
  if (embeddedRuntime) {
    const stream = new AbortController();
    logStream = stream;
    void readContainerLogStream(url, stream.signal, (line) => appendLog(`${line}\n`))
      .catch(() => undefined)
      .finally(() => {
        if (logStream === stream) {
          logStream = null;
          logPaused.value = true;
        }
      });
    return;
  }
  es = new EventSource(url);
  es.onmessage = (ev) => appendLog(`${ev.data}\n`);
  es.onerror = () => undefined;
}

function openLogs(record: ContainerItem) {
  currentLogTarget = { nodeId: record.node_id, name: record.name };
  logTitle.value = `${record.node_name || record.node_id} · ${record.name}`;
  logBuffer.value = '';
  logAutoScroll.value = true;
  logDrawerOpen.value = true;
  startStream();
}

function togglePause() {
  if (logPaused.value) {
    startStream();
  } else {
    stopStream();
    logPaused.value = true;
  }
}

function clearLogs() {
  logBuffer.value = '';
}

function onLogDrawerChange(open: boolean) {
  if (!open) {
    stopStream();
    currentLogTarget = null;
  }
}

/** 选中 run */
const selectedRun = computed(() => runs.value.find((r) => r.id === selectedRunId.value) || null);
/** 是否为生命周期阶段步骤（可展开节点脚本与结构化数据子项） */
function isStageStep(s: RunStepItem): boolean {
  return !!s.stage;
}
/** 某阶段步骤的节点脚本执行 */
function logsOfStage(stage: string): RunLogItem[] {
  return logs.value.filter((l) => l.stage === stage);
}
/** 某阶段步骤的节点结构化对象 */
function nodeDataOfStage(stage: string): NodeDataItem[] {
  return nodeData.value.filter((n) => n.stage === stage);
}
/** 某阶段步骤的全局结构化对象 */
function globalDataOfStage(stage: string): GlobalDataItem[] {
  return globalData.value.filter((g) => g.stage === stage);
}
/** 非阶段步骤结果摘要的单行关键值（折叠面板标题用；空则仅显示「结果摘要」） */
function summaryOf(detail: unknown): string {
  return summarizeDetail(detail);
}
/** 步骤错误首行摘要（折叠面板标题用，避免长错误（含节点脚本输出）撑高时间线） */
function errorHead(err: string): string {
  const first = (err || '').split('\n')[0].trim();
  return first.length > 120 ? `${first.slice(0, 120)}…` : first;
}

/** 运行类型筛选选项（来自当前运行记录，默认「全部」保留全部信息） */
const runKindOptions = computed(() => {
  void locale.value;
  const present = new Set(runs.value.map((r) => r.kind));
  const all: Array<[string, string]> = [
    ['start', t('runDetail.history.kind.start')],
    ['stop', t('runDetail.history.kind.stop')],
    ['health', t('runDetail.history.kind.health')],
    ['deploy', t('runDetail.history.kind.deploy')],
    ['cleanup', t('runDetail.history.kind.cleanup')],
  ];
  const opts = [{ label: t('runDetail.history.allKinds'), value: 'all' }];
  for (const [value, label] of all) {
    if (present.has(value)) opts.push({ label, value });
  }
  return opts;
});
const filteredRuns = computed(() =>
  runKindFilter.value === 'all' ? runs.value : runs.value.filter((r) => r.kind === runKindFilter.value),
);

async function loadDeployment() {
  try {
    deployment.value = await getDeployment(id.value);
    syncProxyForm();
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  }
}

async function loadRuns() {
  try {
    const data = await listRuns(id.value, { limit: 100 });
    runs.value = data.data;
    if (!selectedRunId.value && runs.value.length) {
      selectedRunId.value = runs.value[0].id;
      await loadRunDetail();
    }
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  }
}

async function loadContainers() {
  try {
    containers.value = (await listContainers(id.value)).data;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  }
}

async function loadAccess() {
  try {
    access.value = await getDeploymentAccess(id.value);
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  }
}

/** 手动立即注册到统一网关 */
async function registerVrouter() {
  registering.value = true;
  try {
    access.value = await registerDeploymentVrouter(id.value);
    const st = access.value?.vrouter?.state;
    if (st === 'registered') message.success(t('runDetail.msg.registered'));
    else
      message.warning(
        t('runDetail.msg.registerIncomplete', {
          state: st,
          error: access.value?.vrouter?.last_error || '',
        }),
      );
    await loadDeployment();
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    registering.value = false;
  }
}

// 本地反代端口设置弹窗（端口不直接展示在页面上）
const proxyModalOpen = ref(false);
const proxyPortDraft = ref<number | null>(null);

/** 开关直接生效（沿用当前期望端口） */
async function onProxyToggle(checked: boolean) {
  savingProxy.value = true;
  try {
    access.value = await updateDeploymentLocalProxy(id.value, {
      enabled: checked,
      port: proxyPort.value && proxyPort.value > 0 ? proxyPort.value : 0,
    });
    message.success(checked ? t('runDetail.msg.proxyEnabled') : t('runDetail.msg.proxyDisabled'));
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    savingProxy.value = false;
    await loadDeployment();
  }
}

function openProxyModal() {
  proxyPortDraft.value = proxyPort.value;
  proxyModalOpen.value = true;
}

/** 保存端口设置（保持当前启用状态） */
async function saveProxyPort() {
  savingProxy.value = true;
  try {
    access.value = await updateDeploymentLocalProxy(id.value, {
      enabled: proxyEnabled.value,
      port: proxyPortDraft.value && proxyPortDraft.value > 0 ? proxyPortDraft.value : 0,
    });
    message.success(t('runDetail.msg.proxyPortSaved'));
    proxyModalOpen.value = false;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    savingProxy.value = false;
    await loadDeployment();
  }
}

async function loadRunDetail() {
  if (!selectedRunId.value) {
    steps.value = [];
    nodeData.value = [];
    globalData.value = [];
    logs.value = [];
    return;
  }
  detailLoading.value = true;
  try {
    const [detail, logData] = await Promise.all([
      getRun(id.value, selectedRunId.value),
      listRunLogs(id.value, selectedRunId.value),
    ]);
    steps.value = detail.steps;
    nodeData.value = detail.node_data;
    globalData.value = detail.global_data;
    logs.value = logData.data;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    detailLoading.value = false;
  }
}

async function selectRun(runId: string) {
  selectedRunId.value = runId;
  await loadRunDetail();
}

/** 打开 Temporal 活动弹窗（按需查询，不落库） */
async function openTemporal() {
  temporalOpen.value = true;
  temporal.value = null;
  if (!selectedRunId.value) return;
  temporalLoading.value = true;
  try {
    temporal.value = await getRunTemporal(id.value, selectedRunId.value);
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    temporalLoading.value = false;
  }
}

async function refresh() {
  loading.value = true;
  await Promise.all([loadDeployment(), loadAccess(), loadRuns(), loadContainers()]);
  if (isComfyui.value) await loadComfyuiWeights();
  if (inferenceProfile.value) await loadMetricsWithWiden();
  if (selectedRunId.value) await loadRunDetail();
  loading.value = false;
}

async function load() {
  await Promise.all([loadDeployment(), loadAccess(), loadRuns(), loadContainers()]);
  if (isComfyui.value) await loadComfyuiWeights();
  if (inferenceProfile.value) await loadMetricsWithWiden();
}

onMounted(async () => {
  await load();
  timer = window.setInterval(async () => {
    // 刷新列表 / 容器 / 接入状态；运行中的 run 同步刷新步骤时间线
    await loadRuns();
    await loadContainers();
    await loadAccess();
    if (selectedRun.value?.state === 'running' && !detailLoading.value) {
      await loadRunDetail();
    }
  }, 5000);
});
onUnmounted(() => {
  if (timer) window.clearInterval(timer);
  stopStream();
});
</script>

<template>
  <PageShell
    :title="t('runDetail.overview.title', { name })"
    :subtitle="t('runDetail.overview.subtitle')"
    :loading="loading"
  >
    <template #actions>
      <a-button @click="refresh">
        <ReloadOutlined />
        {{ t('common.refresh') }}
      </a-button>
    </template>

    <!-- 状态概览 -->
    <div class="mi-ov">
      <div class="mi-ov__main">
        <ModelAvatar :family="deployment?.family" :size="62" />
        <div class="mi-ov__id">
          <div class="mi-ov__name">{{ name }}</div>
          <div class="mi-ov__sub">
            <span class="mi-hint">{{ t('runDetail.overview.runConfigId') }}</span>
            <CopyText :text="id" secondary />
          </div>
        </div>
        <div class="mi-ov__badges">
          <StatusTag kind="deployment" :status="deployment?.state" />
          <StatusTag kind="health" :status="deployment?.health_status" />
          <a-button size="small" @click="configModalOpen = true">
            {{ t('runDetail.overview.paramsConfig', { count: configRowsList.length }) }}
          </a-button>
        </div>
      </div>
      <div class="mi-ov__grid">
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('runDetail.overview.modelCard') }}</div>
          <div class="mi-kv__v">
            <span>{{ deployment?.card_name || deployment?.card_id || '-' }}</span>
            <CopyText :text="deployment?.card_id || ''" secondary />
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('runDetail.overview.version') }}</div>
          <div class="mi-kv__v">v{{ version }}</div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('runDetail.overview.weight') }}</div>
          <div class="mi-kv__v">
            <CopyText :text="deployment?.effective_weight_id || '-'" />
            <a-tooltip
              v-if="deployment?.weight_overridden"
              :title="t('runDetail.overview.cardDefaultWeight', { weight: deployment?.weight_id || '-' })"
            >
              <a-tag color="orange" style="margin-left: 6px">
                {{ t('runDetail.overview.overridden') }}
              </a-tag>
            </a-tooltip>
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('runDetail.overview.source') }}</div>
          <div class="mi-kv__v">{{ deployment?.effective_hub_source || '-' }}</div>
        </div>
        <div class="mi-kv mi-kv--wide">
          <div class="mi-kv__k">{{ t('runDetail.overview.modelPath') }}</div>
          <div class="mi-kv__v"><CopyText :text="deployment?.effective_hub_rel_path || '-'" /></div>
        </div>
        <div class="mi-kv mi-kv--wide">
          <div class="mi-kv__k">{{ t('runDetail.overview.images') }}</div>
          <div class="mi-kv__v">
            <a-tag
              v-if="deployment?.images_source"
              :color="deployment.images_source === 'override' ? 'orange' : 'default'"
              style="margin-right: 6px"
            >
              {{
                deployment.images_source === 'override'
                  ? t('runDetail.overview.imageSourceOverride')
                  : t('runDetail.overview.imageSourceCard')
              }}
            </a-tag>
            <a-tag
              v-for="(img, idx) in deployment?.effective_images || []"
              :key="idx"
              class="mi-node-tag"
            >
              {{ imageEntryLabel(img) }}
            </a-tag>
            <span v-if="!(deployment?.effective_images || []).length">
              {{ t('runDetail.overview.noImages') }}
            </span>
          </div>
        </div>
        <div class="mi-kv mi-kv--wide">
          <div class="mi-kv__k">{{ t('runDetail.overview.nodes') }}</div>
          <div class="mi-kv__v">
            <a-tag
              v-for="n in deployment?.nodes || []"
              :key="n.id"
              :color="n.status === 'missing' ? 'red' : n.role === 'head' ? 'gold' : 'blue'"
              class="mi-node-tag"
            >
              <template v-if="n.status === 'missing'">
                {{ t('runDetail.overview.deletedNode') }}（{{ n.id }}）
              </template>
              <template v-else>{{ t('common.nodeLabel', { name: n.name, host: n.host }) }} {{ n.role }}</template>
            </a-tag>
            <span v-if="!(deployment?.nodes || []).length">-</span>
            <div v-if="(deployment?.missing_node_ids || []).length" class="mi-hint mi-error">
              {{ t('runDetail.overview.missingNodeHint') }}
            </div>
          </div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('runDetail.overview.createdAt') }}</div>
          <div class="mi-kv__v"><TimeText :value="deployment?.created_at" /></div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('runDetail.overview.lastStarted') }}</div>
          <div class="mi-kv__v"><TimeText :value="deployment?.last_started_at" /></div>
        </div>
        <div class="mi-kv">
          <div class="mi-kv__k">{{ t('runDetail.overview.lastUpdated') }}</div>
          <div class="mi-kv__v"><TimeText :value="deployment?.updated_at" /></div>
        </div>
        <div class="mi-kv mi-kv--wide">
          <div class="mi-kv__k">{{ t('runDetail.overview.autoRestart') }}</div>
          <div class="mi-kv__v">
            <span v-if="!deployment?.auto_restart_enabled" class="mi-hint">
              {{ t('runDetail.overview.autoRestartOff') }}
            </span>
            <template v-else>
              <a-tag color="blue">
                {{ t('runDetail.overview.autoRestartOn', { minutes: autoRestartMinutes }) }}
              </a-tag>
              <a-tag v-if="autoRestarting" color="orange">
                {{ t('runDetail.overview.autoRestarting', {
                  phase: autoRestartPhaseLabel,
                  attempts: deployment?.auto_restart_attempts ?? 0,
                }) }}
              </a-tag>
              <span v-if="deployment?.auto_restart_next_at" class="mi-hint">
                {{ t('runDetail.overview.autoRestartNextAt') }}
                <TimeText :value="deployment?.auto_restart_next_at" />
              </span>
              <a-tooltip
                v-if="autoRestartIneffective"
                :title="t('runDetail.overview.autoRestartNoHealthProbe')"
              >
                <a-tag color="warning">{{ t('runDetail.overview.autoRestartIneffective') }}</a-tag>
              </a-tooltip>
            </template>
          </div>
        </div>
      </div>
      <div v-if="deployment?.last_error" class="mi-ov__error">
        <span class="mi-hint">{{ t('runDetail.overview.lastError') }}</span>
        <span class="mi-error">{{ deployment.last_error }}</span>
      </div>
      <div v-if="deployment?.auto_restart_last_error" class="mi-ov__error">
        <span class="mi-hint">{{ t('runDetail.overview.autoRestartLastError') }}</span>
        <span class="mi-error">{{ deployment.auto_restart_last_error }}</span>
      </div>
    </div>

    <!-- 接入配置（按引擎裁剪）：ComfyUI 展示访问信息 + 本地反代；OpenAI / API Key / 统一网关仅非 ComfyUI -->
    <a-card size="small" :title="accessTitle" style="margin-bottom: 16px">
      <template #extra>
        <a-space>
          <a-tag v-if="access" :color="access.available ? 'green' : 'default'">
            {{ access.available ? t('runDetail.access.available') : t('runDetail.access.notRunning') }}
          </a-tag>
          <a-button v-if="!isComfyui" size="small" :loading="registering" @click="registerVrouter">{{
            t('runDetail.access.registerVrouter')
          }}</a-button>
        </a-space>
      </template>

      <!-- ComfyUI：浏览器访问地址（Web UI） -->
      <template v-if="isComfyui">
        <a-descriptions :column="2" size="small">
          <a-descriptions-item :label="t('runDetail.access.webUrl')">
            <CopyText v-if="webUrl" :text="webUrl" />
            <span v-else class="mi-hint">{{ t('runDetail.access.noAddress') }}</span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('runDetail.access.node')">
            <CopyText v-if="access?.host" :text="access.host" secondary />
            <span v-else>-</span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('runDetail.access.port')">{{ access?.port ?? '-' }}</a-descriptions-item>
        </a-descriptions>
        <div class="mi-hint" style="margin-bottom: 12px">{{ t('runDetail.access.comfyuiHint') }}</div>
      </template>

      <!-- 通用引擎：OpenAI 兼容接入 + 统一网关 -->
      <a-descriptions v-else :column="2" size="small">
        <a-descriptions-item :label="t('runDetail.access.openaiEndpoint')">
          <CopyText :text="access?.openai_base_url || '-'" />
        </a-descriptions-item>
        <a-descriptions-item :label="t('runDetail.access.modelName')">{{ access?.model || '-' }}</a-descriptions-item>
        <a-descriptions-item label="API Key">
          <CopyText v-if="access?.api_key" :text="access.api_key" />
          <span v-else class="mi-hint">{{ t('runDetail.access.authDisabled') }}</span>
        </a-descriptions-item>
        <a-descriptions-item :label="t('runDetail.access.portPrefix')">
          {{ access?.port ?? '-' }} · {{ access?.url_prefix || '-' }}
        </a-descriptions-item>
        <a-descriptions-item :label="t('runDetail.access.vrouterStatus')">
          <StatusTag kind="vrouter" :status="access?.vrouter?.state" />
          <span v-if="access?.vrouter?.last_error" class="mi-error"> {{ access.vrouter.last_error }}</span>
          <div class="mi-hint">{{ t('runDetail.access.vrouterDirectHint') }}</div>
        </a-descriptions-item>
        <a-descriptions-item :label="t('runDetail.access.vrouterModelId')">
          <CopyText v-if="access?.vrouter?.model_id" :text="access.vrouter.model_id" secondary />
          <span v-else>-</span>
        </a-descriptions-item>
      </a-descriptions>

      <div class="mi-proxy">
        <div class="mi-proxy__head">
          <span class="mi-proxy__title">{{ t('runDetail.proxy.title') }}</span>
          <a-switch
            :checked="proxyEnabled"
            :loading="savingProxy"
            :checked-children="t('runDetail.proxy.on')"
            :un-checked-children="t('runDetail.proxy.off')"
            @change="(v: any) => onProxyToggle(!!v)"
          />
          <a-tag :color="access?.local_proxy?.active_port ? 'green' : 'default'">
            {{
              access?.local_proxy?.active_port
                ? t('runDetail.proxy.listening', { port: access.local_proxy.active_port })
                : t('runDetail.proxy.notRunning')
            }}
          </a-tag>
          <a-button size="small" @click="openProxyModal">{{ t('runDetail.proxy.portSettings') }}</a-button>
        </div>
        <div class="mi-proxy__meta">
          <span class="mi-hint"
            >{{ t('runDetail.proxy.expectedPort')
            }}<span class="mi-mono">{{ proxyPort || t('runDetail.proxy.auto') }}</span></span
          >
          <span class="mi-hint"
            >{{ t('runDetail.proxy.forwardTarget')
            }}<span class="mi-mono">{{ access?.local_proxy?.target || '-' }}</span></span
          >
          <span v-if="access?.local_proxy?.last_error" class="mi-error">{{ access.local_proxy.last_error }}</span>
        </div>
        <div v-if="proxyAccessUrl" class="mi-proxy__access">
          <span class="mi-hint">{{ t('runDetail.proxy.accessUrl') }}</span>
          <CopyText :text="proxyAccessUrl" />
        </div>
      </div>
    </a-card>

    <!-- 推理指标：对受支持引擎（vLLM / SGLang）复用通用指标板；非活跃时仍展示历史曲线 -->
    <a-card size="small" :title="t('runDetail.metrics.title')" style="margin-bottom: 16px">
      <template #extra>
        <a-space wrap>
          <a-tag :color="inferenceProfile ? 'blue' : 'default'">{{
            t('runDetail.metrics.engine', { engine: engineLabel })
          }}</a-tag>
          <a-tag v-if="deployment?.metrics_state" :color="deployment.metrics_state === 'registered' ? 'green' : 'default'">
            {{
              deployment.metrics_state === 'registered'
                ? t('runDetail.metrics.collected')
                : t('runDetail.metrics.state', { state: deployment.metrics_state })
            }}
          </a-tag>
          <span v-if="deployment?.metrics_last_error" class="mi-error">{{ deployment.metrics_last_error }}</span>
        </a-space>
      </template>
      <a-alert
        v-if="inferenceProfile && !metricsActive"
        type="info"
        show-icon
        :message="metricsInactiveMessage"
        style="margin-bottom: 12px"
      />
      <MetricsBoard
        v-if="inferenceProfile"
        :profile="inferenceProfile"
        :groups="metricGroups"
        :loading="metricsLoading"
        :last-updated="lastUpdated"
        :range-mode="rangeMode"
        :custom-range="customRange"
        :auto-refresh="autoRefresh"
        :refresh-secs="refreshSecs"
        :wide="wide"
        :window-start="windowStart"
        :window-end="windowEnd"
        @update:range-mode="rangeMode = $event"
        @update:custom-range="customRange = $event"
        @update:auto-refresh="autoRefresh = $event"
        @update:refresh-secs="refreshSecs = $event"
        @update:wide="wide = $event"
        @refresh="loadMetricsWithWiden(false)"
      />
      <a-empty
        v-else
        :description="
          deployment?.inference_engine
            ? t('runDetail.metrics.unsupportedEngine', { engine: engineLabel })
            : t('runDetail.metrics.engineUnknown')
        "
      />
    </a-card>

    <a-tabs v-model:activeKey="activeTab">
      <a-tab-pane key="run" :tab="t('runDetail.tabs.runsSteps', { count: runs.length })">
        <div class="mi-split">
          <!-- 左列：运行历史（紧凑列表，独立滚动，整行点击选中） -->
          <div class="mi-split__aside">
            <RunHistoryList
              :runs="filteredRuns"
              :selected-id="selectedRunId"
              :filter="runKindFilter"
              :filter-options="runKindOptions"
              @select="selectRun"
              @update:filter="(v: string) => (runKindFilter = v)"
            />
          </div>

          <!-- 右列：选中 run 的步骤时间线（步骤为主轴，阶段步骤内嵌节点脚本 / 结构化数据） -->
          <div class="mi-split__main">
            <template v-if="selectedRun">
              <div class="mi-tl-head">
            <div class="mi-tl-head__meta">
              <span class="mi-section-title" style="margin: 0">{{ t('runDetail.runs.title') }}</span>
              <StatusTag kind="runKind" :status="selectedRun.kind" />
              <StatusTag kind="run" :status="selectedRun.state" />
              <span class="mi-hint">
                {{ formatTime(selectedRun.started_at) }} ·
                {{ formatDuration(selectedRun.started_at, selectedRun.finished_at) }}
              </span>
            </div>
            <a-button v-if="selectedRun.has_temporal" size="small" @click="openTemporal">
              <ThunderboltOutlined />
              {{ t('runDetail.runs.temporalActivities') }}
            </a-button>
          </div>
          <div v-if="selectedRun.error" class="mi-error" style="margin-bottom: 8px">
            {{ selectedRun.error }}
          </div>
          <a-spin :spinning="detailLoading">
            <a-empty v-if="!steps.length" :description="t('runDetail.runs.noSteps')" />
            <div v-else class="mi-tl">
              <div
                v-for="s in steps"
                :key="s.id"
                class="mi-tl-item"
                :class="{ 'mi-tl-item--running': s.state === 'running' }"
              >
                <div class="mi-tl-dot" />
                <div class="mi-tl-body">
                  <div class="mi-tl-line">
                    <StatusTag kind="stepState" :status="s.state" />
                    <span class="mi-tl-name">{{ s.title || s.step }}</span>
                    <a-tag class="mi-tag-soft mi-mono">{{ s.step }}</a-tag>
                    <StatusTag kind="orchestrator" :status="s.orchestrator" />
                    <span v-if="s.node_name" class="mi-hint mi-mono">{{ s.node_name }}</span>
                    <span class="mi-hint">{{ formatDuration(s.started_at, s.finished_at) }}</span>
                  </div>
                  <a-collapse v-if="s.error" ghost size="small" class="mi-tl-error">
                    <a-collapse-panel key="err">
                      <template #header>
                        <span class="mi-error">{{ errorHead(s.error) }}</span>
                        <span class="mi-hint" style="margin-left: 8px">{{ t('runDetail.runs.errorDetail') }}</span>
                      </template>
                      <pre class="mi-tl-error__pre">{{ s.error }}</pre>
                    </a-collapse-panel>
                  </a-collapse>
                  <div v-if="!isStageStep(s) && s.detail" class="mi-tl-detail">
                    <JsonBlock
                      :data="s.detail"
                      :title="
                        summaryOf(s.detail)
                          ? t('runDetail.runs.resultSummaryWith', { summary: summaryOf(s.detail) })
                          : t('runDetail.runs.resultSummary')
                      "
                      :default-open="false"
                    />
                  </div>
                  <a-collapse v-if="isStageStep(s)" ghost size="small" style="margin-top: 6px">
                    <a-collapse-panel key="sub">
                      <template #header>
                        <span class="mi-hint">
                          {{ t('runDetail.runs.nodeScriptsAndData', { count: logsOfStage(s.stage).length }) }}
                        </span>
                      </template>
                      <a-table
                        row-key="id"
                        size="small"
                        :columns="logColumns"
                        :data-source="logsOfStage(s.stage)"
                        :pagination="false"
                        :scroll="{ x: 700 }"
                      >
                        <template #bodyCell="{ column, record }">
                          <template v-if="column.key === 'node'">
                            {{ t('common.nodeLabel', { name: record.node_name || record.node_id, host: record.node_host }) }}
                          </template>
                          <template v-else-if="column.key === 'exit_code'">
                            <a-tag :color="record.exit_code === 0 ? 'green' : 'red'">{{ record.exit_code }}</a-tag>
                          </template>
                          <template v-else-if="column.key === 'started_at'">
                            <TimeText :value="record.started_at" />
                          </template>
                          <template v-else-if="column.key === 'duration'">
                            <span class="mi-mono">{{ formatDuration(record.started_at, record.finished_at) }}</span>
                          </template>
                        </template>
                        <template #expandedRowRender="{ record }">
                          <div v-if="record.truncated" class="mi-hint" style="margin-bottom: 8px">
                            {{
                              t('runDetail.runs.outputTruncated', {
                                stdout: formatBytes(record.stdout_bytes),
                                stderr: formatBytes(record.stderr_bytes),
                              })
                            }}
                          </div>
                          <TerminalBlock
                            :text="record.stdout"
                            label="stdout"
                            :filename="`${record.stage}-${record.node_name || record.node_id}-stdout`"
                            style="margin-bottom: 8px"
                          />
                          <TerminalBlock
                            v-if="record.stderr"
                            :text="record.stderr"
                            label="stderr"
                            tone="error"
                            :filename="`${record.stage}-${record.node_name || record.node_id}-stderr`"
                          />
                        </template>
                        <template #emptyText><a-empty :description="t('runDetail.runs.noStageLogs')" /></template>
                      </a-table>

                      <div class="mi-section-title" style="margin-top: 12px">{{ t('runDetail.runs.globalObjects') }}</div>
                      <template v-if="globalDataOfStage(s.stage).length">
                        <JsonBlock
                          v-for="g in globalDataOfStage(s.stage)"
                          :key="g.id"
                          :data="g.data"
                          :title="t('runDetail.runs.globalTitle', { stage: g.stage, count: g.node_count })"
                          style="margin-bottom: 12px"
                        />
                      </template>
                      <a-empty v-else :description="t('runDetail.runs.noGlobalData')" />

                      <div class="mi-section-title" style="margin-top: 12px">{{ t('runDetail.runs.nodeObjects') }}</div>
                      <template v-if="nodeDataOfStage(s.stage).length">
                        <JsonBlock
                          v-for="n in nodeDataOfStage(s.stage)"
                          :key="n.id"
                          :data="n.data"
                          :title="
                            t('runDetail.runs.nodeObjectTitle', {
                              stage: n.stage,
                              name: n.node_name || n.node_id,
                              host: n.node_host,
                            })
                          "
                          style="margin-bottom: 12px"
                        />
                      </template>
                      <a-empty v-else :description="t('runDetail.runs.noNodeData')" />
                    </a-collapse-panel>
                  </a-collapse>
                </div>
              </div>
            </div>
          </a-spin>
            </template>
            <a-empty v-else :description="t('runDetail.runs.selectRunHint')" />
          </div>
        </div>
      </a-tab-pane>

      <a-tab-pane key="containers" :tab="t('runDetail.tabs.containers', { count: containers.length })">
        <a-table
          row-key="id"
          size="small"
          :columns="containerColumns"
          :data-source="containers"
          :pagination="false"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'node'">
              {{ t('common.nodeLabel', { name: record.node_name || record.node_id, host: record.node_host }) }}
            </template>
            <template v-else-if="column.key === 'name'">
              <CopyText :text="record.name" />
            </template>
            <template v-else-if="column.key === 'image'">
              <CopyText :text="record.image || '-'" secondary />
            </template>
            <template v-else-if="column.key === 'updated_at'">
              <TimeText :value="record.updated_at" />
            </template>
            <template v-else-if="column.key === 'action'">
              <a-button size="small" @click="openLogs(record)">
                <FileTextOutlined />
                {{ t('runDetail.containers.viewLogs') }}
              </a-button>
            </template>
          </template>
          <template #emptyText>
            <a-empty :description="t('runDetail.containers.empty')" />
          </template>
        </a-table>
      </a-tab-pane>

      <a-tab-pane
        v-if="isComfyui"
        key="comfyui-weights"
        :tab="t('runDetail.tabs.comfyuiWeights', { count: comfyuiWeights.length })"
      >
        <div class="mi-section-head" style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px">
          <span class="mi-hint">{{ t('runDetail.comfyui.hint') }}</span>
          <a-button type="primary" size="small" @click="openAddWeight">
            <PlusOutlined />
            {{ t('runDetail.comfyui.add') }}
          </a-button>
        </div>
        <a-table
          row-key="id"
          size="small"
          :columns="comfyuiColumns"
          :data-source="comfyuiWeights"
          :loading="comfyuiLoading"
          :pagination="false"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'weight'">
              <CopyText :text="`${record.source}:${record.weight_id}`" />
            </template>
            <template v-else-if="column.key === 'file'">
              <span style="font-family: monospace">{{ record.file_path }}</span>
            </template>
            <template v-else-if="column.key === 'type'">
              <a-tag>{{ record.target_type }}</a-tag>
            </template>
            <template v-else-if="column.key === 'link'">
              <span style="font-family: monospace">{{ record.link_rel }}</span>
            </template>
            <template v-else-if="column.key === 'action'">
              <a-popconfirm
                :title="t('runDetail.comfyui.removeConfirm')"
                @confirm="removeComfyuiWeight(record)"
              >
                <a-button type="link" danger size="small">{{ t('runDetail.comfyui.remove') }}</a-button>
              </a-popconfirm>
            </template>
          </template>
          <template #emptyText>
            <a-empty :description="t('runDetail.comfyui.empty')" />
          </template>
        </a-table>
      </a-tab-pane>
    </a-tabs>

    <a-drawer
      :open="logDrawerOpen"
      :title="t('runDetail.logDrawer.title', { title: logTitle })"
      width="900"
      @close="logDrawerOpen = false"
      @after-open-change="onLogDrawerChange"
    >
      <TerminalBlock
        ref="logBlockRef"
        :text="logBuffer"
        label="docker logs"
        :empty="t('runDetail.logDrawer.waiting')"
        max-height="calc(100vh - 240px)"
        :filename="logTitle || 'container-log'"
      >
        <template #actions>
          <a-button size="small" @click="togglePause">{{
            logPaused ? t('runDetail.logDrawer.resume') : t('runDetail.logDrawer.pause')
          }}</a-button>
          <a-button size="small" @click="clearLogs">{{ t('runDetail.logDrawer.clear') }}</a-button>
          <a-checkbox v-model:checked="logAutoScroll" class="mi-term-check">{{
            t('runDetail.logDrawer.autoScroll')
          }}</a-checkbox>
          <span class="mi-term-meta">{{
            t('runDetail.logDrawer.retained', { count: logLineCount, max: MAX_LOG_LINES })
          }}</span>
          <a-tag :color="logPaused ? 'orange' : 'green'">{{
            logPaused ? t('runDetail.logDrawer.paused') : t('runDetail.logDrawer.following')
          }}</a-tag>
        </template>
      </TerminalBlock>
    </a-drawer>

    <a-modal
      v-model:open="proxyModalOpen"
      :title="t('runDetail.proxyModal.title')"
      :confirm-loading="savingProxy"
      :ok-text="t('common.save')"
      :cancel-text="t('common.cancel')"
      @ok="saveProxyPort"
    >
      <a-form layout="vertical">
        <a-form-item :label="t('runDetail.proxyModal.mappedPort')">
          <a-input-number
            v-model:value="proxyPortDraft"
            :min="1"
            :max="65535"
            :placeholder="t('runDetail.proxyModal.portPlaceholder')"
            style="width: 100%"
          />
        </a-form-item>
      </a-form>
    </a-modal>

    <!-- Temporal 活动：按需查询（不落库） -->
    <a-modal
      v-model:open="temporalOpen"
      :title="t('runDetail.temporalModal.title')"
      :footer="null"
      width="780"
    >
      <a-spin :spinning="temporalLoading">
        <template v-if="temporal">
          <a-descriptions :column="2" size="small">
            <a-descriptions-item :label="t('runDetail.temporalModal.workflowType')">{{ temporal.workflow_type }}</a-descriptions-item>
            <a-descriptions-item :label="t('runDetail.temporalModal.status')">
              <a-tag>{{ temporal.status }}</a-tag>
            </a-descriptions-item>
            <a-descriptions-item :label="t('runDetail.temporalModal.workflowId')">
              <CopyText :text="temporal.workflow_id" secondary />
            </a-descriptions-item>
            <a-descriptions-item label="Run ID">
              <CopyText :text="temporal.run_id" secondary />
            </a-descriptions-item>
            <a-descriptions-item label="Task Queue">{{ temporal.task_queue }}</a-descriptions-item>
            <a-descriptions-item :label="t('runDetail.temporalModal.historyLength')">{{ temporal.history_length }}</a-descriptions-item>
            <a-descriptions-item :label="t('runDetail.temporalModal.start')">
              <TimeText :value="temporal.start_time" />
            </a-descriptions-item>
            <a-descriptions-item :label="t('runDetail.temporalModal.close')">
              <TimeText :value="temporal.close_time" />
            </a-descriptions-item>
          </a-descriptions>

          <div class="mi-section-title" style="margin-top: 16px">
            {{ t('runDetail.temporalModal.pendingActivities', { count: temporal.pending_activities.length }) }}
          </div>
          <a-table
            row-key="activity_id"
            size="small"
            :columns="temporalColumns"
            :data-source="temporal.pending_activities"
            :pagination="false"
          >
            <template #bodyCell="{ column, record }">
              <template v-if="column.key === 'state'">
                <a-tag>{{ record.state }}</a-tag>
              </template>
              <template v-else-if="column.key === 'scheduled_time'">
                <TimeText :value="record.scheduled_time" />
              </template>
            </template>
            <template #emptyText>
              <a-empty :description="t('runDetail.temporalModal.noPending')" />
            </template>
          </a-table>
        </template>
        <a-empty v-else-if="!temporalLoading" :description="t('runDetail.temporalModal.noData')" />
      </a-spin>
    </a-modal>

    <!-- 参数配置（configs）审查：只读 -->
    <a-modal
      v-model:open="configModalOpen"
      :title="t('runDetail.configModal.title')"
      :footer="null"
      width="720"
    >
      <template v-if="configRowsList.length">
        <a-table
          row-key="name"
          size="small"
          :columns="configColumns"
          :data-source="configRowsList"
          :pagination="false"
        >
          <template #bodyCell="{ column, record }">
            <template v-if="column.key === 'name'">
              <span class="mi-mono">{{ record.name }}</span>
            </template>
            <template v-else-if="column.key === 'valueText'">
              <a-tooltip v-if="String(record.valueText).length > 60" :title="record.valueText">
                <span class="mi-mono">{{ record.valueText }}</span>
              </a-tooltip>
              <span v-else class="mi-mono">{{ record.valueText }}</span>
            </template>
          </template>
        </a-table>
        <div style="margin-top: 16px">
          <JsonBlock :data="deployment?.configs || {}" :title="t('runDetail.configModal.rawJson')" :default-open="false" />
        </div>
        <div class="mi-hint" style="margin-top: 8px">
          {{ t('runDetail.configModal.hint') }}
        </div>
      </template>
      <a-empty v-else :description="t('runDetail.configModal.empty')" />
    </a-modal>

    <!-- ComfyUI 添加权重（左：源模型目录树；右：实例权重目录树 → 相对软链 → 台账） -->
    <a-modal
      v-model:open="addWeightOpen"
      :title="t('runDetail.comfyui.addTitle')"
      :confirm-loading="addWeightSubmitting"
      :ok-text="t('runDetail.comfyui.addSelected', { count: checkedCount })"
      width="960px"
      @ok="submitAddWeight"
    >
      <a-form layout="vertical">
        <a-row :gutter="12">
          <a-col :span="12">
            <a-form-item :label="t('runDetail.comfyui.form.source')">
              <a-radio-group v-model:value="addForm.source" button-style="solid" @change="onAddWeightPick">
                <a-radio-button value="hf">HuggingFace</a-radio-button>
                <a-radio-button value="ms">ModelScope</a-radio-button>
              </a-radio-group>
            </a-form-item>
          </a-col>
          <a-col :span="12">
            <a-form-item :label="t('runDetail.comfyui.form.weight')">
              <a-select
                v-model:value="addForm.weight_id"
                show-search
                :options="variantOptions"
                :placeholder="t('runDetail.comfyui.form.weightPh')"
                @change="onAddWeightPick"
              />
            </a-form-item>
          </a-col>
        </a-row>
      </a-form>

      <div class="mi-cf-panes">
        <div class="mi-cf-pane">
          <div class="mi-cf-pane__head">
            <span class="mi-cf-pane__title">{{ t('runDetail.comfyui.pane.source') }}</span>
            <span class="mi-hint">{{ t('runDetail.comfyui.selectedCount', { selected: checkedCount, total: sourceFileCount }) }}</span>
          </div>
          <a-spin :spinning="filesLoading">
            <div class="mi-cf-tree">
              <a-tree
                v-if="sourceFileTree.length"
                v-model:checked-keys="checkedFilePaths"
                checkable
                default-expand-all
                :tree-data="sourceFileTree"
                :field-names="treeFieldNames"
                :selectable="false"
                :height="300"
              />
              <a-empty
                v-else
                :description="addForm.weight_id ? t('runDetail.comfyui.pane.sourceEmpty') : t('runDetail.comfyui.pane.pickWeight')"
              />
            </div>
          </a-spin>
        </div>
        <div class="mi-cf-pane">
          <div class="mi-cf-pane__head">
            <span class="mi-cf-pane__title">{{ t('runDetail.comfyui.pane.instance') }}</span>
            <span class="mi-hint">{{ t('runDetail.comfyui.instanceCount', { count: comfyuiWeights.length }) }}</span>
          </div>
          <div class="mi-cf-tree">
            <a-tree
              v-if="instanceWeightTree.length"
              default-expand-all
              :tree-data="instanceWeightTree"
              :field-names="treeFieldNames"
              :selectable="false"
              :height="300"
            />
            <a-empty v-else :description="t('runDetail.comfyui.pane.instanceEmpty')" />
          </div>
        </div>
      </div>

      <a-form layout="vertical" style="margin-top: 12px">
        <a-row :gutter="12">
          <a-col :span="12">
            <a-form-item :label="t('runDetail.comfyui.form.type')">
              <a-auto-complete
                v-model:value="addForm.target_type"
                :options="typeOptions"
                :placeholder="t('runDetail.comfyui.form.typePh')"
                :disabled="addForm.auto_infer"
              />
            </a-form-item>
          </a-col>
          <a-col :span="12">
            <a-form-item :label="t('runDetail.comfyui.form.linkName')">
              <a-input
                v-model:value="addForm.link_name"
                :disabled="checkedCount > 1"
                :placeholder="t('runDetail.comfyui.form.linkNamePh')"
              />
            </a-form-item>
          </a-col>
        </a-row>
        <a-form-item style="margin-bottom: 0">
          <a-checkbox v-model:checked="addForm.auto_infer">{{ t('runDetail.comfyui.form.autoInfer') }}</a-checkbox>
          <div class="mi-hint" style="margin-top: 4px">{{ t('runDetail.comfyui.form.typeHint') }}</div>
        </a-form-item>
      </a-form>
    </a-modal>
  </PageShell>
</template>

<style scoped>
.mi-error {
  color: var(--mi-error);
  font-size: 12px;
}
.mi-node-tag {
  margin-bottom: 4px;
}
/* ---------- ComfyUI 权重：双栏迁移弹窗 ---------- */
.mi-cf-panes {
  display: flex;
  gap: 12px;
  align-items: stretch;
}
.mi-cf-pane {
  flex: 1;
  min-width: 0;
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius);
  background: var(--mi-surface);
  display: flex;
  flex-direction: column;
}
.mi-cf-pane__head {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  padding: 8px 12px;
  border-bottom: 1px solid var(--mi-border);
}
.mi-cf-pane__title {
  font-size: 13px;
  font-weight: 600;
  color: var(--mi-text);
}
.mi-cf-tree {
  padding: 8px;
  height: 316px;
  overflow: auto;
}
@media (max-width: 720px) {
  .mi-cf-panes {
    flex-direction: column;
  }
}
/* ---------- 运行与步骤：左右主从布局 ---------- */
.mi-split {
  display: flex;
  align-items: flex-start;
  gap: 16px;
}
.mi-split__aside {
  width: 340px;
  flex: none;
  position: sticky;
  top: 8px;
}
.mi-split__main {
  flex: 1;
  min-width: 0;
}
/* 窄宽回退：取消左右分栏，恢复上下堆叠并保持信息可达 */
@media (max-width: 960px) {
  .mi-split {
    flex-direction: column;
  }
  .mi-split__aside {
    width: auto;
    align-self: stretch;
    position: static;
  }
}
.mi-section-title {
  font-size: 14px;
  font-weight: 600;
  color: var(--mi-text);
  margin-bottom: 8px;
}

/* ---------- 状态概览 ---------- */
.mi-ov {
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-lg);
  background:
    linear-gradient(135deg, rgba(79, 107, 255, 0.06), transparent 55%),
    var(--mi-surface);
  padding: 16px;
  margin-bottom: 16px;
}
.mi-ov__main {
  display: flex;
  align-items: center;
  gap: 14px;
}
.mi-ov__id {
  min-width: 0;
  flex: 1;
}
.mi-ov__name {
  font-size: 18px;
  font-weight: 600;
  color: var(--mi-text);
  line-height: 1.3;
}
.mi-ov__sub {
  display: flex;
  align-items: center;
  gap: 6px;
  margin-top: 2px;
}
.mi-ov__badges {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: none;
}
.mi-ov__grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 12px 20px;
  margin-top: 16px;
  padding-top: 14px;
  border-top: 1px dashed var(--mi-border);
}
.mi-kv--wide {
  grid-column: 1 / -1;
}
.mi-kv__k {
  font-size: 12px;
  color: var(--mi-text-secondary);
  margin-bottom: 3px;
}
.mi-kv__v {
  font-size: 13px;
  color: var(--mi-text);
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}
.mi-ov__error {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px dashed var(--mi-border);
}

/* ---------- 本地反代 ---------- */
.mi-proxy {
  margin-top: 14px;
  padding: 12px 14px;
  border: 1px solid var(--mi-border);
  border-radius: var(--mi-radius-md);
  background: var(--mi-surface-2);
}
.mi-proxy__head {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.mi-proxy__title {
  font-size: 13px;
  font-weight: 600;
  color: var(--mi-text);
}
.mi-proxy__meta {
  display: flex;
  align-items: center;
  gap: 20px;
  flex-wrap: wrap;
  margin-top: 10px;
}
.mi-proxy__access {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
  margin-top: 8px;
}

/* 抽屉内 TerminalBlock 的插槽内容配色（头部为深色） */
.mi-term-check {
  color: #cfd3da;
  font-size: 13px;
}
.mi-term-meta {
  color: #b8bcc4;
  font-size: 12px;
}

/* ---------- 运行步骤时间线 ---------- */
.mi-tl-head {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin: 0 0 10px;
  padding: 10px 0;
  border-bottom: 1px dashed var(--mi-border);
  /* 吸顶：滚动步骤时保持 run 上下文与「Temporal 活动」入口可见 */
  position: sticky;
  top: 0;
  z-index: 2;
  background: var(--mi-surface);
}
.mi-tl-head__meta {
  display: flex;
  align-items: center;
  gap: 10px;
  flex-wrap: wrap;
  flex: 1;
}
.mi-tl {
  display: flex;
  flex-direction: column;
  gap: 4px;
}
.mi-tl-item {
  display: flex;
  gap: 12px;
  padding: 8px 10px;
  border-radius: var(--mi-radius-md);
}
.mi-tl-item--running {
  background: var(--mi-surface-2);
  box-shadow: inset 3px 0 0 var(--mi-primary);
}
.mi-tl-dot {
  flex: none;
  width: 10px;
  height: 10px;
  margin-top: 8px;
  border-radius: 50%;
  background: var(--mi-border);
}
.mi-tl-item--running .mi-tl-dot {
  background: var(--mi-primary);
}
.mi-tl-body {
  flex: 1;
  min-width: 0;
}
.mi-tl-line {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-wrap: wrap;
}
.mi-tl-name {
  font-weight: 600;
  color: var(--mi-text);
  font-size: 13px;
}
.mi-tl-detail {
  margin-top: 6px;
}
/* 步骤错误折叠：默认收起，避免长错误（含节点脚本输出）撑高时间线 */
.mi-tl-error {
  margin-top: 2px;
}
.mi-tl-error__pre {
  margin: 0;
  padding: 8px 10px;
  max-height: 360px;
  overflow: auto;
  white-space: pre-wrap;
  word-break: break-all;
  color: var(--mi-error);
  background: var(--mi-error-soft);
  border: 1px solid rgba(220, 38, 38, 0.18);
  border-radius: var(--mi-radius-sm);
  font-size: 12px;
  line-height: 1.55;
  font-family: 'SFMono-Regular', 'JetBrains Mono', Menlo, Consolas, monospace;
}
.mi-tag-soft {
  color: var(--mi-text-secondary);
  background: var(--mi-surface-2);
  border-color: var(--mi-border);
}
</style>
