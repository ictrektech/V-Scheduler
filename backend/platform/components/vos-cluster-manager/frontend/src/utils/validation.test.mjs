// validation.ts 校验纯函数单测（node:test + Node 24 TS strip）
// 运行：node --test src/utils/validation.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  isValidIpv4,
  validatePassword,
  isValidClusterName,
  validateNodeRow,
} from './validation.ts';

test('IPv4 校验', () => {
  assert.equal(isValidIpv4('10.100.10.23'), true);
  assert.equal(isValidIpv4('0.0.0.0'), true);
  assert.equal(isValidIpv4('255.255.255.255'), true);
  assert.equal(isValidIpv4('10.100.10'), false);
  assert.equal(isValidIpv4('10.100.10.300'), false);
  assert.equal(isValidIpv4('10.100.10.-1'), false);
  assert.equal(isValidIpv4('a.b.c.d'), false);
  assert.equal(isValidIpv4(''), false);
  assert.equal(isValidIpv4('10.100.010.23'), true); // 允许前导零数值段
});

test('密码校验（≥6）', () => {
  assert.equal(validatePassword('123456'), true);
  assert.equal(validatePassword('abcde'), false);
  assert.equal(validatePassword(''), false);
});

test('集群名称校验（非空）', () => {
  assert.equal(isValidClusterName('demo'), true);
  assert.equal(isValidClusterName('  '), false);
  assert.equal(isValidClusterName(''), false);
});

test('节点行整体校验', () => {
  assert.deepEqual(validateNodeRow('10.0.0.1', 'pw123456'), { ipOk: true, pwdOk: true, ok: true });
  assert.deepEqual(validateNodeRow('bad-ip', 'pw123456'), { ipOk: false, pwdOk: true, ok: false });
  assert.deepEqual(validateNodeRow('10.0.0.1', '123'), { ipOk: true, pwdOk: false, ok: false });
  assert.deepEqual(validateNodeRow('bad-ip', '123'), { ipOk: false, pwdOk: false, ok: false });
});