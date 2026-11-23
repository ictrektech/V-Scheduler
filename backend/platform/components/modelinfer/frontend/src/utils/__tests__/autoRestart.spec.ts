import { describe, expect, it } from 'vitest';

import {
  DEFAULT_FAIL_MINUTES,
  MAX_FAIL_MINUTES,
  MIN_FAIL_MINUTES,
  PHASE_IDLE,
  PHASE_STARTING,
  PHASE_STOPPING,
  allVersionsLackHealthProbe,
  clampMinutes,
  hasHealthProbe,
  inRestartLoop,
  minutesToSecs,
  phaseLabelKey,
  secsToMinutes,
} from '../autoRestart';

describe('autoRestart 阈值换算', () => {
  it('默认 5 分钟 = 300 秒', () => {
    expect(DEFAULT_FAIL_MINUTES).toBe(5);
    expect(minutesToSecs(DEFAULT_FAIL_MINUTES)).toBe(300);
    expect(secsToMinutes(300)).toBe(5);
  });

  it('分钟归一到 1~1440', () => {
    expect(clampMinutes(0)).toBe(MIN_FAIL_MINUTES);
    expect(clampMinutes(-10)).toBe(MIN_FAIL_MINUTES);
    expect(clampMinutes(1)).toBe(1);
    expect(clampMinutes(1440)).toBe(MAX_FAIL_MINUTES);
    expect(clampMinutes(2000)).toBe(MAX_FAIL_MINUTES);
    expect(clampMinutes(Number.NaN)).toBe(DEFAULT_FAIL_MINUTES);
    expect(clampMinutes('10' as unknown)).toBe(10);
    expect(clampMinutes(7.6)).toBe(8);
  });

  it('秒 → 分钟向上取整且非法回落默认', () => {
    expect(secsToMinutes(60)).toBe(1);
    expect(secsToMinutes(61)).toBe(2);
    expect(secsToMinutes(90)).toBe(2);
    expect(secsToMinutes(3600)).toBe(60);
    // 低于后端下限 / 非法 → 默认 5 分钟
    expect(secsToMinutes(10)).toBe(DEFAULT_FAIL_MINUTES);
    expect(secsToMinutes(null)).toBe(DEFAULT_FAIL_MINUTES);
    expect(secsToMinutes('abc' as unknown)).toBe(DEFAULT_FAIL_MINUTES);
  });

  it('分钟 → 秒不会越界', () => {
    expect(minutesToSecs(1)).toBe(60);
    expect(minutesToSecs(1440)).toBe(86400);
    expect(minutesToSecs(99999)).toBe(86400);
    expect(minutesToSecs(0)).toBe(60);
  });
});

describe('autoRestart 相位', () => {
  it('只有 stopping / starting 属于循环中', () => {
    expect(inRestartLoop(PHASE_STOPPING)).toBe(true);
    expect(inRestartLoop(PHASE_STARTING)).toBe(true);
    expect(inRestartLoop(PHASE_IDLE)).toBe(false);
    expect(inRestartLoop('weird')).toBe(false);
    expect(inRestartLoop(undefined)).toBe(false);
  });

  it('相位映射到 i18n key', () => {
    expect(phaseLabelKey(PHASE_STOPPING)).toBe('runDetail.overview.autoRestartPhaseStopping');
    expect(phaseLabelKey(PHASE_STARTING)).toBe('runDetail.overview.autoRestartPhaseStarting');
    expect(phaseLabelKey('')).toBe('runDetail.overview.autoRestartPhaseIdle');
  });
});

describe('autoRestart health-probe 判定', () => {
  it('识别 YAML manifest 中的 health-probe', () => {
    const manifest = `id: com.ictrek.x
lifecycle:
  start: ./start.sh
  health-probe: ./health.sh
`;
    expect(hasHealthProbe(manifest)).toBe(true);
  });

  it('未声明 / 空值 / 非字符串视为未声明', () => {
    expect(hasHealthProbe('id: x\nlifecycle:\n  start: ./s.sh\n')).toBe(false);
    expect(hasHealthProbe('lifecycle:\n  health-probe: ""\n')).toBe(false);
    expect(hasHealthProbe('lifecycle:\n  health-probe: 1\n')).toBe(false);
    expect(hasHealthProbe('lifecycle: []')).toBe(false);
    expect(hasHealthProbe('')).toBe(false);
    expect(hasHealthProbe(undefined)).toBe(false);
    expect(hasHealthProbe(':: not yaml ::')).toBe(false);
  });

  it('接受已解析的对象', () => {
    expect(hasHealthProbe({ lifecycle: { 'health-probe': './h.sh' } })).toBe(true);
    expect(hasHealthProbe({ lifecycle: {} })).toBe(false);
  });

  it('全部版本都缺 health-probe 时提示', () => {
    const noProbe = 'lifecycle:\n  start: ./s.sh\n';
    const probe = 'lifecycle:\n  health-probe: ./h.sh\n';
    expect(allVersionsLackHealthProbe([{ manifest_json: noProbe }, { manifest_json: noProbe }])).toBe(true);
    expect(allVersionsLackHealthProbe([{ manifest_json: noProbe }, { manifest_json: probe }])).toBe(false);
    expect(allVersionsLackHealthProbe([])).toBe(false);
    expect(allVersionsLackHealthProbe(undefined)).toBe(false);
  });
});