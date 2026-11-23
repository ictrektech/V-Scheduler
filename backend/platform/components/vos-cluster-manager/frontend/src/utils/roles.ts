/**
 * 节点角色展示纯函数（cluster-role-and-create-entry：manager/worker 角色模型）。
 * 可单测（Node 24 TS-strip + node:test），不依赖 DOM。
 *
 * 角色语义：
 * - manager（管理节点）：运行 etcd、ha postgres，基于 raft 与 postgres 数据同步
 *   维护集群高可用状态；本身是特殊 worker，也可运行集群化工作负载。
 * - worker（工作节点）：不运行 etcd 与 ha postgres，可运行其他集群化工作负载。
 */

/** 节点角色类型（后端 dto::role：manager / worker） */
export type NodeRole = 'manager' | 'worker';

/** 两个角色（用于 radio 选项渲染） */
export const NODE_ROLES: readonly NodeRole[] = ['manager', 'worker'] as const;

/** 旧值（一期 master/node）归一化（与后端 NodeRecordView 归一化对齐） */
export function normalizeRole(role?: string): string {
  if (role === 'master') return 'manager';
  if (role === 'node') return 'worker';
  return role ?? '';
}

/** 是否为管理节点（旧值 master 也视为管理节点） */
export function isManagerRole(role?: string): boolean {
  return normalizeRole(role) === 'manager';
}

/** 角色展示文案：manager → 管理节点 / worker → 工作节点 / 其他原样 */
export function roleLabel(role?: string): string {
  const r = normalizeRole(role);
  if (r === 'manager') return '管理节点';
  if (r === 'worker') return '工作节点';
  return r || '未知';
}

/** 角色徽标颜色：manager → green / worker → blue / 其他 default */
export function roleColor(role?: string): string {
  const r = normalizeRole(role);
  if (r === 'manager') return 'green';
  if (r === 'worker') return 'blue';
  return 'default';
}