// createWizard.ts 向导纯函数单测（node:test + Node 24 TS strip）
// 运行：node --test src/utils/createWizard.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  createWizardState,
  validateClusterInfo,
  validateNodeRowField,
  validateNodeRows,
  buildCreateInput,
  firstRowRoleError,
} from './createWizard.ts';

test('初始状态（1 空行 + 空 cluster_name + 首行管理节点）', () => {
  const s = createWizardState();
  assert.equal(s.cluster_name, '');
  assert.equal(s.nodes.length, 1);
  assert.equal(s.nodes[0].node_ip, '');
  assert.equal(s.nodes[0].role, 'manager', '发起节点（首行）固定为管理节点');
});

test('集群信息校验', () => {
  assert.equal(validateClusterInfo('demo'), '');
  assert.equal(validateClusterInfo('  '), '集群名称必填');
  assert.equal(validateClusterInfo(''), '集群名称必填');
});

test('单行校验', () => {
  const row = { node_ip: '10.0.0.1', admin_password: 'pw123456', display_name: '', role: 'manager' };
  assert.equal(validateNodeRowField(row), '');
  assert.equal(validateNodeRowField({ ...row, node_ip: 'bad' }), '节点 IP 非法');
  assert.equal(validateNodeRowField({ ...row, admin_password: '123' }), '管理员密码需 ≥6 位');
});

test('整体校验（多行 + 空列表）', () => {
  const rows = [
    { node_ip: '10.0.0.1', admin_password: 'pw123456', display_name: '', role: 'manager' },
    { node_ip: 'bad', admin_password: 'pw123456', display_name: '', role: 'worker' },
  ];
  const r = validateNodeRows(rows);
  assert.equal(r.ok, false);
  assert.equal(r.errors[1], '节点 IP 非法');
  assert.equal(r.errors[0], undefined);

  const ok = validateNodeRows([rows[0]]);
  assert.equal(ok.ok, true);
  assert.deepEqual(ok.errors, {});

  const empty = validateNodeRows([]);
  assert.equal(empty.ok, false);
  assert.equal(empty.errors[0], '至少录入 1 个节点');
});

test('发起节点角色守卫：首行 worker 报错 / manager 通过', () => {
  const good = createWizardState();
  assert.equal(firstRowRoleError(good), '');
  const bad = { cluster_name: '', nodes: [{ node_ip: '10.0.0.1', admin_password: 'pw123456', display_name: '', role: 'worker' }] };
  assert.equal(firstRowRoleError(bad), '发起节点（首行）必须为管理节点');
  const badLegacy = { cluster_name: '', nodes: [{ node_ip: '10.0.0.1', admin_password: 'pw123456', display_name: '', role: 'node' }] };
  // 旧值 node 归一化为 worker → 视为非管理节点，触发守卫
  assert.equal(firstRowRoleError(badLegacy), '发起节点（首行）必须为管理节点');
});

test('构建提交体（display_name 回退/role 归一化/trim）', () => {
  const input = buildCreateInput({
    cluster_name: ' demo ',
    nodes: [
      { node_ip: ' 10.0.0.1 ', admin_password: 'pw123456', display_name: ' ', role: 'manager' },
      { node_ip: '10.0.0.2', admin_password: 'pw123456', display_name: 'node-b', role: 'worker' },
    ],
  });
  assert.equal(input.cluster_name, 'demo');
  assert.equal(input.nodes.length, 2);
  assert.equal(input.nodes[0].node_ip, '10.0.0.1');
  assert.equal(input.nodes[0].display_name, undefined);
  assert.equal(input.nodes[0].role, 'manager');
  assert.equal(input.nodes[1].display_name, 'node-b');
  assert.equal(input.nodes[1].role, 'worker');
});