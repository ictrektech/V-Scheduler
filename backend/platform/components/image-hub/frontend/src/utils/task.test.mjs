// utils/task.ts 纯函数单测（node:test + Node 24 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_RETRY_TIMES,
  DEFAULT_TIMEOUT_MINUTES,
  MAX_RETRY_TIMES,
  MIN_RETRY_TIMES,
  MIN_TIMEOUT_MINUTES,
  RETRY_PRESETS,
  TIMEOUT_PRESETS,
  TIMEOUT_UNITS,
  UNLIMITED_TIMEOUT_MINUTES,
  isStopping,
  isValidRetryTimes,
  isValidTimeoutMinutes,
  phaseKey,
  progressParts,
  retryBackoffSecs,
  retryHintParts,
  splitTimeout,
  statusColor,
  statusKey,
  timeoutParts,
} from './task.ts';

const detail = (over) => ({
  phase: 'copying',
  message: '正在复制镜像层',
  percent: null,
  items_done: 0,
  items_total: 1,
  sub_current: null,
  sub_total: null,
  layers_done: 0,
  attempt: 0,
  max_attempts: 1,
  retry_in_secs: null,
  ...over,
});

test('statusKey / statusColor 覆盖全部状态（含 cancelled）', () => {
  assert.equal(statusKey('pending'), 'tasks.status.pending');
  assert.equal(statusKey('running'), 'tasks.status.running');
  assert.equal(statusKey('succeeded'), 'tasks.status.succeeded');
  assert.equal(statusKey('failed'), 'tasks.status.failed');
  assert.equal(statusKey('cancelled'), 'tasks.status.cancelled');

  assert.equal(statusColor('running'), 'processing');
  assert.equal(statusColor('succeeded'), 'success');
  assert.equal(statusColor('failed'), 'error');
  assert.equal(statusColor('cancelled'), 'default');
});

test('phaseKey 覆盖后端阶段常量', () => {
  assert.equal(phaseKey('queued'), 'tasks.phase.queued');
  assert.equal(phaseKey('login'), 'tasks.phase.login');
  assert.equal(phaseKey('preparing'), 'tasks.phase.preparing');
  assert.equal(phaseKey('copying'), 'tasks.phase.copying');
  assert.equal(phaseKey('writing'), 'tasks.phase.writing');
  assert.equal(phaseKey('gc'), 'tasks.phase.gc');
  assert.equal(phaseKey('done'), 'tasks.phase.done');
  assert.equal(phaseKey('cancelled'), 'tasks.phase.cancelled');
  assert.equal(phaseKey('failed'), 'tasks.phase.failed');
  assert.equal(phaseKey('unknown-phase'), 'tasks.phase.running');
});

test('progressParts 仅返回有值的计数（供组件层组合本地化文案）', () => {
  assert.deepEqual(progressParts(null), {
    attempt: null,
    retryInSecs: null,
    items: null,
    sub: null,
    layers: 0,
  });
  assert.deepEqual(progressParts(detail({ layers_done: 3 })).layers, 3);
  assert.deepEqual(progressParts(detail({ items_total: 3, items_done: 1 })).items, {
    done: 1,
    total: 3,
  });
  assert.deepEqual(progressParts(detail({ sub_current: 4, sub_total: 12 })).sub, {
    current: 4,
    total: 12,
  });
  assert.deepEqual(progressParts(detail({ attempt: 2, max_attempts: 3 })).attempt, {
    current: 2,
    max: 3,
  });
  assert.equal(progressParts(detail({ retry_in_secs: 5 })).retryInSecs, 5);
  assert.equal(progressParts(detail({ retry_in_secs: 0 })).retryInSecs, null);
  const all = progressParts(
    detail({
      items_total: 3,
      items_done: 2,
      sub_current: 4,
      sub_total: 12,
      layers_done: 7,
      attempt: 2,
      max_attempts: 3,
      retry_in_secs: 10,
    }),
  );
  assert.deepEqual(all, {
    attempt: { current: 2, max: 3 },
    retryInSecs: 10,
    items: { done: 2, total: 3 },
    sub: { current: 4, total: 12 },
    layers: 7,
  });
});

test('isStopping：运行中且已请求停止', () => {
  assert.equal(isStopping('running', true), true);
  assert.equal(isStopping('running', false), false);
  assert.equal(isStopping('cancelled', true), false);
  assert.equal(isStopping('succeeded', true), false);
});

test('isValidTimeoutMinutes：0 为无限制，无上限', () => {
  assert.equal(MIN_TIMEOUT_MINUTES, 1);
  assert.equal(UNLIMITED_TIMEOUT_MINUTES, 0);
  assert.equal(DEFAULT_TIMEOUT_MINUTES, 60);
  const presetValues = TIMEOUT_PRESETS.map((p) => p.value);
  assert.ok(presetValues.includes(60));
  assert.ok(presetValues.includes(UNLIMITED_TIMEOUT_MINUTES), '预设含「无限制」');
  assert.ok(TIMEOUT_PRESETS.every((p) => isValidTimeoutMinutes(p.value)));
  assert.ok(TIMEOUT_PRESETS.every((p) => p.labelKey.startsWith('timeout.presets.')));
  assert.ok(TIMEOUT_UNITS.every((u) => u.labelKey.startsWith('timeout.unit')));

  assert.equal(isValidTimeoutMinutes(1), true);
  assert.equal(isValidTimeoutMinutes(60), true);
  assert.equal(isValidTimeoutMinutes(1440), true);
  assert.equal(isValidTimeoutMinutes(100000), true, '无上限');
  assert.equal(isValidTimeoutMinutes(0), true, '0 = 无限制合法');
  assert.equal(isValidTimeoutMinutes(60.5), false);
  assert.equal(isValidTimeoutMinutes(null), false);
  assert.equal(isValidTimeoutMinutes(undefined), false);
});

test('isValidRetryTimes 与后端 1~5 校验一致', () => {
  assert.equal(MIN_RETRY_TIMES, 1);
  assert.equal(MAX_RETRY_TIMES, 5);
  assert.equal(DEFAULT_RETRY_TIMES, 2);
  assert.equal(RETRY_PRESETS.length, 5);
  assert.ok(RETRY_PRESETS.every((p) => isValidRetryTimes(p.value)));
  assert.ok(RETRY_PRESETS.every((p) => p.labelKey.startsWith('retry.count')));
  assert.equal(isValidRetryTimes(1), true);
  assert.equal(isValidRetryTimes(5), true);
  assert.equal(isValidRetryTimes(0), false);
  assert.equal(isValidRetryTimes(6), false);
  assert.equal(isValidRetryTimes(null), false);
});

test('retryBackoffSecs 与后端退避序列一致（5/10/20/40/60 封顶）', () => {
  assert.deepEqual([1, 2, 3, 4, 5, 6].map(retryBackoffSecs), [5, 10, 20, 40, 60, 60]);
});

test('retryHintParts：尝试次数与退避序列（供 i18n 插值）', () => {
  assert.deepEqual(retryHintParts(2), { attempts: 3, backoffs: '5s / 10s' });
  assert.deepEqual(retryHintParts(null), { attempts: 3, backoffs: '5s / 10s' });
  assert.deepEqual(retryHintParts(1), { attempts: 2, backoffs: '5s' });
});

test('timeoutParts：无限制 / 小时 / 分钟', () => {
  assert.deepEqual(timeoutParts(720), { kind: 'hours', value: 12 });
  assert.deepEqual(timeoutParts(60), { kind: 'hours', value: 1 });
  assert.deepEqual(timeoutParts(30), { kind: 'minutes', value: 30 });
  assert.deepEqual(timeoutParts(UNLIMITED_TIMEOUT_MINUTES), { kind: 'unlimited' });
  assert.deepEqual(timeoutParts(480), { kind: 'hours', value: 8 });
  assert.deepEqual(timeoutParts(null), { kind: 'hours', value: 1 });
});

test('splitTimeout：整小时拆分，其余按分钟', () => {
  assert.deepEqual(splitTimeout(720), { value: 12, unit: 60 });
  assert.deepEqual(splitTimeout(60), { value: 1, unit: 60 });
  assert.deepEqual(splitTimeout(90), { value: 90, unit: 1 });
});