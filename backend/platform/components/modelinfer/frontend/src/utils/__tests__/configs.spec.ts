import { describe, expect, it } from 'vitest';
import { configRows, formatConfigValue } from '../configs';

describe('formatConfigValue', () => {
  it('布尔 → 是 / 否', () => {
    expect(formatConfigValue(true)).toBe('是');
    expect(formatConfigValue(false)).toBe('否');
  });

  it('字符串 / 数字原样', () => {
    expect(formatConfigValue('safetensors')).toBe('safetensors');
    expect(formatConfigValue(4096)).toBe('4096');
    expect(formatConfigValue(0)).toBe('0');
  });

  it('空值 → -', () => {
    expect(formatConfigValue(null)).toBe('-');
    expect(formatConfigValue(undefined)).toBe('-');
  });

  it('对象 / 数组 → JSON', () => {
    expect(formatConfigValue([1, 2])).toBe('[1,2]');
    expect(formatConfigValue({ a: 1 })).toBe('{"a":1}');
  });
});

describe('configRows', () => {
  it('按键排序并格式化值', () => {
    const rows = configRows({ B: true, A: 1, C: null });
    expect(rows.map((r) => r.name)).toEqual(['A', 'B', 'C']);
    expect(rows[0].valueText).toBe('1');
    expect(rows[1].valueText).toBe('是');
    expect(rows[2].valueText).toBe('-');
  });

  it('空 / 非对象容错', () => {
    expect(configRows({})).toEqual([]);
    expect(configRows(null)).toEqual([]);
    expect(configRows(undefined)).toEqual([]);
  });
});