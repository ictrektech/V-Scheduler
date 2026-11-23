// utils/image.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  parseImageRef,
  normalizeImageRef,
  upstreamRepository,
  upstreamReference,
  localImageRef,
  isValidTag,
  isValidRepository,
} from './image.ts';

test('parseImageRef：显式 registry 与 tag', () => {
  const ref = parseImageRef('docker.m.daocloud.io/library/alpine:3.20');
  assert.deepEqual(ref, {
    registry: 'docker.m.daocloud.io',
    repository: 'library/alpine',
    tag: '3.20',
  });
});

test('parseImageRef：缺省 tag / registry / namespace 归一', () => {
  assert.deepEqual(parseImageRef('docker.io/library/ubuntu'), {
    registry: 'docker.io',
    repository: 'library/ubuntu',
    tag: 'latest',
  });
  assert.deepEqual(parseImageRef('ubuntu:22.04'), {
    registry: 'docker.io',
    repository: 'library/ubuntu',
    tag: '22.04',
  });
  assert.deepEqual(parseImageRef('ictrek/image-hub:1.0'), {
    registry: 'docker.io',
    repository: 'ictrek/image-hub',
    tag: '1.0',
  });
});

test('parseImageRef：带端口的 registry host', () => {
  assert.deepEqual(parseImageRef('127.0.0.1:5000/foo/bar:1.0'), {
    registry: '127.0.0.1:5000',
    repository: 'foo/bar',
    tag: '1.0',
  });
});

test('parseImageRef：非法输入返回 null', () => {
  assert.equal(parseImageRef(''), null);
  assert.equal(parseImageRef('Bad Image!!'), null);
  assert.equal(parseImageRef('docker.io/library/ubuntu@sha256:abc'), null);
  assert.equal(parseImageRef('foo//bar'), null);
  assert.equal(parseImageRef('foo:'), null);
});

test('上游完整名与本地目标引用保留上游身份', () => {
  const ref = parseImageRef('docker.io/library/ubuntu:22.04');
  assert.equal(upstreamRepository(ref), 'docker.io/library/ubuntu');
  assert.equal(upstreamReference(ref), 'docker.io/library/ubuntu:22.04');
  assert.equal(
    localImageRef('127.0.0.1:19913', ref),
    '127.0.0.1:19913/docker.io/library/ubuntu:22.04',
  );
});

test('normalizeImageRef 与校验辅助函数', () => {
  assert.equal(normalizeImageRef('alpine'), 'docker.io/library/alpine:latest');
  assert.equal(normalizeImageRef('Bad Image'), null);
  assert.equal(isValidTag('v1.2.3-rc1'), true);
  assert.equal(isValidTag('bad tag'), false);
  assert.equal(isValidRepository('docker.io/library/ubuntu'), true);
  assert.equal(isValidRepository('Foo/Bar'), false);
});