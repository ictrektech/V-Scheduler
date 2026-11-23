/**
 * 运行配置 → 节点引用工具（纯函数）
 *
 * 后端在节点已被删除时返回占位节点视图（`status="missing"`）并给出 `missing_node_ids`
 * （见 `openspec/changes/modelinfer-dangling-node-reference/`）。本模块集中：
 * - 判断运行配置是否存在失效节点（兼容旧后端：无 `missing_node_ids` 时按节点状态回退）；
 * - 编辑回填时把失效节点从选择中拆出，避免保存必然失败。
 */

/** 节点占位状态：节点已被删除（后端 `DeploymentNodeView.status` 保留值） */
export const MISSING_NODE_STATUS = 'missing';

export interface NodeRefLike {
  id: string;
  status: string;
}

export interface NodeRefRow {
  node_ids: string[];
  nodes: NodeRefLike[];
  /** 后端新增字段；旧后端缺省时回退遍历 `nodes` 的 `status` */
  missing_node_ids?: string[];
}

/** 失效节点 id 列表（保持 `node_ids` 顺序、去重） */
export function missingNodeIds(row: NodeRefRow): string[] {
  if (Array.isArray(row.missing_node_ids)) return [...row.missing_node_ids];
  const out: string[] = [];
  for (const id of row.node_ids) {
    const node = row.nodes.find((n) => n.id === id);
    if (node && node.status === MISSING_NODE_STATUS && !out.includes(id)) out.push(id);
  }
  return out;
}

/** 运行配置是否引用已删除节点 */
export function hasMissingNode(row: NodeRefRow): boolean {
  return missingNodeIds(row).length > 0;
}

/**
 * 编辑回填：按「节点台账仍存在」拆分选择。
 *
 * @param nodeIds 运行配置持久化的节点选择（首元素为 head）
 * @param existingNodeIds 当前节点台账 id 集合
 * @returns `kept` 保持原顺序（去重）；`stale` 为已删除节点（保持原顺序、去重）
 */
export function splitNodeSelection(
  nodeIds: string[],
  existingNodeIds: Iterable<string>,
): { kept: string[]; stale: string[] } {
  const existing = new Set(existingNodeIds);
  const kept: string[] = [];
  const stale: string[] = [];
  for (const id of nodeIds) {
    if (!id) continue;
    if (existing.has(id)) {
      if (!kept.includes(id)) kept.push(id);
    } else if (!stale.includes(id)) {
      stale.push(id);
    }
  }
  return { kept, stale };
}

/** head 失效时回落到首个仍存在的节点（无有效节点时为空串） */
export function fallbackHead(head: string, kept: string[]): string {
  return kept.includes(head) ? head : kept[0] || '';
}