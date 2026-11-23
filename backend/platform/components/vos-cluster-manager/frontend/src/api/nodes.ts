import { del, get, post } from './client';

/** 节点视图（后端 NodeRecordView 脱敏——无 admin_password；role 已归一化 manager|worker） */
export interface NodeRecordView {
  member_id: string;
  display_name: string;
  role: string; // manager（管理节点）| worker（工作节点）
  node_ip: string;
  api_endpoint: string;
  joined_at: string;
}

/** GET /api/nodes 响应 */
export interface NodeListResult {
  nodes: NodeRecordView[];
}

/** POST /api/nodes 请求体 */
export interface NodeAddInput {
  node_ip: string;
  admin_password: string;
  /** 节点角色：manager（管理节点，允许 join_infra）| worker（工作节点，不允许部署基础设施） */
  role?: string;
  /** 展示名称（缺省回退 node_ip） */
  display_name?: string;
  /** 联动部署 etcd/pg 子项目（缺省 true；false 仅登记元数据） */
  join_infra?: boolean;
}

/** POST /api/nodes 响应 */
export interface NodeAddResult {
  node: NodeRecordView;
}

/** DELETE /api/nodes/:id 响应 */
export interface NodeDeleteResult {
  deleted: boolean;
  member_id: string;
}

/** 加入节点超时（join_infra 联动编排 etcd/postgres-ha 需分钟级，避免 30s 默认超时中断 UI） */
const ADD_NODE_TIMEOUT_MS = 15 * 60 * 1000;
/** 移除节点超时（联动清理 raft 成员与子项目同样耗时） */
const REMOVE_NODE_TIMEOUT_MS = 10 * 60 * 1000;

/** 拉取成员表（脱敏视图） */
export function fetchNodes(): Promise<NodeListResult> {
  return get<NodeListResult>('/api/nodes');
}

/** 加入节点（join_infra=true 联动部署 etcd/pg 子项目） */
export function addNode(input: NodeAddInput): Promise<NodeAddResult> {
  return post<NodeAddResult>('/api/nodes', input, { timeout: ADD_NODE_TIMEOUT_MS });
}

/** 移除节点（remove_infra=true 联动清理 etcd raft 成员 + 子项目，缺省 true） */
export function removeNode(memberId: string, removeInfra = true): Promise<NodeDeleteResult> {
  return del<NodeDeleteResult>(
    `/api/nodes/${memberId}`,
    { remove_infra: removeInfra },
    { timeout: REMOVE_NODE_TIMEOUT_MS },
  );
}