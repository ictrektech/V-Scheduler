// utils/mirror.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  filterMirrorsByRegistry,
  isValidMirrorHost,
  normalizeRegistry,
  resolveMirrorSourceRef,
} from './mirror.ts';
import { parseImageRef } from './image.ts';

const CATALOG = [
  { upstream_registry: 'docker.io', host: 'docker.m.daocloud.io', name: 'DaoCloud' },
  { upstream_registry: 'docker.io', host: 'docker.1panel.live', name: '1Panel' },
  { upstream_registry: 'ghcr.io', host: 'ghcr.m.daocloud.io', name: 'DaoCloud' },
  { upstream_registry: 'registry.k8s.io', host: 'k8s.m.daocloud.io', name: 'DaoCloud' },
];

test('normalizeRegistry：去空白 / 去尾斜杠 / 小写', () => {
  assert.equal(normalizeRegistry('  Docker.M.Daocloud.IO/ '), 'docker.m.daocloud.io');
  assert.equal(normalizeRegistry(null), '');
  assert.equal(normalizeRegistry(undefined), '');
});

test('filterMirrorsByRegistry：仅返回该上游候选', () => {
  const docker = filterMirrorsByRegistry(CATALOG, 'docker.io');
  assert.deepEqual(
    docker.map((m) => m.host),
    ['docker.m.daocloud.io', 'docker.1panel.live'],
  );
  assert.deepEqual(
    filterMirrorsByRegistry(CATALOG, 'ghcr.io').map((m) => m.host),
    ['ghcr.m.daocloud.io'],
  );
  // 大小写无关
  assert.equal(filterMirrorsByRegistry(CATALOG, 'DOCKER.IO').length, 2);
  // 未收录 / 空
  assert.deepEqual(filterMirrorsByRegistry(CATALOG, 'registry.example.com'), []);
  assert.deepEqual(filterMirrorsByRegistry(CATALOG, ''), []);
  assert.deepEqual(filterMirrorsByRegistry(null, 'docker.io'), []);
});

test('isValidMirrorHost：与后端 validate_registry_host 字符集一致', () => {
  assert.ok(isValidMirrorHost('docker.m.daocloud.io'));
  assert.ok(isValidMirrorHost('harbor.internal:5000'));
  assert.ok(isValidMirrorHost('  Docker.M.Daocloud.IO '));
  assert.ok(!isValidMirrorHost(''));
  assert.ok(!isValidMirrorHost('   '));
  assert.ok(!isValidMirrorHost('http://bad/mirror'));
  assert.ok(!isValidMirrorHost('docker.io/foo'));
});

test('resolveMirrorSourceRef：镜像源只改来源不改落地名', () => {
  const ubuntu = parseImageRef('ubuntu:22.04');
  assert.ok(ubuntu);
  assert.equal(
    resolveMirrorSourceRef('docker.m.daocloud.io', ubuntu),
    'docker.m.daocloud.io/library/ubuntu:22.04',
  );
  // 不使用镜像源 → 上游原始引用
  assert.equal(resolveMirrorSourceRef(null, ubuntu), 'docker.io/library/ubuntu:22.04');
  assert.equal(resolveMirrorSourceRef(undefined, ubuntu), 'docker.io/library/ubuntu:22.04');
  assert.equal(resolveMirrorSourceRef('  ', ubuntu), 'docker.io/library/ubuntu:22.04');
  // 归一化后再拼
  assert.equal(
    resolveMirrorSourceRef(' Docker.M.Daocloud.IO/ ', ubuntu),
    'docker.m.daocloud.io/library/ubuntu:22.04',
  );

  const ghcr = parseImageRef('ghcr.io/foo/bar:1.0');
  assert.ok(ghcr);
  assert.equal(
    resolveMirrorSourceRef('ghcr.m.daocloud.io', ghcr),
    'ghcr.m.daocloud.io/foo/bar:1.0',
  );
});