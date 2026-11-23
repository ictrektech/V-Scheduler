<template>
  <div>
    <a-space style="display: flex; justify-content: space-between; align-items: center; margin-bottom: 8px" :wrap="true">
      <a-typography-title :level="4" style="margin: 0">集群概览</a-typography-title>
      <!-- 创建集群 / 加入集群入口（cluster-role-and-create-entry：能力收敛为按钮 + 弹窗） -->
      <a-space :wrap="true">
        <a-tooltip :title="agg.hasCluster ? '集群已创建，如需重建请先清理集群状态' : '创建新的多节点集群'">
          <a-button type="primary" :disabled="agg.hasCluster" @click="openCreate = true">
            创建集群
          </a-button>
        </a-tooltip>
        <!-- 加入集群 = 本节点入群（已入群后不可用；集群内扩容请到「节点管理」） -->
        <a-tooltip
          :title="agg.hasCluster
            ? '本节点已在集群中；如需扩容请到「节点管理 → 加入节点」'
            : '把本节点加入一个已存在的集群（需要该集群任一管理节点的 IP 与管理员密码）'"
        >
          <a-button type="default" :disabled="agg.hasCluster" @click="openJoin = true">
            加入集群
          </a-button>
        </a-tooltip>
      </a-space>
    </a-space>

    <!-- 空态：集群未初始化 -->
    <a-empty
      v-if="!loading && !agg.hasCluster"
      description="集群尚未创建"
      style="margin-top: 24px"
    >
      <template #description>
        <span>集群尚未创建。点「创建集群」录入节点 IP 与管理密码，一键编排 etcd + postgres-ha。<br />「管理节点」运行 etcd 与高可用 postgres；「工作节点」不部署基础设施、可运行集群化负载。<br />若本节点要加入已有集群，点「加入集群」录入该集群任一管理节点 IP 与管理员密码。</span>
      </template>
      <a-space :wrap="true">
        <a-button type="primary" @click="openCreate = true">创建集群</a-button>
        <a-button @click="openJoin = true">加入集群</a-button>
      </a-space>
    </a-empty>

    <template v-else>
      <!-- 集群状态卡 -->
      <a-card title="集群" :bordered="true" size="small" style="max-width: 720px">
        <a-descriptions :column="2" bordered size="small">
          <a-descriptions-item label="集群名称">{{ status.cluster?.cluster_name ?? '-' }}</a-descriptions-item>
          <a-descriptions-item label="集群 ID">{{ status.cluster?.cluster_id ?? '-' }}</a-descriptions-item>
          <a-descriptions-item label="状态">
            <a-tag :color="status.cluster?.status === 'created' ? 'green' : 'blue'">
              {{ agg.clusterStatusLabel }}
            </a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="创建时间">{{ status.cluster?.created_at ?? '-' }}</a-descriptions-item>
          <a-descriptions-item label="etcd 拓扑">
            <a-tag :color="etcdTopo.color">{{ etcdTopo.text }}</a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="postgres-ha">
            <a-tag :color="agg.pgOk ? 'green' : 'default'">{{ agg.pgOk ? '就绪' : '未就绪' }}</a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="haproxy 入口">
            <a-tag :color="haproxyCard.color">{{ haproxyCard.text }}</a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="集群 Temporal">
            <a-tag :color="temporalCard.color">{{ temporalCard.text }}</a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="节点数">{{ agg.nodesCount }}</a-descriptions-item>
          <a-descriptions-item label="管理节点 / 工作节点">
            <span>{{ agg.managerCount }} / {{ agg.workerCount }}</span>
          </a-descriptions-item>
          <a-descriptions-item label="管理面数据库">
            <a-tag :color="agg.dbOk ? 'green' : 'orange'">{{ agg.dbOk ? '正常' : '降级' }}</a-tag>
          </a-descriptions-item>
        </a-descriptions>
        <a-alert
          v-if="!agg.etcdOk"
          type="warning"
          show-icon
          style="margin-top: 8px"
          message="etcd 不可达"
          description="etcd 子项目未就绪（服务未启动或 raft 未成立）。编排操作将在 etcd 就绪后可用。"
        />
      </a-card>

      <!-- 拓扑卡：etcd raft 成员 -->
      <a-card title="etcd 成员" style="margin-top: 12px; max-width: 720px" size="small">
        <a-empty v-if="etcdMembers.length === 0" description="本节点未部署 etcd（管理节点运行基础设施，工作节点不运行）" :image="simpleImage" />
        <a-table
          v-else
          :data-source="etcdMembers"
          :pagination="false"
          size="small"
          row-key="compose_project_name"
        >
          <a-table-column title="项目" data-index="compose_project_name" key="name" />
          <a-table-column title="状态" key="state">
            <template #default="{ record }">
              <a-tag :color="record.state === 'deployed' ? 'green' : 'orange'">{{ record.state }}</a-tag>
            </template>
          </a-table-column>
          <a-table-column title="容器" key="containers">
            <template #default="{ record }">
              <span>{{ record.containerState ?? '-' }}</span>
            </template>
          </a-table-column>
        </a-table>
      </a-card>

      <!-- 拓扑卡：postgres-ha（patroni 一主多从） -->
      <a-card title="postgres-ha" style="margin-top: 12px; max-width: 720px" size="small">
        <a-descriptions :column="1" size="small">
          <a-descriptions-item label="主节点">
            <template v-if="pgLeader">
              <a-tag color="green">
                {{ pgLeader.name }}（{{ pgLeader.role }}）
              </a-tag>
            </template>
            <a-tag v-else color="default">未选举（patroni 未就绪）</a-tag>
          </a-descriptions-item>
<a-descriptions-item label="从节点">
            <template v-if="pgReplicas.length > 0">
              <a-tag v-for="r in pgReplicas" :key="r.name" color="blue" style="margin-right: 4px">
                {{ r.name }}（{{ r.ip ?? '未知 IP' }}）
              </a-tag>
            </template>
            <span v-else>无</span>
          </a-descriptions-item>
        </a-descriptions>
        <a-alert
          v-if="!agg.pgOk"
          type="warning"
          show-icon
          style="margin-top: 8px"
          message="postgres-ha 未就绪"
          description="patroni 主从未成立（leader 缺失或 DCS 不可用）。创建/节点操作请确认 etcd 与 pg 子项目均已部署。"
        />
      </a-card>

      <!-- 拓扑卡：haproxy 统一入口（二期 S1：每节点 15432 → postgres HA leader） -->
      <a-card title="haproxy 统一入口" style="margin-top: 12px; max-width: 720px" size="small">
        <a-descriptions :column="1" size="small">
          <a-descriptions-item label="入口状态">
            <a-tag :color="haproxyCard.color">{{ haproxyCard.text }}</a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="本地端口">
            <span v-if="haproxyCard.frontPort">{{ haproxyCard.frontPort }} → postgres HA</span>
            <span v-else>15432 → postgres HA（未部署）</span>
          </a-descriptions-item>
          <a-descriptions-item label="后端（manager）">
            <template v-if="haproxyCard.backends.length">
              <a-tag v-for="b in haproxyCard.backends" :key="b" color="blue" style="margin-right: 4px">
                {{ b }}:5433
              </a-tag>
            </template>
            <span v-else>-</span>
          </a-descriptions-item>
          <a-descriptions-item label="leader">
            <a-tag v-if="haproxyCard.leader" color="green">{{ haproxyCard.leader }}</a-tag>
            <a-tag v-else color="default">未知</a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="本机入口可达">
            <a-tag :color="haproxyCard.localReachable ? 'green' : 'orange'">
              {{ haproxyCard.localReachable ? '是' : '否' }}
            </a-tag>
          </a-descriptions-item>
        </a-descriptions>
        <a-alert
          v-if="status.haproxy && haproxyCard.deployedCount === 0"
          type="info"
          show-icon
          style="margin-top: 8px"
          message="haproxy 尚未部署"
          description="在「节点管理」页对每个节点点「部署入口」，或用 POST /api/infra/haproxy/deploy。后端拓扑由 etcd manager 记录自动推导。"
        />
        <a-alert
          v-else-if="status.haproxy && !haproxyCard.localReachable"
          type="warning"
          show-icon
          style="margin-top: 8px"
          message="本机 haproxy 入口不可达"
          description="本节点 127.0.0.1:15432 探测失败（haproxy 实例可能仍在部署或已停止）。可于「节点管理」重新部署（upsert 刷新）。"
        />
      </a-card>

      <!-- 拓扑卡：集群级 Temporal（二期 S2：首 manager 单实例，DB 经 haproxy → postgres HA） -->
      <a-card title="集群 Temporal" style="margin-top: 12px; max-width: 720px" size="small">
        <a-descriptions :column="1" size="small">
          <a-descriptions-item label="服务状态">
            <a-tag :color="temporalCard.color">{{ temporalCard.text }}</a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="接入地址">
            <span v-if="temporalCard.deployedCount">
              frontend {{ temporalCard.frontendPort }}（gRPC）· 经 haproxy
              {{ temporalCard.haproxyFrontendPort }} · UI {{ temporalCard.uiPort }}
            </span>
            <span v-else>未部署（首个管理节点单实例）</span>
          </a-descriptions-item>
          <a-descriptions-item label="本机 frontend 可达">
            <a-tag :color="temporalCard.localReachable ? 'green' : 'orange'">
              {{ temporalCard.localReachable ? '是' : '否' }}
            </a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="经 haproxy 可达">
            <a-tag :color="temporalCard.viaHaproxyReachable ? 'green' : 'default'">
              {{ temporalCard.viaHaproxyReachable ? '是' : '否' }}
            </a-tag>
          </a-descriptions-item>
          <a-descriptions-item label="DB 通道（haproxy→pg HA）">
            <a-tag :color="temporalCard.dbFrontReachable ? 'green' : 'red'">
              {{ temporalCard.dbFrontReachable ? '可达' : '不可达' }}
            </a-tag>
          </a-descriptions-item>
        </a-descriptions>
        <a-alert
          v-if="status.temporal && temporalCard.deployedCount === 0"
          type="info"
          show-icon
          style="margin-top: 8px"
          message="集群 Temporal 尚未部署"
          description="在「节点管理」页对任一管理节点点「Temporal 部署」（仅 manager 可部署，单实例）。前置：本机 haproxy 15432 与 postgres HA 就绪。"
        />
        <a-alert
          v-else-if="status.temporal && !temporalCard.localReachable"
          type="warning"
          show-icon
          style="margin-top: 8px"
          message="Temporal 实例本机不可达"
          description="本节点 127.0.0.1:7333 探测失败（自举建库/schema 需数分钟，或实例已停止）。可稍后刷新，或「节点管理」重新部署（upsert）。"
        />
      </a-card>
    </template>

    <a-alert
      v-if="!loading && errorMsg"
      type="error"
      show-icon
      style="margin-top: 12px"
      message="集群状态查询失败"
      :description="errorMsg"
    />

    <a-descriptions
      v-if="health"
      title="服务健康"
      :column="2"
      style="margin-top: 16px; max-width: 720px"
      bordered
      size="small"
    >
      <a-descriptions-item label="状态">{{ health.status }}</a-descriptions-item>
      <a-descriptions-item label="版本">{{ health.version }}</a-descriptions-item>
      <a-descriptions-item label="etcd 已配置">
        <a-tag :color="health.etcd_configured ? 'green' : 'default'">
          {{ health.etcd_configured ? '是' : '否' }}
        </a-tag>
      </a-descriptions-item>
      <a-descriptions-item label="etcd 连通">
        <a-tag :color="health.etcd_ok ? 'green' : 'orange'">{{ health.etcd_ok ? '是' : '否' }}</a-tag>
      </a-descriptions-item>
      <a-descriptions-item label="管理面数据库">
        <a-tag :color="health.db_ok ? 'green' : 'orange'">{{ health.db_ok ? '可用' : '降级' }}</a-tag>
      </a-descriptions-item>
      <a-descriptions-item label="时间">{{ health.timestamp }}</a-descriptions-item>
    </a-descriptions>

    <!-- 创建集群弹窗（多步向导；入口收敛 cluster-role-and-create-entry） -->
    <CreateClusterModal v-model:open="openCreate" />
    <!-- 本节点入群弹窗（编排由目标集群管理节点执行；集群内扩容在「节点管理」页） -->
    <JoinClusterModal v-model:open="openJoin" @success="onJoinSuccess" @error="onJoinError" />
  </div>
</template>

<script setup lang="ts">
import { onBeforeUnmount, onMounted, ref, computed } from 'vue';
import { message } from 'ant-design-vue';
import { fetchHealth, type HealthData } from '../api/health';
import { fetchClusterStatus, type ClusterStatus } from '../api/cluster';
import { fetchEtcdStatus, fetchPgStatus, type InfraInstanceDetail, type PatroniLeaderInfo } from '../api/infra';
import { aggregateClusterStatus, etcdTopologyLabel, haproxyMeta, temporalMeta } from '../utils/overview';
import CreateClusterModal from '../components/CreateClusterModal.vue';
import JoinClusterModal from '../components/JoinClusterModal.vue';

const openCreate = ref(false);
const openJoin = ref(false);
const health = ref<HealthData | null>(null);
const status = ref<ClusterStatus>({ etcd_ok: false, cluster: null, infra: null, nodes_count: 0, db_ok: false });
const etcdInstances = ref<InfraInstanceDetail[]>([]);
const pgInstances = ref<InfraInstanceDetail[]>([]);
const loading = ref(true);
const errorMsg = ref('');

/** 聚合状态（纯函数） */
const agg = computed(() => aggregateClusterStatus(status.value));
const etcdTopo = computed(() => etcdTopologyLabel(agg.value.etcdOk, etcdMemberCount.value));
/** haproxy 统一入口卡（二期 S1：数据来自 cluster/status 内嵌 haproxy 视图） */
const haproxyCard = computed(() => haproxyMeta(status.value.haproxy));
/** 集群 Temporal 卡（二期 S2：数据来自 cluster/status 内嵌 temporal 视图，本机视角） */
const temporalCard = computed(() => temporalMeta(status.value.temporal));
const simpleImage = undefined;

/** etcd 成员表（本机部署的 etcd 实例 → 展示行） */
const etcdMembers = computed(() =>
  etcdInstances.value
    .filter((d) => d.instance?.project_name === 'etcd')
    .map((d) => ({
      compose_project_name: d.instance?.compose_project_name ?? '-',
      state: d.instance?.state ?? '-',
      containerState: d.runtime?.aggregate_state ?? (d.runtime?.containers?.[0]?.state ?? '-'),
    })),
);
const etcdMemberCount = ref<number | undefined>(undefined);

/** pg/status 原始结果（preserve patroni_leader 引用） */
const pgStatusRaw = ref<{ patroni_leader?: PatroniLeaderInfo } | null>(null);

/** pg 主从拆分（patroni leader + replication 列表推导从节点 IP） */
const pgLeader = computed(() => {
  const l = pgStatusRaw.value?.patroni_leader;
  if (!l) return null;
  return { name: l.name, role: l.role, ip: l.ip };
});
const pgReplicas = computed(() => {
  const l = pgStatusRaw.value?.patroni_leader;
  if (!l || !l.replication) return [];
  return l.replication.map((r) => ({ name: r.application_name, ip: r.client_addr ?? undefined }));
});

/** 5s 健康轮询（页面可见时；不可见暂停） */
let timer: ReturnType<typeof setInterval> | null = null;

async function refreshHealth(): Promise<void> {
  try {
    health.value = await fetchHealth();
  } catch (e) {
    console.warn('health 查询失败', e);
  }
}

async function refreshAll(): Promise<void> {
  try {
    const st = await fetchClusterStatus();
    status.value = st;
  } catch (e) {
    if (!loading.value) errorMsg.value = String(e);
  }
  try {
    const etcdStatus = await fetchEtcdStatus();
    etcdInstances.value = etcdStatus.instances ?? [];
    etcdMemberCount.value = etcdStatus.member_count;
  } catch (e) {
    console.warn('etcd status 查询失败', e);
  }
  try {
    const pg = await fetchPgStatus();
    pgStatusRaw.value = pg.patroni_leader ? { patroni_leader: pg.patroni_leader } : null;
    pgInstances.value = pg.instances ?? [];
  } catch (e) {
    console.warn('pg status 查询失败', e);
  }
  loading.value = false;
}

/** 本节点入群成功：就地刷新概览状态（不跳转页面） */
async function onJoinSuccess(): Promise<void> {
  message.success('已加入集群');
  await refreshAll();
}

/** 加入集群失败：toast 提示（不覆盖集群状态查询错误警报） */
function onJoinError(msg: string): void {
  message.error(msg);
}

function onVisibility(): void {
  if (document.visibilityState === 'visible') {
    if (!timer) {
      void refreshAll();
      timer = setInterval(refreshAll, 5000);
    }
  } else if (timer) {
    clearInterval(timer);
    timer = null;
  }
}

onMounted(() => {
  document.addEventListener('visibilitychange', onVisibility);
  void refreshHealth();
  void refreshAll();
  timer = setInterval(refreshAll, 5000);
});

onBeforeUnmount(() => {
  document.removeEventListener('visibilitychange', onVisibility);
  if (timer) {
    clearInterval(timer);
    timer = null;
  }
});
</script>

<style scoped></style>