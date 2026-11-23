// utils/platform.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  hasPlatforms,
  platformColumnParts,
  platformEntryText,
} from './platform.ts';

test('platformEntryText：os/arch[/variant]，缺失段以 ? 占位', () => {
  assert.equal(platformEntryText({ os: 'linux', architecture: 'arm64' }), 'linux/arm64');
  assert.equal(
    platformEntryText({ os: 'linux', architecture: 'arm64', variant: 'v8' }),
    'linux/arm64/v8',
  );
  assert.equal(platformEntryText({ os: 'linux', architecture: null }), 'linux/?');
  assert.equal(platformEntryText({}), '?/?');
});

test('platformColumnParts：1 条显示平台、多条返回数量、空返回 none', () => {
  assert.deepEqual(platformColumnParts([{ os: 'linux', architecture: 'arm64' }]), {
    kind: 'single',
    value: 'linux/arm64',
  });
  assert.deepEqual(
    platformColumnParts([{ os: 'linux', architecture: 'amd64', variant: 'v2' }]),
    { kind: 'single', value: 'linux/amd64/v2' },
  );
  assert.deepEqual(
    platformColumnParts([
      { os: 'linux', architecture: 'amd64' },
      { os: 'linux', architecture: 'arm64' },
      { os: 'linux', architecture: 'arm/v7' },
      { os: 'linux', architecture: 'riscv64' },
    ]),
    { kind: 'count', value: 4 },
  );
  assert.deepEqual(platformColumnParts([]), { kind: 'none' });
  assert.deepEqual(platformColumnParts(null), { kind: 'none' });
  assert.deepEqual(platformColumnParts(undefined), { kind: 'none' });
});

test('hasPlatforms：仅非空数组为真', () => {
  assert.equal(hasPlatforms([{ os: 'linux', architecture: 'arm64' }]), true);
  assert.equal(hasPlatforms([]), false);
  assert.equal(hasPlatforms(null), false);
  assert.equal(hasPlatforms(undefined), false);
});