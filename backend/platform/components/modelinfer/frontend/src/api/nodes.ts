import { buildQuery, type PageMeta, type PageQuery } from './paging';
import { del, get, post, put } from './client';

/** node-exporter 默认监听端口（避开上游标准端口 9100，避免与节点自带监控栈冲突） */
export const DEFAULT_NODE_EXPORTER_PORT = 19100;
/** nvidia-gpu-exporter 默认监听端口（避开上游默认端口 9835） */
export const DEFAULT_NVIDIA_GPU_EXPORTER_PORT = 19835;

export interface InterfaceInfo {
  name: string;
  ips: string[];
}

/** 单张 GPU（同一厂商内逐卡记录） */
export interface GpuDevice {
  index: number;
  name: string;
  memory_total_mib: number;
  pci_bus_id: string;
}

export interface NodeItem {
  id: string;
  name: string;
  host: string;
  /** 访问模式：ssh / local（本机 nsenter） */
  access_mode: string;
  ssh_port: number;
  ssh_user: string;
  /** 认证方式：password / key */
  auth_method: string;
  has_password: boolean;
  has_key: boolean;
  has_sudo_password?: boolean;
  hostname: string;
  profile?: string;
  /** CPU 架构（uname -m） */
  arch: string;
  /** 内核版本 */
  kernel_version: string;
  /** 默认网关 */
  gateway: string;
  /** GPU 厂商（nvidia / amd / intel；空 = 未识别） */
  gpu_vendor: string;
  /** GPU 型号汇总 */
  gpu_model: string;
  /** 全部 GPU 设备 */
  gpu_devices: GpuDevice[];
  socket_ifname: string;
  ib_hca: string;
  interfaces: InterfaceInfo[];
  ib_devices: string[];
  fscache: boolean;
  /** node-exporter 监听端口（高级配置，默认 19100） */
  node_exporter_port: number;
  /** nvidia-gpu-exporter 监听端口（高级配置，默认 19835） */
  nvidia_gpu_exporter_port: number;
  status: string;
  /** 是否被至少一个活跃运行配置使用（后端实时派生） */
  busy: boolean;
  /** 首个使用该节点的运行配置 id（空闲为空串；兼容旧后端） */
  busy_deployment_id: string;
  /** 首个使用该节点的运行配置名称（空闲为空串；兼容旧后端） */
  busy_deployment_name: string;
  /** 使用该节点的全部运行配置 id（旧后端缺省为空数组） */
  busy_deployment_ids?: string[];
  /** 使用该节点的全部运行配置名称（旧后端缺省为空数组） */
  busy_deployment_names?: string[];
  last_checked_at?: string | null;
  created_at: string;
  updated_at: string;
  /** 监控状态摘要（后端由监控表实时派生） */
  monitor_state?: string;
  monitor_last_error?: string;
  monitor_deployed_at?: string | null;
}

/** 监控部署步骤 */
export interface MonitorStep {
  name: string;
  title: string;
  state: string;
  detail: string;
  started_at: string;
  finished_at?: string | null;
}

/** 节点监控状态（单个组件） */
export interface NodeMonitor {
  node_id: string;
  /** 组件标识：node-exporter / nvidia-gpu-exporter */
  component: string;
  /** 指标分类：system / gpu */
  category: string;
  state: string;
  service_id: string;
  service_name: string;
  image: string;
  workflow_id: string;
  last_error: string;
  steps: MonitorStep[];
  deployed_at?: string | null;
  created_at: string;
  updated_at: string;
}

/** 节点监控状态（按组件列表） */
export interface NodeMonitors {
  node_id: string;
  components: NodeMonitor[];
}

/** 单条指标时间序列 */
export interface MetricSeries {
  labels: Record<string, string>;
  points: [number, number][];
}

/** 指标组 */
export interface MetricGroup {
  key: string;
  name: string;
  unit: string;
  /** prometheus job（= consul 服务名） */
  source: string;
  /** 指标分类：system / gpu */
  category: string;
  series: MetricSeries[];
}

/** 指标查询响应（通用） */
export interface MetricsResponse {
  range: string;
  start: number;
  end: number;
  step: number;
  groups: MetricGroup[];
}

export interface NodeMetrics extends MetricsResponse {
  node_id: string;
  instance: string;
  /** GPU 指标实例（仅 NVIDIA 节点） */
  gpu_instance?: string | null;
}

export interface MetricsQuery {
  range?: string;
  start?: number;
  end?: number;
  step?: number;
}

/** 节点未筛选统计（页头统计条） */
export interface NodeStats {
  total: number;
  online: number;
  offline: number;
  fscache: number;
  /** 至少承载一个活跃运行配置的节点数 */
  busy: number;
}

export interface NodeListData extends PageMeta {
  data: NodeItem[];
  /** 未筛选统计 */
  stats: NodeStats;
}

/** 节点列表查询参数 */
export interface NodesQuery extends PageQuery {
  keyword?: string;
  status?: string;
  sort?: string;
}

/** 节点连接（认证）信息 */
export interface NodeCredentials {
  /** password / key（缺省 password） */
  auth_method?: string;
  ssh_password?: string;
  ssh_key?: string;
  /** sudo 密码；更新时省略保留，空字符串清除 */
  sudo_password?: string;
}

export interface NodeCreateReq extends NodeCredentials {
  name: string;
  host: string;
  /** 访问模式：ssh（缺省）/ local */
  access_mode?: string;
  ssh_port: number;
  ssh_user?: string;
  hostname?: string;
  socket_ifname?: string;
  ib_hca?: string;
  fscache?: boolean;
  node_exporter_port?: number;
  nvidia_gpu_exporter_port?: number;
}

export interface NodeUpdateReq extends NodeCredentials {
  name?: string;
  host?: string;
  ssh_port?: number;
  ssh_user?: string;
  hostname?: string;
  socket_ifname?: string;
  ib_hca?: string;
  fscache?: boolean;
  node_exporter_port?: number;
  nvidia_gpu_exporter_port?: number;
}

/** 扫描结果（不落库） */
export interface NodeScanResult {
  hostname: string;
  arch: string;
  kernel_version: string;
  gateway: string;
  interfaces: InterfaceInfo[];
  default_socket_ifname?: string | null;
  ib_devices: string[];
  suggested_ib_hca?: string | null;
  gpu: {
    vendor: string;
    driver_version: string;
    devices: GpuDevice[];
  };
}

/** 当前宿主机探测结果（添加「当前节点」用，不落库） */
export interface LocalNodeInfo {
  host: string;
  scan: NodeScanResult;
}

/** 经 nsenter 扫描当前宿主机 */
export function fetchLocalNodeInfo(): Promise<LocalNodeInfo> {
  return get<LocalNodeInfo>('/api/nodes/local-info');
}

export function listNodes(params: NodesQuery = {}): Promise<NodeListData> {
  return get<NodeListData>(`/api/nodes${buildQuery({ ...params })}`);
}

export function getNode(id: string): Promise<NodeItem> {
  return get<NodeItem>(`/api/nodes/${id}`);
}

export function createNode(req: NodeCreateReq): Promise<NodeItem> {
  return post<NodeItem>('/api/nodes', req);
}

export function updateNode(id: string, req: NodeUpdateReq): Promise<NodeItem> {
  return put<NodeItem>(`/api/nodes/${id}`, req);
}

export function deleteNode(id: string): Promise<{ deleted: string }> {
  return del<{ deleted: string }>(`/api/nodes/${id}`);
}

export function testNode(id: string): Promise<NodeItem> {
  return post<NodeItem>(`/api/nodes/${id}/test`);
}

/** 连接目标设备扫描环境与硬件（不落库） */
export function probeNode(req: NodeCreateReq): Promise<NodeScanResult> {
  return post<NodeScanResult>('/api/nodes/probe', req);
}

/** 重新扫描已有节点 */
export function rescanNode(id: string): Promise<NodeItem> {
  return post<NodeItem>(`/api/nodes/${id}/rescan`);
}
/** 节点电源动作 */
export type NodePowerAction = 'shutdown' | 'reboot';

export interface NodePowerReq {
  action: NodePowerAction;
  /** 节点被活跃运行配置占用时需显式强制 */
  force?: boolean;
}

/** 电源操作结果：仅表示指令已下发，不表示已完成断电 / 重启 */
export interface NodePowerResult {
  node_id: string;
  action: NodePowerAction;
  force: boolean;
  accepted_at: string;
}

/** 下发节点关机 / 重启指令 */
export function powerNode(id: string, req: NodePowerReq): Promise<NodePowerResult> {
  return post<NodePowerResult>(`/api/nodes/${id}/power`, req);
}

export function shutdownNode(id: string, force = false): Promise<NodePowerResult> {
  return powerNode(id, { action: 'shutdown', force });
}

export function rebootNode(id: string, force = false): Promise<NodePowerResult> {
  return powerNode(id, { action: 'reboot', force });
}

/** 节点监控状态（各组件） */
export function getNodeMonitor(id: string): Promise<NodeMonitors> {
  return get<NodeMonitors>(`/api/nodes/${id}/monitor`);
}

/** 触发节点监控组件部署（按适用组件，失败可重试） */
export function deployNodeMonitor(id: string): Promise<NodeMonitors> {
  return post<NodeMonitors>(`/api/nodes/${id}/monitor/deploy`);
}

/** 节点指标（prometheus query_range 代理） */
export function getNodeMetrics(id: string, params: MetricsQuery = {}): Promise<NodeMetrics> {
  return get<NodeMetrics>(`/api/nodes/${id}/metrics${buildQuery({ ...params })}`);
}
