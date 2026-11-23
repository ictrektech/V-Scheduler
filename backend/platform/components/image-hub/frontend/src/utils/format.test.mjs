// utils/format.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import { formatBytes } from './format.ts';

test('formatBytes：IEC 1024 进制与自动单位', () => {
  assert.equal(formatBytes(null), '-');
  assert.equal(formatBytes(undefined), '-');
  assert.equal(formatBytes(512), '512 B');
  assert.equal(formatBytes(1024), '1.00 KiB');
  assert.equal(formatBytes(1024 * 1024), '1.00 MiB');
  assert.equal(formatBytes(1024 ** 3), '1.00 GiB');
  assert.equal(formatBytes(1024 ** 4), '1.00 TiB');
  // 大数值降精度
  assert.equal(formatBytes(150 * 1024 ** 2), '150 MiB');
});