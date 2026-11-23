import { describe, expect, it } from 'vitest';
import {
  MISSING_NODE_STATUS,
  fallbackHead,
  hasMissingNode,
  missingNodeIds,
  splitNodeSelection,
} from '../nodeRefs';

describe('missingNodeIds', () => {
  it('优先后端 missing_node_ids', () => {
    const row = {
      node_ids: ['a', 'b'],
      nodes: [
        { id: 'a', status: MISSING_NODE_STATUS },
        { id: 'b', status: 'online' },
      ],
      missing_node_ids: ['a'],
    };
    expect(missingNodeIds(row)).toEqual(['a']);
    expect(hasMissingNode(row)).toBe(true);
  });

  it('旧后端无 missing_node_ids 时按节点状态回退（保持 node_ids 顺序、去重）', () => {
    const row = {
      node_ids: ['b', 'a', 'a'],
      nodes: [
        { id: 'a', status: MISSING_NODE_STATUS },
        { id: 'b', status: 'online' },
      ],
    };
    expect(missingNodeIds(row)).toEqual(['a']);
    expect(hasMissingNode(row)).toBe(true);
  });

  it('节点齐全时为 false', () => {
    const row = {
      node_ids: ['a'],
      nodes: [{ id: 'a', status: 'online' }],
      missing_node_ids: [],
    };
    expect(missingNodeIds(row)).toEqual([]);
    expect(hasMissingNode(row)).toBe(false);
  });
});

describe('splitNodeSelection', () => {
  it('拆出失效节点并保持顺序 / 去重', () => {
    const { kept, stale } = splitNodeSelection(['x', 'a', 'b', 'a'], ['a', 'b']);
    expect(kept).toEqual(['a', 'b']);
    expect(stale).toEqual(['x']);
  });

  it('空选择返回空结果', () => {
    expect(splitNodeSelection([], ['a'])).toEqual({ kept: [], stale: [] });
  });
});

describe('fallbackHead', () => {
  it('head 仍存在则保留', () => {
    expect(fallbackHead('b', ['a', 'b'])).toBe('b');
  });
  it('head 失效则回落到首个节点', () => {
    expect(fallbackHead('x', ['a', 'b'])).toBe('a');
  });
  it('无有效节点为空串', () => {
    expect(fallbackHead('x', [])).toBe('');
  });
});