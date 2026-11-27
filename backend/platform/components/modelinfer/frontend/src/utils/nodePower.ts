/** 节点电源操作的共享影响判断（纯函数，便于单测与列表 / 详情复用）。 */
import type { NodeItem } from '@/api/nodes';

export type NodePowerAction = 'shutdown' | 'reboot';

export interface NodePowerImpact {
  /** 本机节点：操作后管理页面会随宿主机一起不可用 */
  local: boolean;
  /** 是否被活跃运行配置占用 */
  busy: boolean;
  /** 承载的运行配置名称（去重后） */
  deploymentNames: string[];
  /** 未强制时后端会拒绝，前端应直接以 force 确认 */
  needsForceConfirm: boolean;
}

type PowerNodeLike = Pick<NodeItem, 'access_mode'> &
  Partial<
    Pick<
      NodeItem,
      'busy' | 'busy_deployment_id' | 'busy_deployment_name' | 'busy_deployment_ids' | 'busy_deployment_names'
    >
  >;

/** 派生节点电源操作的影响面。 */
export function nodePowerImpact(node: PowerNodeLike): NodePowerImpact {
  const multi = (node.busy_deployment_names ?? []).filter(Boolean);
  const names = multi.length
    ? multi
    : node.busy_deployment_name
      ? [node.busy_deployment_name]
      : node.busy_deployment_id
        ? [node.busy_deployment_id]
        : [];
  const busy = node.busy === true || names.length > 0;
  return {
    local: node.access_mode === 'local',
    busy,
    deploymentNames: names,
    needsForceConfirm: busy,
  };
}
