// roles.ts 纯函数单测（node:test，Node 24 原生 TS strip 运行，无需测试框架）
// 运行：node --test src/utils/roles.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  normalizeRole,
  isManagerRole,
  roleLabel,
  roleColor,
  NODE_ROLES,
} from './roles.ts';

test('normalizeRole：新枚举原样 / 旧值映射 / 未知原样', () => {
  assert.equal(normalizeRole('manager'), 'manager');
  assert.equal(normalizeRole('worker'), 'worker');
  assert.equal(normalizeRole('master'), 'manager');
  assert.equal(normalizeRole('node'), 'worker');
  assert.equal(normalizeRole('weird'), 'weird');
  assert.equal(normalizeRole(undefined), '');
});

test('isManagerRole：manager 与旧值 master 均视为管理节点', () => {
  assert.equal(isManagerRole('manager'), true);
  assert.equal(isManagerRole('master'), true);
  assert.equal(isManagerRole('worker'), false);
  assert.equal(isManagerRole('node'), false);
  assert.equal(isManagerRole(''), false);
});

test('roleLabel：管理节点 / 工作节点 文案', () => {
  assert.equal(roleLabel('manager'), '管理节点');
  assert.equal(roleLabel('worker'), '工作节点');
  assert.equal(roleLabel('master'), '管理节点');
  assert.equal(roleLabel('node'), '工作节点');
  assert.equal(roleLabel(''), '未知');
  assert.equal(roleLabel(undefined), '未知');
});

test('roleColor：manager 绿 / worker 蓝 / 其他默认', () => {
  assert.equal(roleColor('manager'), 'green');
  assert.equal(roleColor('worker'), 'blue');
  assert.equal(roleColor('master'), 'green');
  assert.equal(roleColor('node'), 'blue');
  assert.equal(roleColor('x'), 'default');
});

test('NODE_ROLES 仅含 manager/worker', () => {
  assert.deepEqual([...NODE_ROLES], ['manager', 'worker']);
});