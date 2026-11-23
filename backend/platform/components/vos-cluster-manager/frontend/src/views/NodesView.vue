<template>
  <div>
    <a-typography-title :level="4">节点管理</a-typography-title>

    <a-space style="margin-bottom: 12px">
      <a-tooltip title="向本集群追加节点">
        <a-button type="primary" @click="openAdd">加入节点（扩容）</a-button>
      </a-tooltip>
      <a-button @click="refresh">刷新</a-button>
    </a-space>

    <a-alert
      v-if="errorMsg"
      type="error"
      show-icon
      style="margin-bottom: 12px"
      message="操作失败"
      :description="errorMsg"
    />

    <a-table :data-source="nodes" :loading="loading" :pagination="false" size="small" row-key="member_id">
      <a-table-column title="节点 IP" data-index="node_ip" key="ip" />
      <a-table-column title="名称" data-index="display_name" key="name" />
      <a-table-column title="角色" key="role">
        <template #default="{ record }">
          <a-tag :color="roleColor(record.role)">{{ roleLabel(record.role) }}</a-tag>
        </template>
      </a-table-column>
      <a-table-column title="加入时间" data-index="joined_at" key="joined" />
      <a-table-column title="API 端点" data-index="api_endpoint" key="api" />
      <a-table-column title="统一入口(haproxy)" key="lb" width="220">
        <template #default="{ record }">
          <a-space>
            <a-tag v-if="lbMap[record.node_ip]?.present" color="green">已部署</a-tag>
            <a-tag v-else>未部署</a-tag>
            <a-popconfirm
              v-if="lbMap[record.node_ip]?.present"
              title="移除该节点的 haproxy 统一入口？"
              description="移除后本机 15432 → postgres HA 的转发入口将不可用。"
              ok-text="移除" cancel-text="取消"
              :disabled="lbBusy === record.member_id"
              @confirm="doLbRemove(record)"
            >
              <a-button size="small" danger :loading="lbBusy === record.member_id">移除入口</a-button>
            </a-popconfirm>
            <a-popconfirm
              v-else
              title="在该节点部署 haproxy 统一入口？"
              description="后端拓扑由 etcd manager 记录自动推导；本机将监听 15432 转发至 postgres HA leader。"
              ok-text="部署" cancel-text="取消"
              :disabled="lbBusy === record.member_id"
              @confirm="doLbDeploy(record)"
            >
              <a-button type="primary" size="small" :loading="lbBusy === record.member_id">
                部署入口
              </a-button>
            </a-popconfirm>
          </a-space>
        </template>
      </a-table-column>
      <a-table-column title="集群 Temporal" key="temporal" width="220">
        <template #default="{ record }">
          <a-space v-if="temporalMap[record.node_ip]?.present">
            <a-tag color="green">已部署</a-tag>
            <a-popconfirm
              title="移除该节点的集群 temporal 实例？"
              description="将移除本机 temporal 实例并触发全集群 haproxy 刷新（收缩 7133 后端）。"
              ok-text="移除" cancel-text="取消"
              :disabled="temporalBusy === record.member_id"
              @confirm="doTemporalRemove(record)"
            >
              <a-button size="small" danger :loading="temporalBusy === record.member_id">移除</a-button>
            </a-popconfirm>
          </a-space>
          <a-space v-else-if="temporalMap[record.node_ip]?.deployable === false">
            <a-tag>工作节点不可部署</a-tag>
          </a-space>
          <a-space v-else>
            <a-tag>未部署</a-tag>
            <a-popconfirm
              title="在该管理节点部署集群 temporal？"
              description="单实例语义（首个管理节点为期望位）；前置本机 haproxy 15432 + postgres HA 就绪。DB 与凭据均经本机闭环，不出节点。"
              ok-text="部署" cancel-text="取消"
              :disabled="temporalBusy === record.member_id"
              @confirm="doTemporalDeploy(record)"
            >
              <a-button type="primary" size="small" :loading="temporalBusy === record.member_id">
                Temporal 部署
              </a-button>
            </a-popconfirm>
          </a-space>
        </template>
      </a-table-column>
      <a-table-column title="操作" key="action" width="140">
        <template #default="{ record }">
          <a-popconfirm
            title="确认移除该节点？"
            :description="isManagerRole(record.role) ? '将移除该节点的 etcd / postgres-ha 子项目、etcd raft 成员与元数据。' : '该节点为工作节点（无 etcd / postgres-ha 子项目），将移除其元数据。'"
            ok-text="移除" cancel-text="取消"
            :disabled="removingId === record.member_id"
            @confirm="doRemove(record.member_id)"
          >
            <a-button danger size="small" :loading="removingId === record.member_id">
              移除
            </a-button>
          </a-popconfirm>
        </template>
      </a-table-column>
    </a-table>

    <a-empty v-if="!loading && nodes.length === 0" description="暂无成员节点" style="margin-top: 24px">
      <template #description>
        <span>尚未录入任何节点。点「加入节点（扩容）」录入节点 IP 与管理密码；或到「集群概览」创建完整集群（管理节点 + 工作节点）。</span>
      </template>
      <a-button type="primary" @click="goOverview">去概览创建</a-button>
    </a-empty>

    <!-- 加入节点 Modal（集群内扩容；本节点入群请到「集群概览」→「加入集群」） -->
    <JoinNodeModal v-model:open="addOpen" @success="refresh" @error="errorMsg = $event" />
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref } from 'vue';
import { useRouter } from 'vue-router';
import { fetchNodes, removeNode, type NodeRecordView } from '../api/nodes';
import { deployHaproxy, removeHaproxy, fetchHaproxyNodes, deployTemporal, removeTemporal, fetchTemporalNodes } from '../api/infra';
import { roleLabel, roleColor, isManagerRole } from '../utils/roles';
import JoinNodeModal from '../components/JoinNodeModal.vue';

const router = useRouter();
const nodes = ref<NodeRecordView[]>([]);
const loading = ref(true);
const errorMsg = ref('');
const addOpen = ref(false);
const removingId = ref('');

/** haproxy 入口聚合态：node_ip → compose_project_name（present）映射（二期 S1） */
const lbMap = ref<Record<string, { present: boolean; compose_project_name: string | null }>>({});
const lbBusy = ref('');

/** 集群 temporal 聚合态（二期 S2：present + deployable 角色门控，deployable=false 仅 worker） */
const temporalMap = ref<
  Record<string, { present: boolean; deployable: boolean; compose_project_name: string | null }>
>({});
const temporalBusy = ref('');

async function refresh(): Promise<void> {
  loading.value = true;
  errorMsg.value = '';
  try {
    nodes.value = (await fetchNodes()).nodes ?? [];
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    loading.value = false;
  }
  await refreshLb();
  await refreshTemporal();
}

/** 刷新各节点 haproxy 入口部署态（跨节点聚合，失败静默保留旧值） */
async function refreshLb(): Promise<void> {
  try {
    const res = await fetchHaproxyNodes();
    const m: Record<string, { present: boolean; compose_project_name: string | null }> = {};
    for (const n of res.nodes) m[n.node_ip] = { present: n.haproxy_present, compose_project_name: n.compose_project_name };
    lbMap.value = m;
  } catch (e) {
    console.warn('haproxy nodes 查询失败', e);
  }
}

/** 刷新各节点 temporal 部署态（manager 限定，失败静默保留旧值） */
async function refreshTemporal(): Promise<void> {
  try {
    const res = await fetchTemporalNodes();
    const m: Record<
      string,
      { present: boolean; deployable: boolean; compose_project_name: string | null }
    > = {};
    for (const n of res.nodes)
      m[n.node_ip] = {
        present: n.temporal_present,
        deployable: n.deployable,
        compose_project_name: n.compose_project_name,
      };
    temporalMap.value = m;
  } catch (e) {
    console.warn('temporal nodes 查询失败', e);
  }
}

async function doTemporalDeploy(record: NodeRecordView): Promise<void> {
  errorMsg.value = '';
  temporalBusy.value = record.member_id;
  try {
    // at=node_ip → 目标 manager 用本地 etcd 凭据自举部署（密码不出本机）
    const res = await deployTemporal({ node_ip: record.node_ip, at: record.node_ip });
    const bad = (res.haproxy_refresh ?? []).filter((r) => !r.ok);
    if (bad.length) {
      errorMsg.value = `temporal 已部署，但 ${bad.map((b) => b.node_ip).join(',')} 的 haproxy 刷新失败（可用「刷新入口」或稍后自动恢复）`;
    }
    await refreshTemporal();
    await refreshLb();
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    temporalBusy.value = '';
  }
}

async function doTemporalRemove(record: NodeRecordView): Promise<void> {
  errorMsg.value = '';
  temporalBusy.value = record.member_id;
  const name =
    temporalMap.value[record.node_ip]?.compose_project_name ||
    `cluster-temporal-${record.node_ip.replace(/\./g, '-')}`;
  try {
    const res = await removeTemporal(name, record.node_ip);
    const bad = (res.haproxy_refresh ?? []).filter((r) => !r.ok);
    if (bad.length) {
      errorMsg.value = `temporal 已移除，但 ${bad.map((b) => b.node_ip).join(',')} 的 haproxy 收缩刷新失败`;
    }
    await refreshTemporal();
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    temporalBusy.value = '';
  }
}

async function doLbDeploy(record: NodeRecordView): Promise<void> {
  errorMsg.value = '';
  lbBusy.value = record.member_id;
  try {
    // at=node_ip → 编排方转发到目标节点部署（token 不出本机）
    await deployHaproxy({ node_ip: record.node_ip, at: record.node_ip });
    await refreshLb();
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    lbBusy.value = '';
  }
}

async function doLbRemove(record: NodeRecordView): Promise<void> {
  errorMsg.value = '';
  lbBusy.value = record.member_id;
  const name = lbMap.value[record.node_ip]?.compose_project_name;
  if (!name) {
    errorMsg.value = '未找到该节点的 haproxy 实例名';
    lbBusy.value = '';
    return;
  }
  try {
    await removeHaproxy(name, record.node_ip);
    await refreshLb();
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    lbBusy.value = '';
  }
}

function openAdd(): void {
  addOpen.value = true;
  errorMsg.value = '';
}

async function doRemove(memberId: string): Promise<void> {
  errorMsg.value = '';
  removingId.value = memberId;
  try {
    await removeNode(memberId, true);
    await refresh();
  } catch (e) {
    errorMsg.value = String(e);
  } finally {
    removingId.value = '';
  }
}

function goOverview(): void {
  void router.push('/overview');
}

onMounted(refresh);
</script>

<style scoped></style>