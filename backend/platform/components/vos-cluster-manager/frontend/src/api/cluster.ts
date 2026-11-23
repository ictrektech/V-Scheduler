import { get, post } from './client';

/** 集群信息（/vos/cluster/info 对应） */
export interface ClusterInfo {
  cluster_id: string;
  cluster_name: string;
  status: string;
  created_at: string;
}

/** 基础设施就绪位（/vos/cluster/infra_status 对应；二期扩展字段向后兼容） */
export interface InfraStatus {
  etcd_ok: boolean;
  postgres_ha_ok: boolean;
  haproxy_ok?: boolean;
  temporal_ok?: boolean;
  updated_at: string;
}

/**
 * haproxy 计算视图（二期 S1；cluster/status 内嵌，etcd 不可达为 null）。
 * 与后端 haproxy_status_data(probe_leader=false) 对齐（不含 instances 明细）。
 */
export interface ClusterHaproxyView {
  haproxy_ok: boolean;
  front_port: number;
  stats_port: number;
  /** postgres HA 后端（etcd manager 推导） */
  pg_backends: string[];
  /** 已部署 haproxy 实例数（集群内视角按 manager 部署数为期望） */
  deployed_count: number;
  /** 本机入口 127.0.0.1:<front_port> TCP 可达 */
  local_pg_front_reachable: boolean;
  patroni_leader: string | null;
}

/**
 * temporal 计算视图（二期 S2；cluster/status 内嵌，etcd 不可达为 null）。
 * 与后端 temporal_status_data 对齐（不含 instances 明细）。
 */
export interface ClusterTemporalView {
  temporal_ok: boolean;
  frontend_port: number;
  haproxy_frontend_port: number;
  ui_port: number;
  deployed_count: number;
  local_frontend_reachable: boolean;
  front_via_haproxy_reachable: boolean;
  db_front_reachable: boolean;
}

/** GET /api/cluster/status 聚合结果 */
export interface ClusterStatus {
  etcd_ok: boolean;
  cluster: ClusterInfo | null;
  infra: InfraStatus | null;
  nodes_count: number;
  /** 管理节点数（运行 etcd + ha postgres，role=manager） */
  manager_count?: number;
  /** 工作节点数（不运行基础设施，role=worker） */
  worker_count?: number;
  /** haproxy 统一入口视图（二期 S1；null=etcd 不可达） */
  haproxy?: ClusterHaproxyView | null;
  /** 集群 temporal 视图（二期 S2；null=etcd 不可达） */
  temporal?: ClusterTemporalView | null;
  db_ok: boolean;
}

/** 拉取集群状态聚合（etcd 连通性 + 集群信息 + 节点数 + 管理面 DB 可用性） */
export function fetchClusterStatus(): Promise<ClusterStatus> {
  return get<ClusterStatus>('/api/cluster/status');
}

/** 创建向导节点录入行 */
export interface CreateNodeInput {
  /** 节点 IP（IPv4，运维面可达） */
  node_ip: string;
  /** 节点管理员密码（长度 ≥6；仅编排期使用，不回显） */
  admin_password: string;
  /** 节点角色：manager（管理节点，运行 etcd + ha postgres）| worker（工作节点，不部署基础设施） */
  role?: string;
  /** 展示名称（缺省回退 node_ip） */
  display_name?: string;
  /** api_endpoint（可选，缺省由后端推导表单外网地址） */
  api_endpoint?: string;
}

/** POST /api/cluster/create 请求体 */
export interface CreateClusterInput {
  cluster_name: string;
  nodes: CreateNodeInput[];
}

/** POST /api/cluster/create 响应 */
export interface CreateClusterResult {
  cluster_id: string;
  task_id: string;
}

/** POST /api/cluster/create/status 响应（status: Running | Completed | Failed | Cancelled | ...） */
export interface ClusterCreateStatusResult {
  task_id: string;
  status: string;
}

/** 提交集群创建编排（返回 task_id，进度经 createStatus 轮询） */
export function createCluster(input: CreateClusterInput): Promise<CreateClusterResult> {
  return post<CreateClusterResult>('/api/cluster/create', input);
}

/** 查询集群创建编排进度（task_id 来自 createCluster 返回值） */
export function createClusterStatus(taskId: string): Promise<ClusterCreateStatusResult> {
  return post<ClusterCreateStatusResult>('/api/cluster/create/status', { task_id: taskId });
}
/** POST /api/cluster/join 请求体（本节点入群：编排由目标集群 seed 管理节点执行） */
export interface JoinClusterInput {
  /** 目标集群任一管理节点 IP（只接受 IP，不接受 URL） */
  seed_ip: string;
  /** 该集群管理员密码（长度 ≥6；仅转发给 seed，不回显） */
  admin_password: string;
  /** 本节点名称（可选，缺省回退本机 IP） */
  display_name?: string;
  /** 本机 IP 覆盖（可选，留空由后端按到 seed 的出口地址自动探测） */
  node_ip?: string;
}

/** POST /api/cluster/join 响应 */
export interface JoinClusterResult {
  /** 入群后本机 etcd 看到的集群信息 */
  cluster: ClusterInfo;
  /** 本节点 IP（自动探测或显式传入） */
  node_ip: string;
  /** seed 返回的成员对象（可选透传；不可用时为 null） */
  member: Record<string, unknown> | null;
}

/** 本节点入群超时（seed 侧联动 etcd/pg 编排分钟级，远超 30s 默认值） */
const JOIN_CLUSTER_TIMEOUT_MS = 15 * 60 * 1000;

/** 把本节点加入一个已存在的集群（引导请求；seed 复用 /api/nodes 编排） */
export function joinCluster(input: JoinClusterInput): Promise<JoinClusterResult> {
  return post<JoinClusterResult>('/api/cluster/join', input, { timeout: JOIN_CLUSTER_TIMEOUT_MS });
}
