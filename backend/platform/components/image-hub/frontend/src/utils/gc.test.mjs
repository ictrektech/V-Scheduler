// utils/gc.ts 纯函数单测（node:test + Node 原生 TS strip）
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  GC_INTERVAL_PRESETS,
  gcHeadlineKey,
  gcIntervalParts,
  gcMarkedCount,
  gcResultCounts,
  isGcTask,
  isValidGcInterval,
  kindKey,
  nextRunKey,
} from './gc.ts';

test('isValidGcInterval 边界（1~720）', () => {
  assert.equal(isValidGcInterval(1), true);
  assert.equal(isValidGcInterval(24), true);
  assert.equal(isValidGcInterval(720), true);
  assert.equal(isValidGcInterval(0), false);
  assert.equal(isValidGcInterval(721), false);
  assert.equal(isValidGcInterval(1.5), false);
  assert.equal(isValidGcInterval(null), false);
  assert.equal(isValidGcInterval(undefined), false);
});

test('gcIntervalParts（天 / 小时 / 非法回落默认）', () => {
  assert.deepEqual(gcIntervalParts(1), { unit: 'hours', value: 1 });
  assert.deepEqual(gcIntervalParts(23), { unit: 'hours', value: 23 });
  assert.deepEqual(gcIntervalParts(24), { unit: 'days', value: 1 });
  assert.deepEqual(gcIntervalParts(72), { unit: 'days', value: 3 });
  assert.deepEqual(gcIntervalParts(0), { unit: 'days', value: 1 }, '非法回落默认 24 小时 = 1 天');
});

test('GC_INTERVAL_PRESETS 均在合法范围且有 i18n key', () => {
  assert.ok(GC_INTERVAL_PRESETS.length > 0);
  for (const preset of GC_INTERVAL_PRESETS) {
    assert.equal(isValidGcInterval(preset.value), true, String(preset.value));
    assert.ok(preset.labelKey.startsWith('gc.intervals.'), preset.labelKey);
  }
});

test('kindKey 覆盖全部任务类型', () => {
  assert.equal(kindKey('pull'), 'tasks.kind.pull');
  assert.equal(kindKey('sync'), 'tasks.kind.sync');
  assert.equal(kindKey('clone'), 'tasks.kind.clone');
  assert.equal(kindKey('rename'), 'tasks.kind.rename');
  assert.equal(kindKey('import'), 'tasks.kind.import');
  assert.equal(kindKey('gc'), 'tasks.kind.gc');
  assert.equal(kindKey(''), 'tasks.kind.task');
  assert.equal(isGcTask('gc'), true);
  assert.equal(isGcTask('pull'), false);
});

test('gcResultCounts / gcHeadlineKey / gcMarkedCount', () => {
  const real = {
    dry_run: false,
    delete_untagged: true,
    blobs_marked: 37,
    blobs_deleted: 16,
    manifests_deleted: 3,
    storage_bytes_before: 100,
    storage_bytes_after: 40,
    freed_bytes: 60,
  };
  assert.deepEqual(gcResultCounts(real), { blobs: 16, manifests: 3, dryRun: false });
  assert.equal(gcHeadlineKey(real), 'gc.result.doneHeadline');
  assert.equal(gcMarkedCount(real), 37);

  const dry = { ...real, dry_run: true, freed_bytes: 0 };
  assert.equal(gcResultCounts(dry).dryRun, true);
  assert.equal(gcHeadlineKey(dry), 'gc.result.dryHeadline');

  // 非 gc 任务 / 无摘要
  assert.equal(gcResultCounts(null), null);
  assert.equal(gcHeadlineKey(undefined), '');
  assert.equal(gcMarkedCount(null), null);
});

test('nextRunKey', () => {
  assert.equal(nextRunKey(null, false), 'gc.nextRunText.disabled');
  assert.equal(nextRunKey('2026-09-25T00:00:00Z', true), 'gc.nextRunText.at');
  assert.equal(nextRunKey(null, true), 'gc.nextRunText.pending');
});