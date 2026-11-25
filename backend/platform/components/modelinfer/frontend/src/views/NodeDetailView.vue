<script setup lang="ts">
/** 节点详情页：基础配置 + 节点监控（按组件）+ 通用指标板（系统 + GPU）。 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue';
import { useRoute, useRouter } from 'vue-router';
import { useI18n } from 'vue-i18n';
import { message, Modal } from 'ant-design-vue';
import {
  DeleteOutlined,
  EditOutlined,
  ProfileOutlined,
  ReloadOutlined,
  SyncOutlined,
  ThunderboltOutlined,
} from '@ant-design/icons-vue';
import PageShell from '@/components/PageShell.vue';
import StatusTag from '@/components/StatusTag.vue';
import TimeText from '@/components/TimeText.vue';
import CopyText from '@/components/CopyText.vue';
import NodeEditModal from '@/components/NodeEditModal.vue';
import MetricsBoard from '@/components/MetricsBoard.vue';
import {
  deleteNode,
  deployNodeMonitor,
  getNode,
  getNodeMetrics,
  getNodeMonitor,
  rescanNode,
  testNode,
  type NodeItem,
  type NodeMonitor,
} from '@/api/nodes';
import { useMetrics } from '@/composables/useMetrics';
import { monitorComponentLabelKey, nodeMetricsProfile } from '@/utils/metricProfiles';
import { reportApiError } from '@/i18n/api-error';

const { t } = useI18n();
const route = useRoute();
const router = useRouter();
const nodeId = computed(() => String(route.params.id || ''));

const loading = ref(false);
const node = ref<NodeItem | null>(null);
const editOpen = ref(false);

// ---------- 监控 ----------
const monitors = ref<NodeMonitor[]>([]);
/** 部署进度弹窗中查看的组件 */
const progressComponent = ref<NodeMonitor | null>(null);
const deploying = ref(false);
let pollTimer: ReturnType<typeof setInterval> | null = null;

// ---------- 指标（通用取数 composable）----------
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
  (params) => getNodeMetrics(nodeId.value, params),
  (msg) => message.error(msg),
);
const profile = computed(() => nodeMetricsProfile(node.value));

async function load() {
  loading.value = true;
  try {
    node.value = await getNode(nodeId.value);
  } catch (e) {
    reportApiError(e);
  } finally {
    loading.value = false;
  }
}

async function loadMonitor() {
  try {
    const data = await getNodeMonitor(nodeId.value);
    monitors.value = data.components;
    // 同步进度弹窗中的组件（避免展示旧快照）
    if (progressComponent.value) {
      progressComponent.value =
        data.components.find((c) => c.component === progressComponent.value?.component) ?? null;
    }
    managePolling();
  } catch (e) {
    reportApiError(e);
  }
}

function managePolling() {
  const shouldPoll = monitors.value.some((m) => m.state === 'deploying') || deploying.value;
  if (shouldPoll && !pollTimer) {
    pollTimer = setInterval(() => {
      void loadMonitor();
    }, 3000);
  } else if (!shouldPoll && pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

async function onTest() {
  try {
    node.value = await testNode(nodeId.value);
    message.success(t('nodeDetail.msg.connectionOk'));
  } catch (e) {
    reportApiError(e);
    await load();
  }
}

async function onRescan() {
  try {
    node.value = await rescanNode(nodeId.value);
    message.success(t('nodeDetail.msg.rescanDone'));
  } catch (e) {
    reportApiError(e);
  }
}

async function onDeployMonitor() {
  deploying.value = true;
  try {
    const data = await deployNodeMonitor(nodeId.value);
    monitors.value = data.components;
    message.success(t('nodeDetail.msg.deployTriggered'));
  } catch (e) {
    reportApiError(e);
    await loadMonitor();
  } finally {
    deploying.value = false;
    managePolling();
  }
}

function onDelete() {
  Modal.confirm({
    title: t('nodeDetail.delete.title'),
    content: t('nodeDetail.delete.content', { name: node.value?.name, host: node.value?.host }),
    okText: t('common.delete'),
    okType: 'danger',
    cancelText: t('common.cancel'),
    async onOk() {
      await deleteNode(nodeId.value);
      message.success(t('nodeDetail.msg.deleted'));
      void router.push('/nodes');
    },
  });
}

const hasGpu = computed(() => (node.value?.gpu_devices?.length || 0) > 0);

/** GPU 列表列定义（随语言切换重算） */
const gpuColumns = computed(() => [
  { title: '#', dataIndex: 'index', width: 60 },
  { title: t('nodeDetail.gpu.columns.model'), dataIndex: 'name' },
  { title: t('nodeDetail.gpu.columns.memory'), key: 'mem', width: 140 },
  { title: 'PCI', dataIndex: 'pci_bus_id', width: 180 },
]);

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

/** 监控组件显示名（内建组件本地化，未知组件回显原值） */
function componentLabel(component: string): string {
  const key = monitorComponentLabelKey(component);
  return key ? t(key) : component || '-';
}

/** 组件监听端口（取节点高级配置） */
function portOf(m: NodeMonitor): number | string {
  if (m.component === 'node-exporter') return node.value?.node_exporter_port ?? '-';
  if (m.component === 'nvidia-gpu-exporter') return node.value?.nvidia_gpu_exporter_port ?? '-';
  return '-';
}

onMounted(async () => {
  await load();
  await loadMonitor();
  await loadMetrics();
});

onBeforeUnmount(() => {
  if (pollTimer) clearInterval(pollTimer);
});

function onEditSaved(n: NodeItem) {
  node.value = n;
}
</script>

<template>
  <PageShell
    :title="node ? (node.hostname || node.name) : t('nodeDetail.title')"
    :subtitle="node ? [node.host,node.profile].filter(Boolean).join(' · ') : ''"
    :loading="loading"
  >
    <template #actions>
      <a-button @click="onTest"><ReloadOutlined />{{ t('nodeDetail.actions.testConnection') }}</a-button>
      <a-button @click="onRescan"><SyncOutlined />{{ t('nodeDetail.actions.rescan') }}</a-button>
      <a-button type="primary" @click="editOpen = true"><EditOutlined />{{ t('nodeDetail.actions.editConfig') }}</a-button>
      <a-button danger @click="onDelete"><DeleteOutlined />{{ t('common.delete') }}</a-button>
    </template>

    <template v-if="node">
      <a-card size="small" :title="t('nodeDetail.basic.title')" class="mi-block">
        <a-descriptions size="small" :column="3" bordered>
          <a-descriptions-item :label="t('nodeDetail.basic.nodeName')">{{ node.name }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.host')"><CopyText :text="node.host" /></a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.connectionStatus')">
            <StatusTag kind="node" :status="node.status" />
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.sshUser')">{{ node.ssh_user }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.sshPort')">{{ node.ssh_port }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.authMethod')">
            <StatusTag kind="authMethod" :status="node.auth_method" />
            <span class="mi-hint" style="margin-left: 8px">
              {{ node.auth_method === 'key' ? (node.has_key ? t('nodeDetail.basic.keyConfigured') : t('nodeDetail.basic.notConfigured')) : node.has_password ? t('nodeDetail.basic.passwordConfigured') : t('nodeDetail.basic.notConfigured') }}
            </span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.hostname')">{{ node.hostname || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.cpuArch')">{{ node.arch || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.kernelVersion')">{{ node.kernel_version || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.gateway')">{{ node.gateway || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.usageStatus')">
            <StatusTag kind="usage" :status="node.busy ? 'busy' : 'idle'" />
            <span v-if="node.busy" class="mi-hint" style="margin-left: 8px" :title="usageText(node)">
              {{ t('nodes.runningConfigs', { names: usageText(node) }) }}
            </span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.lastChecked')"><TimeText :value="node.last_checked_at" /></a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.createdAt')"><TimeText :value="node.created_at" /></a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.basic.updatedAt')"><TimeText :value="node.updated_at" /></a-descriptions-item>
        </a-descriptions>
      </a-card>

      <a-card size="small" :title="t('nodeDetail.gpu.title')" class="mi-block">
        <template v-if="hasGpu">
          <div class="mi-gpu-head">
            <StatusTag kind="gpuVendor" :status="node.gpu_vendor" />
            <span class="mi-strong">{{ node.gpu_model }}</span>
            <span class="mi-hint">{{ t('nodeDetail.gpu.count', { count: node.gpu_devices.length }) }}</span>
          </div>
          <a-table
            size="small"
            row-key="index"
            :data-source="node.gpu_devices"
            :pagination="false"
            :columns="gpuColumns"
          >
            <template #bodyCell="{ column, record }">
              <template v-if="column.key === 'mem'">
                {{ record.memory_total_mib ? `${record.memory_total_mib} MiB` : '-' }}
              </template>
            </template>
          </a-table>
        </template>
        <a-empty v-else :description="t('nodeDetail.gpu.empty')" />
      </a-card>

      <a-card size="small" :title="t('nodeDetail.config.title')" class="mi-block">
        <a-descriptions size="small" :column="2" bordered>
          <a-descriptions-item :label="t('nodeDetail.config.fscache')">
            <a-tag v-if="node.fscache" color="green">{{ t('common.enabledTag') }}</a-tag>
            <span v-else class="mi-hint">{{ t('common.disabledTag') }}</span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.config.nodeExporterPort')">{{ node.node_exporter_port }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.config.nvidiaGpuExporterPort')">
            {{ node.nvidia_gpu_exporter_port }}
          </a-descriptions-item>
          <a-descriptions-item label="SOCKET_IFNAME">{{ node.socket_ifname || '-' }}</a-descriptions-item>
          <a-descriptions-item label="IB_HCA">{{ node.ib_hca || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.config.interfaces')" :span="2">
            <span v-if="node.interfaces.length">
              {{ node.interfaces.map((i) => `${i.name}: ${i.ips.join(',')}`).join(t('common.listSep')) }}
            </span>
            <span v-else>-</span>
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.config.ibDevices')" :span="2">
            <span v-if="node.ib_devices.length">{{ node.ib_devices.join(', ') }}</span>
            <span v-else>{{ t('nodeDetail.config.none') }}</span>
          </a-descriptions-item>
        </a-descriptions>
      </a-card>

      <!-- 节点监控：按组件列表 -->
      <a-card size="small" class="mi-block">
        <template #title>
          <span class="mi-card-title">{{ t('nodeDetail.monitor.title') }}</span>
        </template>
        <template #extra>
          <a-button size="small" type="primary" :loading="deploying" @click="onDeployMonitor">
            <ThunderboltOutlined />{{ t('nodeDetail.monitor.deploy') }}
          </a-button>
        </template>

        <div v-for="m in monitors" :key="m.component" class="mi-comp">
          <div class="mi-comp__head">
            <StatusTag kind="monitor" :status="m.state" />
            <span class="mi-strong">{{ componentLabel(m.component) }}</span>
            <a-button size="small" @click="progressComponent = m">
              <ProfileOutlined />{{ t('nodeDetail.monitor.progress') }}
            </a-button>
          </div>
          <a-descriptions size="small" :column="4" bordered>
            <a-descriptions-item :label="t('nodeDetail.monitor.listenPort')">{{ portOf(m) }}</a-descriptions-item>
            <a-descriptions-item :label="t('nodeDetail.monitor.image')">{{ m.image || '-' }}</a-descriptions-item>
            <a-descriptions-item :label="t('nodeDetail.monitor.serviceId')">{{ m.service_id || '-' }}</a-descriptions-item>
            <a-descriptions-item :label="t('nodeDetail.monitor.lastDeployed')"><TimeText :value="m.deployed_at" /></a-descriptions-item>
          </a-descriptions>
          <a-alert
            v-if="m.state === 'failed'"
            type="error"
            show-icon
            :message="t('nodeDetail.monitor.failed')"
            :description="m.last_error || t('nodeDetail.monitor.unknownError')"
            style="margin-top: 8px"
          />
        </div>
        <a-empty v-if="!monitors.length" :description="t('nodeDetail.monitor.empty')" />
      </a-card>

      <!-- 指标：通用指标板（系统 + GPU 分区） -->
      <a-card size="small" class="mi-block">
        <template #title>
          <span class="mi-card-title">{{ t('nodeDetail.metrics.title') }}</span>
        </template>
        <MetricsBoard
          :profile="profile"
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
          @refresh="loadMetrics(false)"
        />
      </a-card>
    </template>

    <!-- 组件部署进度弹窗 -->
    <a-modal
      :open="progressComponent !== null"
      :title="progressComponent ? `${componentLabel(progressComponent.component)} · ${t('nodeDetail.monitor.progress')}` : t('nodeDetail.monitor.progress')"
      width="720px"
      :footer="null"
      @cancel="progressComponent = null"
    >
      <template v-if="progressComponent">
        <a-descriptions size="small" :column="2" bordered style="margin-bottom: 12px">
          <a-descriptions-item :label="t('nodeDetail.monitor.component')">{{ componentLabel(progressComponent.component) }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.monitor.status')">
            <StatusTag kind="monitor" :status="progressComponent.state" />
          </a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.monitor.image')">{{ progressComponent.image || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.monitor.listenPort')">{{ portOf(progressComponent) }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.monitor.serviceId')">{{ progressComponent.service_id || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.monitor.serviceName')">{{ progressComponent.service_name || '-' }}</a-descriptions-item>
          <a-descriptions-item :label="t('nodeDetail.monitor.lastDeployed')"><TimeText :value="progressComponent.deployed_at" /></a-descriptions-item>
        </a-descriptions>
        <a-alert
          v-if="progressComponent.state === 'failed'"
          type="error"
          show-icon
          :message="t('nodeDetail.monitor.deployFailed')"
          :description="progressComponent.last_error || t('nodeDetail.monitor.unknownError')"
          style="margin-bottom: 12px"
        />
        <a-steps
          v-if="progressComponent.steps?.length"
          direction="vertical"
          size="small"
          :current="progressComponent.steps.length"
        >
          <a-step
            v-for="s in progressComponent.steps"
            :key="s.name + s.started_at"
            :status="s.state === 'succeeded' ? 'finish' : s.state === 'failed' ? 'error' : 'process'"
          >
            <template #title>{{ s.title }}</template>
            <template #description>
              <div class="mi-step-detail">{{ s.detail || '-' }}</div>
              <div class="mi-hint"><TimeText :value="s.finished_at || s.started_at" /></div>
            </template>
          </a-step>
        </a-steps>
        <a-empty v-else :description="t('nodeDetail.monitor.componentEmpty')" />
        <div class="mi-modal-footer">
          <a-button :loading="deploying" type="primary" @click="onDeployMonitor">
            <ThunderboltOutlined />{{ t('nodeDetail.monitor.deploy') }}
          </a-button>
        </div>
      </template>
    </a-modal>

    <NodeEditModal v-model:open="editOpen" :node="node" @saved="onEditSaved" />
  </PageShell>
</template>

<style scoped>
.mi-block {
  margin-bottom: 16px;
}
.mi-card-title {
  font-weight: 600;
}
.mi-gpu-head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
}
.mi-strong {
  font-weight: 600;
}
.mi-hint {
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
}
.mi-comp + .mi-comp {
  margin-top: 14px;
  padding-top: 14px;
  border-top: 1px dashed var(--mi-border);
}
.mi-comp__head {
  display: flex;
  align-items: center;
  gap: 10px;
  margin-bottom: 10px;
}
.mi-step-detail {
  font-size: 12px;
  color: var(--mi-text);
  word-break: break-all;
}
.mi-modal-footer {
  display: flex;
  justify-content: flex-end;
  margin-top: 16px;
}
</style>