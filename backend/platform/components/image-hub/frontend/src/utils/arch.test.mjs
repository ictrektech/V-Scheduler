// utils/arch.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  ARCH_OPTIONS,
  DEFAULT_SPECIFIC_ARCHS,
  archRequestValue,
  formatArch,
  normalizeArchList,
} from './arch.ts';

test('ARCH_OPTIONS：候选为 docker 标准完整平台名（不含 variant 形式）', () => {
  assert.ok(ARCH_OPTIONS.includes('linux/amd64'));
  assert.ok(ARCH_OPTIONS.includes('linux/arm64'));
  assert.ok(ARCH_OPTIONS.includes('linux/arm'));
  assert.ok(ARCH_OPTIONS.every((a) => a.startsWith('linux/')), '候选应为完整平台名');
  assert.ok(!ARCH_OPTIONS.some((a) => a.split('/').length > 2), '不应含 variant 形式');
});

test('DEFAULT_SPECIFIC_ARCHS：默认预选 linux/amd64 + linux/arm64', () => {
  assert.deepEqual([...DEFAULT_SPECIFIC_ARCHS], ['linux/amd64', 'linux/arm64']);
});

test('normalizeArchList：去空白 / 去空项 / 去重保序', () => {
  assert.deepEqual(
    normalizeArchList([' linux/amd64 ', '', 'linux/arm64', 'linux/amd64']),
    ['linux/amd64', 'linux/arm64'],
  );
  assert.deepEqual(normalizeArchList([]), []);
});

test('archRequestValue：all / host 直通，specific 逗号拼接', () => {
  assert.equal(archRequestValue('all', []), 'all');
  assert.equal(archRequestValue('host', []), 'host');
  assert.equal(
    archRequestValue('specific', ['linux/amd64', 'linux/arm64']),
    'linux/amd64,linux/arm64',
  );
  assert.equal(
    archRequestValue('specific', ['linux/arm64', ' linux/amd64 ', 'linux/arm64']),
    'linux/arm64,linux/amd64',
  );
  assert.equal(archRequestValue('specific', []), '');
});

test('formatArch：全部 / 本地 / 列表 / 空（结构化值，组件层翻译）', () => {
  assert.deepEqual(formatArch('all'), { kind: 'all' });
  assert.deepEqual(formatArch('host'), { kind: 'host' });
  assert.deepEqual(formatArch('system'), { kind: 'host' });
  assert.deepEqual(formatArch('linux/amd64,linux/arm64'), {
    kind: 'list',
    value: 'linux/amd64,linux/arm64',
  });
  assert.deepEqual(formatArch(''), { kind: 'none' });
  assert.deepEqual(formatArch(null), { kind: 'none' });
});