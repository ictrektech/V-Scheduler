import { describe, expect, it } from 'vitest';
import {
  formatBytes,
  formatDuration,
  formatFullTime,
  formatRelativeTime,
  formatSeconds,
  formatTime,
  formatTimeTooltip,
  summarizeDetail,
} from '../format';

describe('formatBytes', () => {
  it('按 IEC 单位换算', () => {
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(1024)).toBe('1.0 KiB');
    expect(formatBytes(13 * 1024)).toBe('13.0 KiB');
    expect(formatBytes(1024 ** 3)).toBe('1.0 GiB');
    expect(formatBytes(1024 ** 4)).toBe('1.0 TiB');
  });

  it('空值返回 -', () => {
    expect(formatBytes(null)).toBe('-');
    expect(formatBytes(undefined)).toBe('-');
    expect(formatBytes(NaN)).toBe('-');
  });
});

describe('时间格式化', () => {
  const ns = '2026-09-22T21:56:34.585257561+08:00';

  it('兼容带纳秒的 RFC3339', () => {
    expect(formatTime(ns)).toBe('2026-09-22 21:56');
    expect(formatFullTime(ns)).toBe('2026-09-22 21:56:34');
    expect(formatTimeTooltip(ns)).toContain('2026-09-22 21:56:34');
    expect(formatTimeTooltip(ns)).toContain('前');
  });

  it('相对时间含"前"', () => {
    expect(formatRelativeTime(ns)).toContain('前');
  });

  it('空值返回 -；非法串原样回显', () => {
    expect(formatTime(null)).toBe('-');
    expect(formatTime('')).toBe('-');
    expect(formatTime('not-a-date')).toBe('not-a-date');
  });
});

describe('时长格式化', () => {
  it('formatSeconds 分段', () => {
    expect(formatSeconds(0)).toBe('0s');
    expect(formatSeconds(45)).toBe('45s');
    expect(formatSeconds(200)).toBe('3m20s');
    expect(formatSeconds(180)).toBe('3m');
    expect(formatSeconds(3900)).toBe('1h5m');
    expect(formatSeconds(7200)).toBe('2h');
    expect(formatSeconds(90000)).toBe('1d1h');
    expect(formatSeconds(-1)).toBe('-');
    expect(formatSeconds(Number.NaN)).toBe('-');
  });

  it('formatDuration 由起止时间计算', () => {
    expect(formatDuration('2026-09-25T12:00:00+08:00', '2026-09-25T12:03:20+08:00')).toBe('3m20s');
    expect(formatDuration(null, '2026-09-25T12:03:20+08:00')).toBe('-');
  });
});

describe('summarizeDetail', () => {
  it('取顶层标量拼接单行', () => {
    expect(summarizeDetail({ nodes: 4, image_count: 1 })).toBe('nodes=4 · image_count=1');
    expect(summarizeDetail({ host: '10.10.11.154' })).toBe('host=10.10.11.154');
  });

  it('数组 / 对象只展示大小', () => {
    expect(summarizeDetail({ stages: ['a', 'b', 'c'], meta: { x: 1 } })).toBe('stages[3] · meta{1}');
  });

  it('跳过空值并限制条数', () => {
    expect(summarizeDetail({ a: null, b: '', c: 1 })).toBe('c=1');
    expect(summarizeDetail({ a: 1, b: 2, c: 3, d: 4, e: 5 }, 3)).toBe('a=1 · b=2 · c=3');
  });

  it('容错：非对象 / 空值返回空串', () => {
    expect(summarizeDetail(null)).toBe('');
    expect(summarizeDetail(undefined)).toBe('');
    expect(summarizeDetail('text')).toBe('');
    expect(summarizeDetail(42)).toBe('');
    expect(summarizeDetail({})).toBe('');
    expect(summarizeDetail([])).toBe('');
  });
});