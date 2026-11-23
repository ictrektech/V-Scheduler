import { get, post } from './client';

/** 平台 projects 实例容器实况 */
export interface InfraContainerVo {
  id?: string;
  name?: string;
  service?: string;
  state?: string;
  health?: string;
}

/** 平台 projects 实例运行态 */
export interface InfraRuntimeVo {
  docker_reachable?: boolean;
  aggregate_state?: string;
  containers?: InfraContainerVo[];
}

/** 平台 projects 实例管理态 */
export interface InfraInstanceVo {
  id: string;
  app_id: string;
  project_name: string;
  compose_project_name: string;
  /** deploying | deployed | removing | failed | removed */
  state: string;
  deployed_at?: string;
  removed_at?: string;
  create_time: string;
  updated_at: string;
}

/** 平台 projects 实例详情（infra 状态接口 instances[] 元素） */
export interface InfraInstanceDetail {
  instance?: InfraInstanceVo;
  runtime?: InfraRuntimeVo;
  env_keys?: string[];
}

/** GET /api/infra/etcd/status */
export interface EtcdStatusResult {
  etcd_ok: boolean;
  member_count?: number;
  instances: InfraInstanceDetail[];
  db_ok: boolean;
}

/** patroni leader 信息（etcd DCS 探活成功且有主时返回） */
export interface PatroniReplicaInfo {
  application_name: string;
  client_addr: string;
  state: string;
}

export interface PatroniLeaderInfo {
  role: string; // primary | replica ...
  name: string;
  ip?: string;
  /** 从节点 streaming 列表（leader 响应 replication[]） */
  replication?: PatroniReplicaInfo[];
  [key: string]: unknown;
}

/** GET /api/infra/pg/status */
export interface PgStatusResult {
  postgres_ha_ok: boolean;
  /** leader 未知时 null（patroni 未就绪 / etcd DCS 不可用） */
  patroni_leader: PatroniLeaderInfo | null;
  pg_tcp_ok: boolean;
  dcs_ready: boolean;
  instances: InfraInstanceDetail[];
  db_ok: boolean;
}

/** 拉取 etcd 子项目状态（raft member_count + 实例清单） */
export function fetchEtcdStatus(): Promise<EtcdStatusResult> {
  return get<EtcdStatusResult>('/api/infra/etcd/status');
}

/** 拉取 postgres-ha 子项目状态（patroni leader + 实例清单） */
export function fetchPgStatus(): Promise<PgStatusResult> {
  return get<PgStatusResult>('/api/infra/pg/status');
}

/** GET /api/infra/haproxy/status（二期 S1 cluster-haproxy-lb） */
export interface HaproxyStatusResult {
  /** 本机存在 deployed haproxy 实例且入口可达 */
  haproxy_ok: boolean;
  front_port: number;
  stats_port: number;
  /** postgres HA 后端（etcd 推导的 manager IP 清单） */
  pg_backends: string[];
  deployed_count: number;
  instances: InfraInstanceDetail[];
  /** 本机 127.0.0.1:15432 TCP 探测（主容器 host 网络即本机 haproxy 入口） */
  local_pg_front_reachable: boolean;
  /** patroni leader IP（遍历 managers 8008 /leader 探得；未就绪 null） */
  patroni_leader: string | null;
  db_ok: boolean;
}

/** haproxy deploy 入参（node_ip=目标节点；at 存在=编排方转发到该节点执行） */
export interface HaproxyDeployInput {
  node_ip: string;
  at?: string;
}

/** POST /api/infra/haproxy/deploy 响应 */
export interface HaproxyDeployResult {
  instance: InfraInstanceVo;
  /** upsert 刷新（原活跃实例先 remove 再 deploy） */
  refreshed: boolean;
}

/** 拉取 haproxy 子项目状态 */
export function fetchHaproxyStatus(): Promise<HaproxyStatusResult> {
  return get<HaproxyStatusResult>('/api/infra/haproxy/status');
}

/** 部署/刷新某节点 haproxy 入口（at 缺省=本节点；传目标节点 IP=跨节点转发） */
export function deployHaproxy(input: HaproxyDeployInput): Promise<HaproxyDeployResult> {
  return post<HaproxyDeployResult>('/api/infra/haproxy/deploy', input);
}

/** 移除某节点 haproxy 入口实例（at 传目标节点 IP=跨节点转发） */
export function removeHaproxy(composeProjectName: string, at?: string): Promise<{ removed: boolean }> {
  return post<{ removed: boolean }>('/api/infra/haproxy/remove', {
    compose_project_name: composeProjectName,
    ...(at ? { at } : {}),
  });
}

/** haproxy 实例清单结果 */
export interface HaproxyInstancesResult {
  instances: InfraInstanceVo[];
}

/** GET /api/infra/haproxy/instances —— 各节点 haproxy 实例清单（compose_project_name 含 IP slug） */
export function fetchHaproxyInstances(): Promise<HaproxyInstancesResult> {
  return get<HaproxyInstancesResult>('/api/infra/haproxy/instances');
}

/** 跨节点 haproxy 入口部署态（节点管理页「统一入口」列） */
export interface HaproxyNodeVo {
  node_ip: string;
  compose_project_name: string | null;
  haproxy_present: boolean;
}

export interface HaproxyNodesResult {
  nodes: HaproxyNodeVo[];
  front_port: number;
}

/** GET /api/infra/haproxy/nodes —— 遍历 etcd 节点记录，逐节点查目标 haproxy 实例是否存在 */
export function fetchHaproxyNodes(): Promise<HaproxyNodesResult> {
  return get<HaproxyNodesResult>('/api/infra/haproxy/nodes');
}

// ============================================================================
// 二期 S2：集群级 temporal（project_name=temporal，仅 manager 单实例 D-TE-02）
// ============================================================================

/** temporal 单节点刷新结果（deploy/remove 后联动刷新全集群 haproxy 的逐节点反馈） */
export interface HaproxyRefreshResult {
  node_ip: string;
  ok: boolean;
  err?: string;
}

/** GET /api/infra/temporal/status（本机视角：实例 + 端口探测，业务态） */
export interface TemporalStatusResult {
  /** 本机存在 deployed temporal 实例且 frontend 7333 可达 */
  temporal_ok: boolean;
  frontend_port: number;
  haproxy_frontend_port: number;
  ui_port: number;
  deployed_count: number;
  instances: InfraInstanceDetail[];
  /** 本机 127.0.0.1:7333（temporal frontend gRPC） */
  local_frontend_reachable: boolean;
  /** 本机 127.0.0.1:7133（haproxy temporal 入口转发） */
  front_via_haproxy_reachable: boolean;
  /** 本机 127.0.0.1:15432（haproxy pg 入口——temporal DB 通道） */
  db_front_reachable: boolean;
  db_ok: boolean;
}

/** temporal deploy 入参（node_ip=目标节点；at 存在=编排方转发到该节点执行） */
export interface TemporalDeployInput {
  node_ip: string;
  at?: string;
}

/** POST /api/infra/temporal/deploy 响应（含 haproxy 联动刷新逐节点结果） */
export interface TemporalDeployResult {
  instance: InfraInstanceVo;
  refreshed: boolean;
  haproxy_refresh?: HaproxyRefreshResult[];
}

/** 拉取集群 temporal 子项目状态（本机视角） */
export function fetchTemporalStatus(): Promise<TemporalStatusResult> {
  return get<TemporalStatusResult>('/api/infra/temporal/status');
}

/** 部署/刷新集群 temporal（at 缺省=本节点；传目标 manager IP=跨节点转发，密码不出本机） */
export function deployTemporal(input: TemporalDeployInput): Promise<TemporalDeployResult> {
  return post<TemporalDeployResult>('/api/infra/temporal/deploy', input);
}

/** 移除集群 temporal 实例（at 传目标节点 IP=跨节点转发） */
export function removeTemporal(
  composeProjectName: string,
  at?: string,
): Promise<{ removed: boolean; compose_project_name: string; haproxy_refresh?: HaproxyRefreshResult[] }> {
  return post('/api/infra/temporal/remove', {
    compose_project_name: composeProjectName,
    ...(at ? { at } : {}),
  });
}

/** 跨节点 temporal 部署态（节点管理页；deployable=false 仅 worker） */
export interface TemporalNodeVo {
  node_ip: string;
  compose_project_name: string | null;
  temporal_present: boolean;
  deployable: boolean;
}

export interface TemporalNodesResult {
  nodes: TemporalNodeVo[];
  frontend_port: number;
}

/** GET /api/infra/temporal/nodes —— 遍历 etcd 节点记录，逐节点查目标 temporal 实例是否存在 */
export function fetchTemporalNodes(): Promise<TemporalNodesResult> {
  return get<TemporalNodesResult>('/api/infra/temporal/nodes');
}

/** POST /api/infra/haproxy/refresh_all —— 全节点刷新 haproxy 配置（temporal 收缩兜底） */
export function refreshAllHaproxy(): Promise<{ count: number; refreshed: HaproxyRefreshResult[] }> {
  return post('/api/infra/haproxy/refresh_all', {});
}