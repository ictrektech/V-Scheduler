// utils/ip.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { normalizeHost, isLocalIp, sortDeviceIpList, addressListOrFallback } from './ip.ts';

test('normalizeHost：去端口 / 去方括号 / 小写', () => {
  assert.equal(normalizeHost('192.168.1.100:1180'), '192.168.1.100');
  assert.equal(normalizeHost('[::1]:5666'), '::1');
  assert.equal(normalizeHost('NAS.Local:5666'), 'nas.local');
  assert.equal(normalizeHost('10.0.0.1'), '10.0.0.1');
});

test('isLocalIp：回环 / docker 网段', () => {
  assert.equal(isLocalIp('127.0.0.1'), true);
  assert.equal(isLocalIp('::1'), true);
  assert.equal(isLocalIp('172.17.0.1'), true);
  assert.equal(isLocalIp('172.31.255.254'), true);
  assert.equal(isLocalIp('172.32.0.1'), false);
  assert.equal(isLocalIp('10.100.10.21'), false);
});

test('sortDeviceIpList：当前主机优先，本地地址末尾', () => {
  const list = ['172.17.0.1', '10.100.10.22', '10.100.10.21', '127.0.0.1'];
  const sorted = sortDeviceIpList(list, '10.100.10.21');
  assert.equal(sorted[0], '10.100.10.21');
  assert.equal(sorted[1], '10.100.10.22');
  // 本地地址排在可路由地址之后（同优先级保持原相对顺序，稳定排序）
  assert.deepEqual(sorted.slice(2), ['172.17.0.1', '127.0.0.1']);
});

test('addressListOrFallback：空列表兜底当前主机', () => {
  assert.deepEqual(addressListOrFallback([], '10.100.10.21'), ['10.100.10.21']);
  assert.deepEqual(addressListOrFallback([], ''), []);
  assert.deepEqual(addressListOrFallback(['10.0.0.5'], '10.0.0.5'), ['10.0.0.5']);
});