import test from 'node:test';
import assert from 'node:assert/strict';
import { speedSeries } from '../src/gateway/model-speed.js';

test('speed chart uses completion times and does not connect across missing TPS', () => {
  const points = [
    {time:'2026-11-11T08:00:00Z', tokens_per_second:12, ttft_ms:24},
    {time:'2026-11-11T08:00:01Z', tokens_per_second:null, ttft_ms:15},
    {time:'2026-11-11T08:00:10Z', tokens_per_second:18, ttft_ms:30},
  ];
  const speed = speedSeries(points, 'tokens_per_second');
  const wait = speedSeries(points, 'ttft_ms');
  assert.deepEqual(speed.segments.map(s => s.map(d => d.index)), [[0], [2]]);
  assert.equal(wait.segments.length, 1);
  assert.ok(Number(wait.dots[1].x) < 100);
  assert.equal(wait.max, 30);
});
