<template>
  <a-modal
    :open="open"
    :width="880"
    :footer="null"
    :closable="true"
    :mask-closable="false"
    :keyboard="false"
    centered
    title="创建集群"
    @cancel="handleCancel"
  >
    <a-steps :current="step" size="small" style="margin-bottom: 20px">
      <a-step title="集群信息" />
      <a-step title="节点录入" />
      <a-step title="确认" />
      <a-step title="提交" />
      <a-step title="进度" />
    </a-steps>

    <!-- Step 1: 集群信息 -->
    <a-card v-if="step === 0" title="集群信息" :bordered="true" size="small">
      <a-form layout="vertical">
        <a-form-item label="集群名称" required>
          <a-input v-model:value="wizard.cluster_name" placeholder="如 demo-cluster" @press-enter="next" />
        </a-form-item>
      </a-form>
      <a-space>
        <a-button type="primary" :disabled="!!clusterNameError" @click="next">下一步</a-button>
      </a-space>
      <div v-if="clusterNameError" style="color: #cf1322; font-size: 12px; margin-top: 8px">{{ clusterNameError }}</div>
    </a-card>

    <!-- Step 2: 节点录入 -->
    <a-card v-if="step === 1" title="节点录入" :bordered="true" size="small">
      <a-alert
        type="info"
        show-icon
        style="margin-bottom: 12px"
        message="录入集群各节点 IP 与管理员密码"
        description="编排器将仅对「管理节点」部署 etcd / postgres-ha 子项目并建立 raft；「工作节点」不部署基础设施、可运行集群化负载。密码仅用于编排期与节点授权，不会在界面/日志回显。"
      />
      <div v-for="(row, i) in wizard.nodes" :key="i" class="row">
        <a-space align="baseline" :wrap="true">
          <a-input v-model:value="row.node_ip" placeholder="节点 IP" style="width: 150px" />
          <a-input-password v-model:value="row.admin_password" placeholder="管理员密码（≥6）" style="width: 180px" />
          <a-input v-model:value="row.display_name" placeholder="名称（可选）" style="width: 130px" />
          <a-radio-group v-model:value="row.role" :disabled="i === 0">
            <a-radio value="manager">管理节点</a-radio>
            <a-radio value="worker">工作节点</a-radio>
          </a-radio-group>
          <a-button v-if="wizard.nodes.length > 1" danger size="small" @click="removeRow(i)">删除</a-button>
          <a-tooltip v-if="i === 0" title="发起节点须为管理节点——将部署 etcd 与高可用 postgres（第一个管理节点同时承担 raft 初始成员与 postgres 主库）">
            <span style="color: #1677ff; font-size: 12px">发起节点</span>
          </a-tooltip>
        </a-space>
        <div v-if="rowErrors[i]" style="color: #cf1322; font-size: 12px; margin-top: 4px">{{ rowErrors[i] }}</div>
      </div>
      <a-button style="margin-top: 12px" @click="addRow">+ 添加节点</a-button>
      <a-space style="margin-top: 12px; display: flex">
        <a-button @click="step = 0">上一步</a-button>
        <a-button type="primary" @click="next">下一步</a-button>
      </a-space>
      <div v-if="firstRowError" style="color: #cf1322; font-size: 12px; margin-top: 8px">{{ firstRowError }}</div>
    </a-card>

    <!-- Step 3: 确认 -->
    <a-card v-if="step === 2" title="确认" :bordered="true" size="small">
      <a-descriptions :column="1" bordered size="small">
        <a-descriptions-item label="集群名称">{{ wizard.cluster_name }}</a-descriptions-item>
        <a-descriptions-item label="节点数">{{ wizard.nodes.length }}</a-descriptions-item>
        <a-descriptions-item label="管理节点">{{ managerRows.length }}（运行 etcd + 高可用 postgres）</a-descriptions-item>
      </a-descriptions>
      <a-table
        :data-source="wizard.nodes.map((r, i) => ({ key: i, ...r }))"
        :pagination="false"
        size="small"
        style="margin-top: 12px"
      >
        <a-table-column title="节点 IP" data-index="node_ip" key="ip" />
        <a-table-column title="名称" data-index="display_name" key="name" />
        <a-table-column title="角色" key="role">
          <template #default="{ record }">
            <a-tag :color="roleColor(record.role)">{{ roleLabel(record.role) }}<template v-if="record.key === 0">（发起）</template></a-tag>
          </template>
        </a-table-column>
      </a-table>
      <a-space style="margin-top: 12px; display: flex">
        <a-button @click="step = 1">上一步</a-button>
        <a-button type="primary" @click="step = 3">下一步</a-button>
      </a-space>
    </a-card>

    <!-- Step 4: 提交 -->
    <a-card v-if="step === 3" title="提交" :bordered="true" size="small">
      <a-space>
        <a-button type="primary" :loading="submitting" @click="submit">提交并创建</a-button>
        <a-button @click="step = 2">返回确认</a-button>
      </a-space>
      <div v-if="submitError" style="color: #cf1322; font-size: 12px; margin-top: 8px">{{ submitError }}</div>
    </a-card>

    <!-- Step 5: 进度 -->
    <a-card v-if="step === 4" title="创建进度" :bordered="true" size="small">
      <a-spin v-if="progressStatus === 'Running'" :spinning="true">
        <div style="min-height: 60px">
          <div>编排进行中（部署 etcd raft → postgres-ha → 注册集群信息），请稍候……</div>
          <div style="margin-top: 8px; color: rgba(0,0,0,0.45)">task_id: {{ taskId }}</div>
        </div>
      </a-spin>
      <template v-else-if="progressStatus === 'Completed'">
        <a-result status="success" title="集群创建完成">
          <template #extra>
            <a-button type="primary" @click="goOverview">进入概览</a-button>
          </template>
        </a-result>
      </template>
      <template v-else-if="progressStatus === 'Failed'">
        <a-alert type="error" show-icon message="集群创建失败" :description="progressMsg" />
        <a-space style="margin-top: 12px">
          <a-button type="primary" @click="restart">重新提交</a-button>
          <a-button @click="goOverview">进入概览</a-button>
        </a-space>
      </template>
    </a-card>
  </a-modal>
</template>

<script setup lang="ts">
import { computed, onBeforeUnmount, reactive, ref, watch } from 'vue';
import { useRouter } from 'vue-router';
import { createCluster, createClusterStatus } from '../api/cluster';
import {
  buildCreateInput,
  createWizardState,
  firstRowRoleError,
  validateClusterInfo,
  validateNodeRows,
  type WizardState,
} from '../utils/createWizard';
import { roleColor, roleLabel, type NodeRole } from '../utils/roles.ts';

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{ 'update:open': [value: boolean] }>();

const router = useRouter();
const step = ref(0);
const wizard: WizardState = reactive(createWizardState());
const rowErrors = ref<Record<number, string>>({});
const submitting = ref(false);
const submitError = ref('');
const taskId = ref('');
const progressStatus = ref<'Running' | 'Completed' | 'Failed'>('Running');
const progressMsg = ref('');

const clusterNameError = computed(() => validateClusterInfo(wizard.cluster_name));
const firstRowError = computed(() => firstRowRoleError(wizard));
const managerRows = computed(() => wizard.nodes.filter((r) => r.role === 'manager'));

/**
 * 打开时重置为全新向导状态（保证「重新打开弹窗 = 全新向导」）；
 * 关闭时销毁轮询定时器。
 */
watch(
  () => props.open,
  (open) => {
    if (open) {
      resetWizard();
    } else {
      stopPolling();
    }
  },
);

function resetWizard(): void {
  Object.assign(wizard, createWizardState());
  rowErrors.value = {};
  step.value = 0;
  submitting.value = false;
  submitError.value = '';
  taskId.value = '';
  progressStatus.value = 'Running';
  progressMsg.value = '';
}

function close(): void {
  stopPolling();
  emit('update:open', false);
}

function handleCancel(): void {
  close();
}

/** 步骤 1 → 2 前置跳转（集群名称合法 + 行校验通过才允许前进） */
function addRow(): void {
  wizard.nodes.push({ node_ip: '', admin_password: '', display_name: '', role: 'worker' });
}
function removeRow(i: number): void {
  if (wizard.nodes.length > 1) wizard.nodes.splice(i, 1);
}
function next(): void {
  if (step.value === 0) {
    if (clusterNameError.value) return;
    step.value = 1;
    return;
  }
  if (step.value === 1) {
    const { errors, ok } = validateNodeRows(wizard.nodes);
    rowErrors.value = errors;
    if (!ok) return;
    if (firstRowError.value) return;
    step.value = 2;
  }
}

/** 提交 create（20506 并发防重：提示引导） */
async function submit(): Promise<void> {
  submitError.value = '';
  submitting.value = true;
  try {
    const res = await createCluster(buildCreateInput(wizard));
    taskId.value = res.task_id;
    step.value = 4;
    startPolling();
  } catch (e) {
    const msg = String(e);
    if (msg.includes('20506') || msg.includes('创建中') || msg.includes('集群已创建')) {
      submitError.value = `集群创建中或已存在（${msg}）。可关闭弹窗查看概览当前状态。`;
    } else {
      submitError.value = msg;
    }
  } finally {
    submitting.value = false;
  }
}

/** 进度轮询（5s；关闭弹窗/卸载销毁） */
let pollTimer: ReturnType<typeof setInterval> | null = null;
async function pollOnce(): Promise<void> {
  try {
    const st = await createClusterStatus(taskId.value);
    progressStatus.value = st.status as 'Running' | 'Completed' | 'Failed';
    if (st.status === 'Failed') progressMsg.value = '编排失败，请查看后端日志（temporal workflow 详情）。';
    if (st.status !== 'Running' && pollTimer) {
      clearInterval(pollTimer);
      pollTimer = null;
    }
  } catch (e) {
    // 查询失败不中断轮询（后端短暂不可达时容错）
    console.warn('create/status 查询失败', e);
  }
}
function startPolling(): void {
  if (pollTimer) clearInterval(pollTimer);
  progressStatus.value = 'Running';
  void pollOnce();
  pollTimer = setInterval(pollOnce, 5000);
}
function stopPolling(): void {
  if (pollTimer) {
    clearInterval(pollTimer);
    pollTimer = null;
  }
}

/** 失败重试：回 Step 4 保留数据重新提交 */
function restart(): void {
  step.value = 3;
  progressStatus.value = 'Running';
  progressMsg.value = '';
}

/** 成功/失败进入概览：关闭弹窗 + 路由跳转 */
function goOverview(): void {
  close();
  void router.push('/overview');
}

onBeforeUnmount(stopPolling);
</script>

<style scoped>
.row {
  margin-bottom: 8px;
}
</style>