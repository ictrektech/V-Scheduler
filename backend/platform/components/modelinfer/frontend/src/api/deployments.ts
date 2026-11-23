import { buildQuery, type PageMeta, type PageQuery } from './paging';
import { del, get, post, put } from './client';
import type { MetricsQuery, MetricsResponse } from './nodes';

export interface DeploymentNodeItem {
  id: string;
  name: string;
  host: string;
  hostname: string;
  socket_ifname: string;
  ib_hca: string;
  status: string;
  role: string;
}

/** 镜像声明条目（与卡片 manifest `images` 同结构） */
export type ImageEntry = string | { name: string; arch?: string };

/** 权重覆盖 */
export interface WeightOverride {
  source: string;
  model_id: string;
}

export interface DeploymentItem {
  id: string;
  name: string;
  plaza_model_id: string;
  card_id: string;
  card_name: string;
  weight_id: string;
  /** 家族标识（后端 SSOT 推断，未命中为 `generic`） */
  family: string;
  /** 卡片 manifest 声明的推理引擎类型（`inference-engine`；缺省为空串） */
  engine: string;
  hub_source: string;
  hub_rel_path: string;
  node_ids: string[];
  head_node_id: string;
  nodes: DeploymentNodeItem[];
  /**
   * 已删除 / 不存在的节点 id（后端派生字段；旧后端缺省时前端按 `nodes[].status` 回退）
   *
   * 见 `openspec/changes/modelinfer-dangling-node-reference/`。
   */
  missing_node_ids?: string[];
  configs: Record<string, unknown>;
  /** 权重覆盖（null = 使用卡片默认权重） */
  weight_override?: WeightOverride | null;
  /** 是否覆盖权重 */
  weight_overridden: boolean;
  /** 生效权重（覆盖优先，否则卡片默认） */
  effective_weight_id: string;
  effective_hub_source: string;
  effective_hub_rel_path: string;
  /** 镜像覆盖条目（空 = 使用卡片声明） */
  images_override: ImageEntry[];
  /** 生效镜像条目（覆盖优先，否则卡片声明） */
  effective_images: ImageEntry[];
  /** 生效镜像来源：card / override */
  images_source: 'card' | 'override' | string;
  /** 额外环境变量（在卡片 configs 之后注入） */
  extra_env: Record<string, string>;
  /** 标准接入参数（API Key 不在此回显，见 getDeploymentAccess） */
  serve_model_name: string;
  serve_port: number;
  serve_url_prefix: string;
  /** 统一网关（V-Scheduler / 兼容旧 V-Router）注册状态：disabled / unregistered / registered / error */
  vrouter_state: string;
  vrouter_upstream_id: string;
  vrouter_model: string;
  vrouter_registered_at?: string | null;
  vrouter_last_error: string;
  /** 本地反代（本机 TCP 端口 → head vLLM） */
  local_proxy_enabled: boolean;
  /** 期望端口（0 = 自动分配） */
  local_proxy_port: number;
  /** 实际监听端口（0 = 未运行） */
  local_proxy_active_port: number;
  local_proxy_last_error: string;
  /** 探活失败自动重启：开关 / 连续失败阈值（秒） */
  auto_restart_enabled: boolean;
  auto_restart_fail_secs: number;
  /** 自动重启运行态：相位（"" / stopping / starting）/ 失败起点 / backoff 截止 / 尝试次数 /
   *  最近动作时间 / 最近错误 */
  auto_restart_phase: string;
  auto_restart_failed_since: string;
  auto_restart_next_at: string;
  auto_restart_attempts: number;
  auto_restart_last_at: string;
  auto_restart_last_error: string;
  /** 卡片是否声明 `lifecycle.health-probe`（未声明时自动重启不会生效） */
  health_probe_declared: boolean;
  /** 推理引擎类型（vllm / unknown / 空 = 未探测） */
  inference_engine: string;
  inference_engine_version: string;
  /** 引擎来源：script / probe / unknown */
  inference_engine_source: string;
  /** 推理指标注册状态：unregistered / registered / error / unsupported */
  metrics_state: string;
  metrics_service_id: string;
  metrics_registered_at?: string | null;
  metrics_last_error: string;
  state: string;
  desired_state: string;
  health_status: string;
  last_error: string;
  last_started_at?: string | null;
  created_at: string;
  updated_at: string;
}

/** 运行配置未筛选统计（页头统计条） */
export interface DeploymentStats {
  total: number;
  /** `starting | running | degraded` */
  running: number;
  stopped: number;
  failed: number;
}

/** 活跃运行配置对节点的承载项（同一节点可有多项；仅用于展示 / 统计） */
export interface OccupiedNode {
  node_id: string;
  deployment_id: string;
  deployment_name: string;
}

export interface DeploymentListData extends PageMeta {
  data: DeploymentItem[];
  /** 未筛选统计 */
  stats: DeploymentStats;
  /** 活跃运行配置对节点的承载明细（同一节点可有多项） */
  occupied_nodes: OccupiedNode[];
}

/** 运行配置列表查询参数 */
export interface DeploymentsQuery extends PageQuery {
  keyword?: string;
  state?: string;
  card_id?: string;
  sort?: string;
}

export interface DeploymentCreateReq {
  name: string;
  plaza_model_id: string;
  hub_source: string;
  node_ids: string[];
  configs: Record<string, unknown>;
  /** 权重覆盖（`model_id` 为空 = 使用卡片默认权重） */
  weight_override?: WeightOverride | null;
  /** 镜像覆盖（空数组 = 使用卡片 manifest.images） */
  images_override?: ImageEntry[];
  /** 额外环境变量（在卡片 configs 之后注入） */
  extra_env?: Record<string, string>;
  /** 标准接入参数（留空 = 平台自动推断） */
  serve_model_name?: string;
  serve_api_key?: string;
  serve_port?: number | null;
  serve_url_prefix?: string;
  /** 本地反代：是否开启（默认关闭） */
  local_proxy_enabled?: boolean;
  /** 本地反代：期望端口（空 / 0 = 自动分配 31000~31100） */
  local_proxy_port?: number | null;
  /** 探活失败自动重启（默认关闭） */
  auto_restart_enabled?: boolean;
  /** 自动重启连续失败阈值（秒，默认 300，范围 30~86400） */
  auto_restart_fail_secs?: number;
}

/** V-Router 注册信息 */
export interface VrouterInfo {
  state: string;
  upstream_id: string;
  model_id: string;
  registered_at?: string | null;
  last_error: string;
}

/** 本地反代信息 */
export interface LocalProxyInfo {
  enabled: boolean;
  /** 期望端口（0 = 自动分配） */
  port: number;
  /** 实际监听端口（0 = 未运行） */
  active_port: number;
  /** 转发目标 head_host:serve_port */
  target: string;
  last_error: string;
}

/** 运行配置的接入配置 */
export interface DeploymentAccess {
  deployment_id: string;
  available: boolean;
  deployment_state: string;
  host: string;
  port: number;
  url_prefix: string;
  model: string;
  api_key: string;
  base_url: string;
  openai_base_url: string;
  /** 浏览器访问地址（`http://<host>:<port>`，无 URL 前缀；host 为空时为空串） */
  web_url: string;
  sources: Record<string, string>;
  vrouter: VrouterInfo;
  local_proxy: LocalProxyInfo;
}

/** V-Router 检测状态（不暴露网关地址） */
export interface VrouterStatus {
  installed: boolean;
  reachable: boolean;
  credential_present: boolean;
  registered_count: number;
  message: string;
}

export function listDeployments(params: DeploymentsQuery = {}): Promise<DeploymentListData> {
  return get<DeploymentListData>(`/api/deployments${buildQuery({ ...params })}`);
}

export function getDeployment(id: string): Promise<DeploymentItem> {
  return get<DeploymentItem>(`/api/deployments/${id}`);
}

export function createDeployment(req: DeploymentCreateReq): Promise<DeploymentItem> {
  return post<DeploymentItem>('/api/deployments', req);
}

export function updateDeployment(id: string, req: Partial<DeploymentCreateReq>): Promise<DeploymentItem> {
  return put<DeploymentItem>(`/api/deployments/${id}`, req);
}

export function deleteDeployment(id: string): Promise<{ deleted: string }> {
  return del<{ deleted: string }>(`/api/deployments/${id}`);
}

export function startDeployment(id: string): Promise<DeploymentItem> {
  return post<DeploymentItem>(`/api/deployments/${id}/start`);
}

export function stopDeployment(id: string, force = false): Promise<DeploymentItem> {
  return post<DeploymentItem>(`/api/deployments/${id}/stop${force ? '?force=true' : ''}`);
}

/** 运行配置推理指标响应（复用通用指标组结构） */
export interface DeploymentMetrics extends MetricsResponse {
  deployment_id: string;
  /** 引擎字面值（vllm / unknown / 空） */
  engine: string;
  engine_version: string;
  /** 该引擎是否支持指标采集 */
  supported: boolean;
  /** 当前是否处于采集状态（运行态且 prometheus 目标在线）；仅提示位，不活跃时仍返回历史数据 */
  active: boolean;
  /** 本次结果中最近采样时间（unix 秒）；无数据为 null */
  last_sample?: number | null;
  /** prometheus 实例标签（`<head IP>:<serve_port>`；不受支持时为 null） */
  instance?: string | null;
}

/** 运行配置推理指标（受支持引擎时返回指标组；否则空组） */
export function getDeploymentMetrics(
  id: string,
  params: MetricsQuery = {},
): Promise<DeploymentMetrics> {
  return get<DeploymentMetrics>(`/api/deployments/${id}/metrics${buildQuery({ ...params })}`);
}

/** 运行配置的接入配置（端点 / 模型名 / API Key / V-Router 状态） */
export function getDeploymentAccess(id: string): Promise<DeploymentAccess> {
  return get<DeploymentAccess>(`/api/deployments/${id}/access`);
}

/** V-Router 检测状态 */
export function getVrouterStatus(): Promise<VrouterStatus> {
  return get<VrouterStatus>('/api/vrouter/status');
}

/** 手动立即注册到 V-Router（best-effort） */
export function registerDeploymentVrouter(id: string): Promise<DeploymentAccess> {
  return post<DeploymentAccess>(`/api/deployments/${id}/vrouter/register`);
}

/** 在线开启/关闭本地反代并调整端口（运行中也可操作） */
export function updateDeploymentLocalProxy(
  id: string,
  req: { enabled: boolean; port?: number | null },
): Promise<DeploymentAccess> {
  return put<DeploymentAccess>(`/api/deployments/${id}/local-proxy`, req);
}

/** ComfyUI 实例权重台账记录 */
export interface ComfyuiWeightItem {
  id: string;
  deployment_id: string;
  source: string;
  weight_id: string;
  file_path: string;
  target_type: string;
  link_name: string;
  /** 实例权重目录内相对路径（`<target_type>/<link_name>`） */
  link_rel: string;
  /** 软链接目标（相对路径） */
  target_rel: string;
  created_at: string;
  updated_at: string;
}

/** ComfyUI 实例权重台账响应 */
export interface ComfyuiWeightsData {
  instance_id: string;
  /** ComfyUI 预设类型目录清单 */
  types: string[];
  weights: ComfyuiWeightItem[];
}

/** 添加 ComfyUI 实例权重请求 */
export interface ComfyuiAddWeightReq {
  source: string;
  weight_id: string;
  file_path: string;
  target_type: string;
  /** 链接名（留空取源文件基名） */
  link_name?: string;
}

/** ComfyUI 实例权重台账 */
export function listComfyuiWeights(id: string): Promise<ComfyuiWeightsData> {
  return get<ComfyuiWeightsData>(`/api/deployments/${id}/comfyui/weights`);
}

/** 添加 ComfyUI 实例权重（相对软链 + 台账） */
export function addComfyuiWeight(id: string, req: ComfyuiAddWeightReq): Promise<ComfyuiWeightItem> {
  return post<ComfyuiWeightItem>(`/api/deployments/${id}/comfyui/weights`, req);
}

/** 删除 ComfyUI 实例权重（软链 + 台账） */
export function deleteComfyuiWeight(id: string, recordId: string): Promise<{ deleted: string }> {
  return del<{ deleted: string }>(`/api/deployments/${id}/comfyui/weights/${recordId}`);
}