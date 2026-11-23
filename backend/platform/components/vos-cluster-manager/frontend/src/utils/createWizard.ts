import type { CreateClusterInput, CreateNodeInput } from '../api/cluster';
import type { NodeRole } from './roles';
// Node 24 TS-strip 运行时导入需显式 .ts 扩展名（vue/vite 亦支持）
import { isValidIpv4, validatePassword, isValidClusterName, MIN_PASSWORD_LEN } from './validation.ts';
import { normalizeRole, isManagerRole } from './roles.ts';

/**
 * 创建向导数据模型与校验纯函数（可单测，不依赖 DOM）。
 * 步骤模型：① 集群信息 → ② 节点录入 → ③ 确认 → ④ 提交 → ⑤ 进度。
 *
 * 角色约束（cluster-role-and-create-entry）：
 * - **首行（发起节点）必须为管理节点**（role=manager，锁 radio），与后端
 *   `nodes[0]` 必为 manager 的校验一致——编排在本机自举 etcd + 高可用 postgres；
 * - 其余行默认工作节点（worker，不部署基础设施），可切换为管理节点。
 */

/** 向导节点录入行 */
export interface WizardNodeRow {
  node_ip: string;
  admin_password: string;
  display_name: string;
  role: NodeRole;
}

/** 向导全局状态（steps 间保持） */
export interface WizardState {
  cluster_name: string;
  nodes: WizardNodeRow[];
}

/** 新建向导初始状态（1 个空行；首行为发起节点，固定管理节点） */
export function createWizardState(): WizardState {
  return {
    cluster_name: '',
    nodes: [{ node_ip: '', admin_password: '', display_name: '', role: 'manager' }],
  };
}

/**
 * 发起节点是否合法（首行必须为管理节点）。
 * 若首行 role 被误改为 worker（radio 虽锁定，校验仍兜底），返回错误信息。
 */
export function firstRowRoleError(state: WizardState): string {
  if (state.nodes.length === 0) return '';
  if (!isManagerRole(state.nodes[0]?.role)) return '发起节点（首行）必须为管理节点';
  return '';
}

/** 步骤 ① 集群信息校验（返回错误信息，空串表示通过） */
export function validateClusterInfo(name: string): string {
  if (!isValidClusterName(name)) return '集群名称必填';
  return '';
}

/** 步骤 ② 单行校验（返回错误信息，空串表示通过） */
export function validateNodeRowField(row: WizardNodeRow): string {
  if (!isValidIpv4(row.node_ip)) return '节点 IP 非法';
  if (!validatePassword(row.admin_password)) return `管理员密码需 ≥${MIN_PASSWORD_LEN} 位`;
  return '';
}

/** 步骤 ② 整体校验：返回每行错误（index → 错误信息）与整体是否通过 */
export function validateNodeRows(rows: WizardNodeRow[]): { errors: Record<number, string>; ok: boolean } {
  const errors: Record<number, string> = {};
  let ok = true;
  if (rows.length === 0) {
    return { errors: { 0: '至少录入 1 个节点' }, ok: false };
  }
  rows.forEach((row, i) => {
    const err = validateNodeRowField(row);
    if (err) {
      errors[i] = err;
      ok = false;
    }
  });
  return { errors, ok };
}

/** 构建提交体（display_name 空 → 回退 node_ip；role 经 normalizeRole 兜底 worker） */
export function buildCreateInput(state: WizardState): CreateClusterInput {
  return {
    cluster_name: state.cluster_name.trim(),
    nodes: state.nodes.map<CreateNodeInput>((row) => ({
      node_ip: row.node_ip.trim(),
      admin_password: row.admin_password,
      role: normalizeRole(row.role) || 'worker',
      display_name: row.display_name.trim() || undefined,
    })),
  };
}