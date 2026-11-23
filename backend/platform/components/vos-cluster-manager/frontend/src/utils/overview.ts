import type { ClusterStatus, ClusterHaproxyView, ClusterTemporalView } from '../api/cluster';

/**
 * 集群概览聚合纯函数（可单测，不依赖 DOM）。
 * 输入后端冻结契约（/api/cluster/status / /api/infra/{etcd,pg}/status），
 * 输出页面展示所需的派生状态。
 */

/** 集群状态展示标签（后端 status 枚举：「created」/「initialized」/其他） */
export function clusterStatusLabel(s: string): string {
  if (s === 'created') return '已创建';
  if (s === 'initialized') return '已初始化';
  return s;
}

/** 概览聚合结果（就绪位 + 集群存在 + 节点数 + 角色计数） */
export interface OverviewAggregated {
  /** 集群是否已初始化/创建（cluster 记录存在） */
  hasCluster: boolean;
  clusterStatusLabel: string;
  etcdOk: boolean;
  pgOk: boolean;
  dbOk: boolean;
  nodesCount: number;
  /** 管理节点数（role=manager，运行 etcd + ha postgres） */
  managerCount: number;
  /** 工作节点数（role=worker，不部署基础设施） */
  workerCount: number;
}

/** 聚合 cluster/status 为页面状态卡数据（就绪位缺失按 false 计） */
export function aggregateClusterStatus(status: ClusterStatus): OverviewAggregated {
  return {
    hasCluster: status.cluster !== null,
    clusterStatusLabel: status.cluster ? clusterStatusLabel(status.cluster.status) : '',
    etcdOk: status.etcd_ok,
    pgOk: status.infra?.postgres_ha_ok ?? false,
    dbOk: status.db_ok,
    nodesCount: status.nodes_count ?? 0,
    managerCount: status.manager_count ?? 0,
    workerCount: status.worker_count ?? 0,
  };
}

/** patroni 角色展示元数据（primary/master/replica/其他） */
export function pgRoleMeta(role: string): { text: string; color: string } {
  const r = (role || '').toLowerCase();
  if (r === 'primary' || r === 'master') return { text: '主', color: 'green' };
  if (r === 'replica' || r === 'standby') return { text: '从', color: 'blue' };
  return { text: role || '未知', color: 'default' };
}

/** etcd 集群就绪判定（member_count 缺失视为不可信，由 etcd_ok 兜底） */
export function etcdTopologyLabel(etcdOk: boolean, memberCount?: number): { text: string; color: string } {
  if (!etcdOk) return { text: 'etcd 不可达', color: 'red' };
  if (memberCount === undefined) return { text: '状态未知', color: 'default' };
  return { text: `${memberCount} 节点`, color: 'green' };
}

/**
 * haproxy 入口状态展示元数据（二期 S1 cluster-haproxy-lb）。
 * 输入 cluster/status 内嵌 haproxy 视图（可空），输出状态卡的标签 / 颜色 / 明细文案。
 *
 * 就绪判定：
 * - 视图缺失（etcd 不可达）→ {text: '未就绪', color: 'default'}
 * - 已部署实例且本机入口可达 → {text: '就绪（N 实例）', color: 'green'}
 * - 有实例但本机入口不可达 → {text: '本机入口不可达', color: 'orange'}
 * - 无实例 → {text: '未部署', color: 'default'}
 */
export function haproxyMeta(
  view: ClusterHaproxyView | null | undefined,
): { text: string; color: string; deployedCount: number; backends: string[]; frontPort: number; localReachable: boolean; leader: string | null } {
  if (!view) {
    return {
      text: '未就绪',
      color: 'default',
      deployedCount: 0,
      backends: [],
      frontPort: 0,
      localReachable: false,
      leader: null,
    };
  }
  const base = {
    deployedCount: view.deployed_count,
    backends: view.pg_backends ?? [],
    frontPort: view.front_port,
    localReachable: view.local_pg_front_reachable,
    leader: view.patroni_leader ?? null,
  };
  if (view.haproxy_ok) return { text: `就绪（${view.deployed_count} 实例）`, color: 'green', ...base };
  if (view.deployed_count > 0 && !view.local_pg_front_reachable)
    return { text: '本机入口不可达', color: 'orange', ...base };
  if (view.deployed_count === 0) return { text: '未部署', color: 'default', ...base };
  return { text: '部署中', color: 'orange', ...base };
}
/**
 * 集群 temporal 状态展示元数据（二期 S2 cluster-temporal-service）。
 * 输入 cluster/status 内嵌 temporal 视图（可空），输出状态卡标签 / 颜色 / 明细。
 *
 * 就绪判定（单实例语义 D-TE-02——deployed_count 期望 ≤1）：
 * - 视图缺失（etcd 不可达）→ {text: '未就绪', color: 'default'}
 * - 已部署且本机 frontend 7333 可达 → {text: '就绪', color: 'green'}
 * - 有实例但本机 frontend 不可达 → {text: '实例异常', color: 'orange'}
 * - 无实例 → {text: '未部署', color: 'default'}
 */
export function temporalMeta(
  view: ClusterTemporalView | null | undefined,
): {
  text: string;
  color: string;
  deployedCount: number;
  frontendPort: number;
  haproxyFrontendPort: number;
  uiPort: number;
  localReachable: boolean;
  viaHaproxyReachable: boolean;
  dbFrontReachable: boolean;
} {
  if (!view) {
    return {
      text: '未就绪',
      color: 'default',
      deployedCount: 0,
      frontendPort: 0,
      haproxyFrontendPort: 0,
      uiPort: 0,
      localReachable: false,
      viaHaproxyReachable: false,
      dbFrontReachable: false,
    };
  }
  const base = {
    deployedCount: view.deployed_count,
    frontendPort: view.frontend_port,
    haproxyFrontendPort: view.haproxy_frontend_port,
    uiPort: view.ui_port,
    localReachable: view.local_frontend_reachable,
    viaHaproxyReachable: view.front_via_haproxy_reachable,
    dbFrontReachable: view.db_front_reachable,
  };
  if (view.temporal_ok) return { text: '就绪', color: 'green', ...base };
  if (view.deployed_count > 0) return { text: '实例异常', color: 'orange', ...base };
  return { text: '未部署', color: 'default', ...base };
}
