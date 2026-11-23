<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, reactive, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { message, Modal } from 'ant-design-vue';
import { DownOutlined, PlusOutlined, ReloadOutlined, SearchOutlined, SyncOutlined } from '@ant-design/icons-vue';
import PageShell from '@/components/PageShell.vue';
import StatPill from '@/components/StatPill.vue';
import StatusTag from '@/components/StatusTag.vue';
import TimeText from '@/components/TimeText.vue';
import CopyText from '@/components/CopyText.vue';
import NodeEditModal from '@/components/NodeEditModal.vue';
import {
  DEFAULT_NODE_EXPORTER_PORT,
  DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
  createNode,
  deleteNode,
  listNodes,
  probeNode,
  rescanNode,
  testNode,
  type NodeCreateReq,
  type NodeItem,
  type NodeScanResult,
  type NodeStats,
} from '@/api/nodes';
import { DEFAULT_PAGE_SIZE, PAGE_SIZE_OPTIONS } from '@/api/paging';
import { createSeqGuard, debounce } from '@/utils/async';
import { useI18n } from 'vue-i18n';
import { resolveApiErrorMessage } from '@/i18n/api-error';
import { localizedTitle } from '@/platform/localized-title';
import { openDetailInNewTab } from '@/platform/open-tab';

const { t } = useI18n();

const router = useRouter();

const loading = ref(false);
/** 当前页节点（服务端分页） */
const rows = ref<NodeItem[]>([]);
/** 未筛选全量统计（页头统计条） */
const stats = ref<NodeStats>({ total: 0, online: 0, offline: 0, fscache: 0, busy: 0 });
/** 服务端分页元数据（`total` 为筛选后条数） */
const page = reactive({ page_num: 1, page_size: DEFAULT_PAGE_SIZE, total: 0 });
const seqGuard = createSeqGuard();

// ---------- 向导（添加节点） ----------
const wizardOpen = ref(false);
/** 0 基础信息 / 1 扫描环境 / 2 配置 / 3 确认 */
const step = ref(0);
const scanning = ref(false);
const saving = ref(false);
const scanned = ref<NodeScanResult | null>(null);

// ---------- 编辑（配置） ----------
const editOpen = ref(false);
const editing = ref<NodeItem | null>(null);

/** 工具条筛选 / 排序 */
const filters = reactive({
  keyword: '',
  status: 'all',
  sort: 'name' as 'name' | 'recent',
});

const form = reactive<NodeCreateReq>({
  name: '',
  host: '',
  ssh_port: 22,
  ssh_user: 'root',
  auth_method: 'password',
  ssh_password: '',
  ssh_key: '',
  sudo_password: '',
  hostname: '',
  socket_ifname: '',
  ib_hca: '',
  fscache: false,
  node_exporter_port: DEFAULT_NODE_EXPORTER_PORT,
  nvidia_gpu_exporter_port: DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
});

const columns = computed(() => [
  { title: t('nodes.column.name'), dataIndex: 'name', key: 'name', width: 160 },
  { title: t('nodes.column.host'), key: 'host', width: 150 },
  { title: t('nodes.column.arch'), key: 'arch', width: 90 },
  { title: t('nodes.column.gpu'), key: 'gpu', width: 180 },
  { title: t('nodes.column.auth'), key: 'auth', width: 90 },
  { title: 'SOCKET_IFNAME', key: 'socket_ifname', width: 150 },
  { title: 'IB_HCA', key: 'ib_hca', width: 110 },
  { title: 'fscache', key: 'fscache', width: 90 },
  { title: t('nodes.column.status'), key: 'status', width: 100 },
  { title: t('nodes.column.monitor'), key: 'monitor', width: 100 },
  { title: t('nodes.column.usage'), key: 'usage', width: 160 },
  { title: t('nodes.column.lastCheckedAt'), key: 'last_checked_at', width: 150 },
  { title: t('nodes.column.action'), key: 'action', width: 210, align: 'right' as const },
]);

/** 状态筛选下拉选项 */
const statusFilterOptions = computed(() => [
  { label: t('nodes.filter.allStatus'), value: 'all' },
  { label: t('nodes.filter.online'), value: 'online' },
  { label: t('nodes.filter.offline'), value: 'offline' },
]);

/** 排序下拉选项 */
const sortOptions = computed(() => [
  { label: t('nodes.sort.name'), value: 'name' },
  { label: t('nodes.sort.recent'), value: 'recent' },
]);

/** 扫描结果 GPU 明细表列 */
const gpuColumns = computed(() => [
  { title: '#', dataIndex: 'index', width: 60 },
  { title: t('nodes.gpuTable.model'), dataIndex: 'name' },
  { title: t('nodes.gpuTable.memory'), key: 'mem', width: 120 },
  { title: 'PCI', dataIndex: 'pci_bus_id', width: 160 },
]);

const ifaceOptions = computed(() =>
  (scanned.value?.interfaces || []).map((i) => ({
    label: t('common.ifaceLabel', { name: i.name, ips: i.ips.join(', ') }),
    value: i.name,
  })),
);
const ibOptions = computed(() =>
  (scanned.value?.ib_devices || []).map((d) => ({ label: d, value: d })),
);
const scannedHasGpu = computed(() => (scanned.value?.gpu?.devices?.length || 0) > 0);
const scannedIsNvidia = computed(() => scanned.value?.gpu?.vendor === 'nvidia');

// ---------- 工具条 ----------
const isFiltering = computed(() => !!filters.keyword.trim() || filters.status !== 'all');

/** 分页器配置（服务端分页） */
const pagination = computed(() => ({
  current: page.page_num,
  pageSize: page.page_size,
  total: page.total,
  showSizeChanger: true,
  pageSizeOptions: PAGE_SIZE_OPTIONS,
  showTotal: (total: number) => t('common.totalCount', { total }),
}));

/** 节点承载的运行配置名称（多值优先，旧后端回退单值） */
function usageDeploymentNames(n: NodeItem): string[] {
  const multi = (n.busy_deployment_names || []).filter(Boolean);
  if (multi.length) return multi;
  const single = n.busy_deployment_name || n.busy_deployment_id;
  return single ? [single] : [];
}

function usageText(n: NodeItem): string {
  return usageDeploymentNames(n).join(t('nodes.nodeJoin'));
}

function onTableChange(p: { current?: number; pageSize?: number }) {
  if (p.pageSize && p.pageSize !== page.page_size) {
    page.page_size = p.pageSize;
    page.page_num = 1;
  } else if (p.current) {
    page.page_num = p.current;
  }
  void load();
}

function filterIb(input: string, option: { label: string }): boolean {
  return option.label.toLowerCase().includes(input.toLowerCase());
}

async function load() {
  const seq = seqGuard.next();
  loading.value = true;
  try {
    const data = await listNodes({
      page_num: page.page_num,
      page_size: page.page_size,
      keyword: filters.keyword.trim(),
      status: filters.status,
      sort: filters.sort,
    });
    if (!seqGuard.isCurrent(seq)) return; // 已有更新的请求，丢弃过期响应
    rows.value = data.data;
    stats.value = data.stats;
    page.total = data.total;
    // 末页条目被删除等导致页码越界时回退
    if (data.pages > 0 && page.page_num > data.pages) {
      page.page_num = data.pages;
      void load();
    }
  } catch (e) {
    if (seqGuard.isCurrent(seq)) message.error(resolveApiErrorMessage(e));
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
watch(() => [filters.status, filters.sort], reloadFromFirstPage);
onBeforeUnmount(() => debouncedKeywordReload.cancel());

function resetForm() {
  form.name = '';
  form.host = '';
  form.ssh_port = 22;
  form.ssh_user = 'root';
  form.auth_method = 'password';
  form.ssh_password = '';
  form.ssh_key = '';
  form.sudo_password = '';
  form.hostname = '';
  form.socket_ifname = '';
  form.ib_hca = '';
  form.fscache = false;
  form.node_exporter_port = DEFAULT_NODE_EXPORTER_PORT;
  form.nvidia_gpu_exporter_port = DEFAULT_NVIDIA_GPU_EXPORTER_PORT;
  scanned.value = null;
}

function openCreate() {
  resetForm();
  step.value = 0;
  wizardOpen.value = true;
}

function openEdit(row: NodeItem) {
  editing.value = row;
  editOpen.value = true;
}

async function openDetail(row: NodeItem) {
  // 平台内优先经 platform.openTab 打开独立 Vben 标签页；桥接缺失 / 调用失败回退当前页
  const handled = await openDetailInNewTab({
    hashPath: `#/nodes/${row.id}`,
    title: localizedTitle('routes.nodeDetail', row.name),
  });
  if (!handled) void router.push(`/nodes/${row.id}`);
}

/** 第 1 步校验 */
function validateBasic(): boolean {
  if (!form.name.trim() || !form.host.trim()) {
    message.warning(t('nodes.msg.nameHostRequired'));
    return false;
  }
  if (!form.ssh_user?.trim()) {
    message.warning(t('nodes.msg.sshUserRequired'));
    return false;
  }
  if (form.auth_method === 'password' && !form.ssh_password) {
    message.warning(t('nodes.msg.sshPasswordRequired'));
    return false;
  }
  if (form.auth_method === 'key' && !(form.ssh_key ?? '').trim()) {
    message.warning(t('nodes.msg.sshKeyRequired'));
    return false;
  }
  return true;
}

/** 第 2 步：扫描环境（不落库） */
async function doScan() {
  if (!validateBasic()) return false;
  scanning.value = true;
  try {
    const scan = await probeNode({ ...form });
    scanned.value = scan;
    if (!form.hostname) form.hostname = scan.hostname;
    if (!form.socket_ifname && scan.default_socket_ifname) form.socket_ifname = scan.default_socket_ifname;
    if (!form.ib_hca && scan.suggested_ib_hca) form.ib_hca = scan.suggested_ib_hca;
    message.success(
      t('nodes.msg.scanDone', {
        arch: scan.arch,
        kernel: scan.kernel_version,
        count: scan.interfaces.length,
      }) +
        (scan.gpu.vendor
          ? t('nodes.msg.scanGpuSuffix', { vendor: scan.gpu.vendor })
          : t('nodes.msg.scanNoGpuSuffix')),
    );
    return true;
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
    return false;
  } finally {
    scanning.value = false;
  }
}

async function gotoScanStep() {
  const ok = await doScan();
  if (ok) step.value = 1;
}

async function rescanInWizard() {
  await doScan();
}

async function submitCreate() {
  if (!scanned.value) {
    message.warning(t('nodes.msg.needScan'));
    return;
  }
  saving.value = true;
  try {
    await createNode({ ...form, name: form.name.trim(), host: form.host.trim() });
    message.success(t('nodes.msg.nodeAdded'));
    wizardOpen.value = false;
    await load();
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    saving.value = false;
  }
}

async function onTest(row: NodeItem) {
  try {
    await testNode(row.id);
    message.success(t('nodes.msg.connectOk', { host: row.host }));
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    await load();
  }
}

async function onRescan(row: NodeItem) {
  try {
    await rescanNode(row.id);
    message.success(t('nodes.msg.rescanDone', { host: row.host }));
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    await load();
  }
}

function onDelete(row: NodeItem) {
  Modal.confirm({
    title: t('nodes.deleteTitle'),
    content: t('nodes.deleteConfirm', { name: row.name, host: row.host }),
    okText: t('common.delete'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      await deleteNode(row.id);
      message.success(t('nodes.msg.nodeDeleted'));
      await load();
    },
  });
}

function onMoreMenu(e: { key: string | number }, row: NodeItem) {
  switch (e.key) {
    case 'detail':
      void openDetail(row);
      break;
    case 'edit':
      openEdit(row);
      break;
    case 'test':
      void onTest(row);
      break;
    case 'delete':
      onDelete(row);
      break;
  }
}

onMounted(load);
</script>

<template>
  <PageShell
    :title="t('nodes.title')"
    :subtitle="t('nodes.subtitle')"
    :loading="loading"
  >
    <template #stats>
      <StatPill :label="t('nodes.stats.total')" :value="stats.total" tone="primary" />
      <StatPill :label="t('nodes.stats.online')" :value="stats.online" />
      <StatPill :label="t('nodes.stats.offline')" :value="stats.offline" />
      <StatPill :label="t('nodes.stats.busy')" :value="stats.busy" />
      <StatPill :label="t('nodes.stats.fscache')" :value="stats.fscache" />
    </template>

    <template #actions>
      <a-button @click="load">
        <ReloadOutlined />
        {{ t('common.refresh') }}
      </a-button>
      <a-button type="primary" @click="openCreate">
        <PlusOutlined />
        {{ t('nodes.addNode') }}
      </a-button>
    </template>

    <div class="mi-toolbar">
      <a-input
        v-model:value="filters.keyword"
        :placeholder="t('nodes.searchPlaceholder')"
        allow-clear
        style="width: 280px"
      >
        <template #prefix><SearchOutlined /></template>
      </a-input>
      <a-select
        v-model:value="filters.status"
        style="width: 130px"
        :options="statusFilterOptions"
      />
      <a-select
        v-model:value="filters.sort"
        style="width: 150px; margin-left: auto"
        :options="sortOptions"
      />
    </div>

    <a-table
      row-key="id"
      :columns="columns"
      :data-source="rows"
      :pagination="pagination"
      :scroll="{ x: 1820 }"
      @change="onTableChange"
    >
      <template #bodyCell="{ column, record }">
        <template v-if="column.key === 'name'">
          <a class="mi-link" @click="openDetail(record)">{{ record.hostname || record.name }}</a>
        </template>

        <template v-else-if="column.key === 'host'">
          <CopyText :text="record.host" />
          <div v-if="record.profile" class="mi-hint">{{ record.profile }}</div>
        </template>

        <template v-else-if="column.key === 'arch'">
          {{ record.arch || t('common.none') }}
        </template>

        <template v-else-if="column.key === 'gpu'">
          <template v-if="record.gpu_vendor">
            <StatusTag kind="gpuVendor" :status="record.gpu_vendor" />
            <div class="mi-hint">
              {{ t('nodes.gpuModelCount', { model: record.gpu_model, count: record.gpu_devices.length }) }}
            </div>
          </template>
          <span v-else class="mi-hint">{{ t('common.notRecognized') }}</span>
        </template>

        <template v-else-if="column.key === 'auth'">
          <StatusTag kind="authMethod" :status="record.auth_method" />
        </template>

        <template v-else-if="column.key === 'socket_ifname'">
          <a-tag v-if="record.socket_ifname" class="mi-tag-soft">{{ record.socket_ifname }}</a-tag>
          <span v-else>{{ t('common.none') }}</span>
        </template>

        <template v-else-if="column.key === 'ib_hca'">
          <a-tag v-if="record.ib_hca" color="blue">{{ record.ib_hca }}</a-tag>
          <span v-else>{{ t('common.none') }}</span>
        </template>

        <template v-else-if="column.key === 'fscache'">
          <a-tag v-if="record.fscache" color="green">{{ t('common.enabledTag') }}</a-tag>
          <span v-else class="mi-hint">{{ t('common.disabledTag') }}</span>
        </template>

        <template v-else-if="column.key === 'status'">
          <StatusTag kind="node" :status="record.status" />
        </template>

        <template v-else-if="column.key === 'monitor'">
          <StatusTag kind="monitor" :status="record.monitor_state || 'pending'" />
        </template>

        <template v-else-if="column.key === 'usage'">
          <StatusTag kind="usage" :status="record.busy ? 'busy' : 'idle'" />
          <div v-if="record.busy" class="mi-hint" :title="usageText(record)">
            {{ t('nodes.runningConfigs', { names: usageText(record) }) }}
          </div>
        </template>

        <template v-else-if="column.key === 'last_checked_at'">
          <TimeText :value="record.last_checked_at" />
        </template>

        <template v-else-if="column.key === 'action'">
          <a-space :size="4">
            <a-button size="small" type="link" @click="openDetail(record)">{{ t('common.detail') }}</a-button>
            <a-button size="small" type="link" @click="onRescan(record)">{{ t('nodes.rescan') }}</a-button>
            <a-dropdown :trigger="['click']">
              <a-button size="small" type="link">
                {{ t('common.more') }}
                <DownOutlined />
              </a-button>
              <template #overlay>
                <a-menu @click="(e: any) => onMoreMenu(e, record)">
                  <a-menu-item key="edit">{{ t('nodes.editConfig') }}</a-menu-item>
                  <a-menu-item key="test">{{ t('nodes.testConnection') }}</a-menu-item>
                  <a-menu-divider />
                  <a-menu-item key="delete" danger>{{ t('common.delete') }}</a-menu-item>
                </a-menu>
              </template>
            </a-dropdown>
          </a-space>
        </template>
      </template>
      <template #emptyText>
        <a-empty :description="t('nodes.noNodes')">
          <a-button type="primary" @click="openCreate">{{ t('nodes.addNode') }}</a-button>
        </a-empty>
      </template>
    </a-table>

    <div v-if="stats.total && isFiltering" class="mi-footer-hint">
      {{ t('nodes.filteredCount', { shown: page.total, total: stats.total }) }}
    </div>

    <!-- 添加节点：多步向导 -->
    <a-modal
      v-model:open="wizardOpen"
      :title="t('nodes.addNode')"
      width="760px"
      :mask-closable="false"
      :footer="null"
    >
      <a-steps size="small" :current="step" style="margin-bottom: 20px">
        <a-step :title="t('nodes.step.basic')" />
        <a-step :title="t('nodes.step.scan')" />
        <a-step :title="t('nodes.step.config')" />
        <a-step :title="t('nodes.step.confirm')" />
      </a-steps>

      <!-- Step 0 基础信息 -->
      <template v-if="step === 0">
        <a-form layout="vertical">
          <a-row :gutter="12">
            <a-col :span="12">
              <a-form-item :label="t('nodes.form.nodeName')" required>
                <a-input v-model:value="form.name" :placeholder="t('nodes.form.nodeNamePlaceholder')" />
              </a-form-item>
            </a-col>
            <a-col :span="12">
              <a-form-item :label="t('nodes.form.host')" required>
                <a-input v-model:value="form.host" :placeholder="t('nodes.form.hostPlaceholder')" />
              </a-form-item>
            </a-col>
          </a-row>
          <a-row :gutter="12">
            <a-col :span="12">
              <a-form-item :label="t('nodes.form.sshPort')">
                <a-input-number v-model:value="form.ssh_port" :min="1" :max="65535" style="width: 100%" />
              </a-form-item>
            </a-col>
            <a-col :span="12">
              <a-form-item :label="t('nodes.form.sshUser')">
                <a-input v-model:value="form.ssh_user" autocomplete="username" />
              </a-form-item>
            </a-col>
          </a-row>
          <a-form-item :label="t('nodes.form.authMethod')">
            <a-radio-group v-model:value="form.auth_method">
              <a-radio-button value="password">{{ t('nodes.form.rootPassword') }}</a-radio-button>
              <a-radio-button value="key">{{ t('nodes.form.rootKey') }}</a-radio-button>
            </a-radio-group>
          </a-form-item>
          <a-form-item v-if="form.auth_method === 'password'" :label="t('nodes.form.rootPassword')" required>
            <a-input-password v-model:value="form.ssh_password" autocomplete="new-password" />
          </a-form-item>
          <a-form-item v-else :label="t('nodes.form.rootKey')" required>
            <a-textarea
              v-model:value="form.ssh_key"
              :rows="5"
              placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
            />
          </a-form-item>
          <a-form-item
            v-if="form.ssh_user?.trim() && form.ssh_user.trim() !== 'root'"
            :label="t('nodes.form.sudoPassword')"
            :extra="t('nodes.form.sudoHint')"
          >
            <a-input-password v-model:value="form.sudo_password" autocomplete="new-password" />
          </a-form-item>
        </a-form>
      </template>

      <!-- Step 1 扫描环境 -->
      <template v-else-if="step === 1">
        <div class="mi-scan-head">
          <a-button :loading="scanning" @click="rescanInWizard"><SyncOutlined />{{ t('nodes.rescan') }}</a-button>
        </div>
        <template v-if="scanned">
          <a-descriptions size="small" :column="2" bordered>
            <a-descriptions-item :label="t('nodes.scan.hostname')">
              {{ scanned.hostname || t('common.none') }}
            </a-descriptions-item>
            <a-descriptions-item :label="t('nodes.scan.arch')">
              {{ scanned.arch || t('common.none') }}
            </a-descriptions-item>
            <a-descriptions-item :label="t('nodes.scan.kernelVersion')">
              {{ scanned.kernel_version || t('common.none') }}
            </a-descriptions-item>
            <a-descriptions-item :label="t('nodes.scan.gateway')">
              {{ scanned.gateway || t('common.none') }}
            </a-descriptions-item>
            <a-descriptions-item :label="t('nodes.scan.interfaces')" :span="2">
              <span v-if="scanned.interfaces.length">
                {{ scanned.interfaces.map((i) => `${i.name}: ${i.ips.join(',')}`).join(t('common.listSep')) }}
              </span>
              <span v-else>{{ t('common.none') }}</span>
            </a-descriptions-item>
            <a-descriptions-item :label="t('nodes.scan.ibDevices')" :span="2">
              <span v-if="scanned.ib_devices.length">{{ scanned.ib_devices.join(', ') }}</span>
              <span v-else>{{ t('nodes.scan.noIbDevices') }}</span>
            </a-descriptions-item>
          </a-descriptions>

          <div class="mi-form-section" style="margin-top: 16px">GPU</div>
          <template v-if="scannedHasGpu">
            <div class="mi-gpu-head">
              <StatusTag kind="gpuVendor" :status="scanned.gpu.vendor" />
              <span class="mi-hint">
                {{ t('nodes.scan.driver', { version: scanned.gpu.driver_version || t('common.none') }) }}
              </span>
            </div>
            <a-table
              size="small"
              row-key="index"
              :data-source="scanned.gpu.devices"
              :pagination="false"
              :columns="gpuColumns"
            >
              <template #bodyCell="{ column, record }">
                <template v-if="column.key === 'mem'">
                  {{ record.memory_total_mib || t('common.none') }}
                </template>
              </template>
            </a-table>
          </template>
          <a-alert
            v-else
            type="info"
            show-icon
            :message="t('nodes.scan.noGpu')"
            :description="t('nodes.scan.noGpuDesc')"
          />
        </template>
        <a-empty v-else :description="t('nodes.scan.notScanned')" />
      </template>

      <!-- Step 2 配置 -->
      <template v-else-if="step === 2">
        <a-form layout="vertical">
          <div class="mi-form-section">{{ t('nodes.config.general') }}</div>
          <a-form-item :label="t('nodes.config.enableFscache')">
            <a-switch v-model:checked="form.fscache" />
            <span class="mi-hint" style="margin-left: 8px">
              {{ t('nodes.config.fscacheHint') }}
            </span>
          </a-form-item>

          <template v-if="scannedIsNvidia">
            <div class="mi-form-section">{{ t('nodes.config.nvidia') }}</div>
            <a-row :gutter="12">
              <a-col :span="12">
                <a-form-item :label="t('nodes.config.socketIfname')">
                  <a-select
                    v-model:value="form.socket_ifname"
                    :options="ifaceOptions"
                    allow-clear
                    :placeholder="t('nodes.config.socketIfnamePlaceholder')"
                  />
                </a-form-item>
              </a-col>
              <a-col :span="12">
                <a-form-item :label="t('nodes.config.ibHca')">
                  <a-select
                    v-model:value="form.ib_hca"
                    :options="ibOptions"
                    :show-search="true"
                    allow-clear
                    :placeholder="t('nodes.config.ibHcaPlaceholder')"
                    :filter-option="filterIb"
                  />
                </a-form-item>
              </a-col>
            </a-row>
          </template>

          <a-collapse ghost>
            <a-collapse-panel key="advanced" :header="t('nodes.config.advanced')">
              <a-form-item :label="t('nodes.config.nodeExporterPort')">
                <a-input-number
                  v-model:value="form.node_exporter_port"
                  :min="1024"
                  :max="65535"
                  style="width: 200px"
                />
                <span class="mi-hint" style="margin-left: 8px">{{ t('nodes.config.nodeExporterHint') }}</span>
              </a-form-item>
              <a-form-item v-if="scannedIsNvidia" :label="t('nodes.config.nvidiaGpuExporterPort')">
                <a-input-number
                  v-model:value="form.nvidia_gpu_exporter_port"
                  :min="1024"
                  :max="65535"
                  style="width: 200px"
                />
                <span class="mi-hint" style="margin-left: 8px">
                  {{ t('nodes.config.nvidiaGpuExporterHint') }}
                </span>
              </a-form-item>
            </a-collapse-panel>
          </a-collapse>
        </a-form>
      </template>

      <!-- Step 3 确认 -->
      <template v-else>
        <a-descriptions size="small" :column="2" bordered>
          <a-descriptions-item :label="t('nodes.confirm.name')">{{ form.name }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodes.form.host')">
            {{ form.host }}:{{ form.ssh_port }}
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodes.form.sshUser')">
            {{ form.ssh_user }}
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodes.form.authMethod')">
            {{ form.auth_method === 'key' ? t('nodes.authMethod.key') : t('nodes.authMethod.password') }}
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodes.scan.arch')">
            {{ scanned?.arch || t('common.none') }}
          </a-descriptions-item>
          <a-descriptions-item label="GPU">
            {{
              scanned?.gpu?.vendor
                ? t('nodes.confirm.gpuValue', {
                    vendor: scanned.gpu.vendor,
                    count: scanned.gpu.devices.length,
                  })
                : t('common.notRecognized')
            }}
          </a-descriptions-item>
          <a-descriptions-item label="fscache">
            {{ form.fscache ? t('common.enabled') : t('common.disabled') }}
          </a-descriptions-item>
          <a-descriptions-item label="SOCKET_IFNAME">
            {{ form.socket_ifname || t('common.none') }}
          </a-descriptions-item>
          <a-descriptions-item label="IB_HCA">{{ form.ib_hca || t('common.none') }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodes.confirm.nodeExporterPort')">
            {{ form.node_exporter_port }}
          </a-descriptions-item>
          <a-descriptions-item v-if="scannedIsNvidia" :label="t('nodes.confirm.nvidiaGpuExporterPort')">
            {{ form.nvidia_gpu_exporter_port }}
          </a-descriptions-item>
        </a-descriptions>
      </template>

      <div class="mi-wizard-footer">
        <a-button v-if="step > 0" @click="step -= 1">{{ t('common.back') }}</a-button>
        <a-button @click="wizardOpen = false">{{ t('common.cancel') }}</a-button>
        <a-button v-if="step === 0" type="primary" :loading="scanning" @click="gotoScanStep">
          {{ t('nodes.next.scan') }}
        </a-button>
        <a-button v-else-if="step === 1" type="primary" :disabled="!scanned" @click="step = 2">
          {{ t('nodes.next.config') }}
        </a-button>
        <a-button v-else-if="step === 2" type="primary" @click="step = 3">
          {{ t('nodes.next.confirm') }}
        </a-button>
        <a-button v-else type="primary" :loading="saving" @click="submitCreate">
          {{ t('nodes.confirmAdd') }}
        </a-button>
      </div>
    </a-modal>

    <NodeEditModal v-model:open="editOpen" :node="editing" @saved="load" />
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
.mi-strong {
  font-weight: 600;
}
.mi-link {
  font-weight: 600;
  cursor: pointer;
}
.mi-tag-soft {
  color: var(--mi-text-secondary);
  background: var(--mi-surface-2);
  border-color: var(--mi-border);
}
.mi-hint {
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
}
.mi-footer-hint {
  margin-top: 12px;
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
  text-align: right;
}
.mi-wizard-footer {
  display: flex;
  justify-content: flex-end;
  gap: 8px;
  margin-top: 20px;
}
.mi-form-section {
  margin: 8px 0 12px;
  font-size: 13px;
  font-weight: 600;
  color: var(--mi-text);
  border-left: 3px solid var(--mi-primary);
  padding-left: 8px;
}
.mi-scan-head {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 12px;
}
.mi-gpu-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}
</style>
