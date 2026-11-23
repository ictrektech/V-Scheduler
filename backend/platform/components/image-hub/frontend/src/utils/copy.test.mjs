// utils/copy.ts 纯函数单测（node:test + Node 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  copyPreview,
  copyVerbKey,
  explicitTag,
  sourceRefParts,
  withoutTag,
} from './copy.ts';

test('explicitTag / withoutTag', () => {
  assert.equal(explicitTag('alpine:3.20'), '3.20');
  assert.equal(explicitTag('docker.io/library/alpine'), null);
  assert.equal(explicitTag('127.0.0.1:5000/foo'), null, '端口不是 tag');
  assert.equal(explicitTag('alpine:'), null, '空 tag 视为无');
  assert.equal(withoutTag('alpine:3.20'), 'alpine');
  assert.equal(withoutTag('127.0.0.1:5000/foo'), '127.0.0.1:5000/foo');
});

test('copyPreview 单 tag：目标标签缺省沿用源标签', () => {
  const p = copyPreview('docker.io/library/alpine', '3.20', 'local/alpine', '');
  assert.equal(p.scope, 'tag');
  assert.equal(p.errorKey, null);
  assert.equal(p.targetRef, 'docker.io/local/alpine:3.20');

  const v = copyPreview('docker.io/library/alpine', '3.20', '', 'stable');
  assert.equal(v.targetRef, 'docker.io/library/alpine:stable');
});

test('copyPreview 单 tag：目标仓库归一（缺省 docker.io / library）', () => {
  const p = copyPreview('docker.io/library/alpine', '3.20', 'alpine-copy', 'v1');
  assert.equal(p.targetRef, 'docker.io/library/alpine-copy:v1');

  const withRegistry = copyPreview('docker.io/library/alpine', '3.20', 'internal.registry/foo', '1');
  assert.equal(withRegistry.targetRef, 'internal.registry/foo:1');
});

test('copyPreview 单 tag：与源相同 / 非法输入返回 i18n key', () => {
  assert.equal(
    copyPreview('docker.io/library/alpine', '3.20', 'docker.io/library/alpine', '3.20').errorKey,
    'copy.errors.sameTarget',
  );
  assert.equal(
    copyPreview('docker.io/library/alpine', '3.20', 'Bad Name', 'x').errorKey,
    'copy.errors.targetInvalid',
  );
  assert.equal(
    copyPreview('docker.io/library/alpine', '3.20', 'local/alpine:1', '').errorKey,
    'copy.errors.targetHasTag',
  );
  assert.equal(
    copyPreview('docker.io/library/alpine', '3.20', 'local/alpine', 'bad tag').errorKey,
    'copy.errors.tagInvalid',
  );
});

test('copyPreview 整仓：要求目标仓库且与源不同、不支持 tag', () => {
  const ok = copyPreview('docker.io/library/alpine', undefined, 'local/alpine', '');
  assert.equal(ok.scope, 'repository');
  assert.equal(ok.errorKey, null);
  assert.equal(ok.targetRef, 'docker.io/local/alpine');

  assert.equal(
    copyPreview('docker.io/library/alpine', undefined, '', '').errorKey,
    'copy.errors.targetRequired',
  );
  assert.equal(
    copyPreview('docker.io/library/alpine', undefined, 'docker.io/library/alpine', '').errorKey,
    'copy.errors.sameRepo',
  );
  assert.equal(
    copyPreview('docker.io/library/alpine', undefined, 'local/alpine', 'v1').errorKey,
    'copy.errors.repoTagNotSupported',
  );
});

test('copyVerbKey / sourceRefParts', () => {
  assert.equal(copyVerbKey('clone'), 'copy.verb.clone');
  assert.equal(copyVerbKey('rename'), 'copy.verb.rename');
  assert.deepEqual(sourceRefParts('docker.io/library/alpine', '3.20'), {
    ref: 'docker.io/library/alpine:3.20',
    allTags: false,
  });
  assert.deepEqual(sourceRefParts('docker.io/library/alpine'), {
    ref: 'docker.io/library/alpine',
    allTags: true,
  });
});