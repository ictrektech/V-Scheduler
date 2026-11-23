import { describe, expect, it } from 'vitest';
import {
  MAX_EXTRA_ENV,
  envKeyError,
  envMapToRows,
  envRowsToMap,
  fromEditableImages,
  imageEntryArch,
  imageEntryLabel,
  imageEntryName,
  imagesOverflow,
  parseWeightOverrideValue,
  toEditableImages,
  validateEnvRows,
  weightOptions,
  weightOverrideValue,
} from '../runtimeOverrides';

describe('runtimeOverrides 镜像条目', () => {
  it('解析字符串与对象条目', () => {
    expect(imageEntryName('repo/a:1')).toBe('repo/a:1');
    expect(imageEntryArch('repo/a:1')).toBe('');
    expect(imageEntryName({ name: 'repo/b:2', arch: 'arm64' })).toBe('repo/b:2');
    expect(imageEntryArch({ name: 'repo/b:2', arch: 'arm64' })).toBe('arm64');
    expect(imageEntryLabel({ name: 'repo/b:2', arch: 'arm64' })).toBe('repo/b:2 (arm64)');
    expect(imageEntryLabel('repo/a:1')).toBe('repo/a:1');
  });

  it('可编辑行与后端条目互转', () => {
    const rows = toEditableImages(['repo/a:1', { name: 'repo/b:2', arch: 'arm64' }, '  ']);
    expect(rows).toEqual([
      { name: 'repo/a:1', arch: '' },
      { name: 'repo/b:2', arch: 'arm64' },
    ]);
    expect(fromEditableImages(rows)).toEqual(['repo/a:1', { name: 'repo/b:2', arch: 'arm64' }]);
    // 空名称行被丢弃；arch 空写字符串形式
    expect(
      fromEditableImages([
        { name: '  ', arch: 'amd64' },
        { name: ' repo/c:3 ', arch: '' },
      ]),
    ).toEqual(['repo/c:3']);
    expect(fromEditableImages(undefined)).toEqual([]);
    expect(imagesOverflow(Array.from({ length: 33 }, (_, i) => ({ name: `r/x${i}:1`, arch: '' })))).toBe(
      true,
    );
  });
});

describe('runtimeOverrides 权重覆盖', () => {
  it('覆盖值与下拉值互转', () => {
    expect(weightOverrideValue(null)).toBe('');
    expect(weightOverrideValue({ source: 'ms', model_id: 'org/w' })).toBe('ms:org/w');
    expect(parseWeightOverrideValue('ms:org/w')).toEqual({ source: 'ms', model_id: 'org/w' });
    expect(parseWeightOverrideValue('')).toBeNull();
    expect(parseWeightOverrideValue('bad')).toBeNull();
    expect(parseWeightOverrideValue(':x')).toBeNull();
  });

  it('变体按 source:model_id 去重并排序', () => {
    const options = weightOptions([
      { source: 'hf', model_id: 'org/b', rel_path: '', abs_path: '' },
      { source: 'ms', model_id: 'org/a', rel_path: '', abs_path: '' },
      { source: 'hf', model_id: 'org/b', rel_path: '', abs_path: '' },
      { source: '', model_id: 'org/c', rel_path: '', abs_path: '' },
    ]);
    expect(options.map((o) => o.value)).toEqual(['hf:org/b', 'ms:org/a']);
  });
});

describe('runtimeOverrides 额外环境变量', () => {
  it('键名规则与后端一致', () => {
    expect(envKeyError('DEBUG_MODE')).toBeNull();
    expect(envKeyError('')).toBe('empty');
    expect(envKeyError('1BAD')).toBe('invalid');
    expect(envKeyError('A-B')).toBe('invalid');
    expect(envKeyError('MODELINFER_SERVE_PORT')).toBe('reserved');
    expect(envKeyError('PATH')).toBe('system');
    expect(envKeyError('MAX', ['MAX'])).toBe('conflict');
    expect(envKeyError('A', [], new Set(['A']))).toBe('duplicate');
  });

  it('行批量校验与对象互转', () => {
    const rows = [
      { key: 'GOOD', value: '1' },
      { key: '', value: '' },
      { key: 'PATH', value: '/tmp' },
      { key: 'GOOD', value: '2' },
    ];
    const errors = validateEnvRows(rows, ['CFG']);
    expect(errors.get(0)).toBeUndefined();
    expect(errors.get(1)).toBeUndefined();
    expect(errors.get(2)).toBe('system');
    expect(errors.get(3)).toBe('duplicate');

    expect(envRowsToMap(rows)).toEqual({ GOOD: '1', PATH: '/tmp' });
    expect(envMapToRows({ B: '2', A: '1' })).toEqual([
      { key: 'A', value: '1' },
      { key: 'B', value: '2' },
    ]);
    expect(envMapToRows(undefined)).toEqual([]);

    const tooMany = Array.from({ length: MAX_EXTRA_ENV + 1 }, (_, i) => ({ key: `K${i}`, value: '' }));
    expect(validateEnvRows(tooMany).get(0)).toBe('overflow');
  });
});