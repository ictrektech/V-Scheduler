import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyModelTest, upstreamHost } from '../src/gateway/model-test.js';

test('connection diagnosis uses provider evidence and hides URL credentials', () => {
  assert.deepEqual(classifyModelTest(502, { code: 'UPSTREAM_NETWORK_UNREACHABLE' }, false),
    { outcome: 'network', reachability: 'unreachable' });
  assert.deepEqual(classifyModelTest(504, { code: 'UPSTREAM_REQUEST_TIMEOUT' }, false),
    { outcome: 'timeout', reachability: 'unknown' });
  assert.deepEqual(classifyModelTest(403, {}, true), { outcome: 'auth', reachability: 'reachable' });
  assert.deepEqual(classifyModelTest(403, {}, false), { outcome: 'failed', reachability: 'unknown' });
  assert.deepEqual(classifyModelTest(503, { error: { code: 'model_not_loaded' } }, true),
    { outcome: 'unloaded', reachability: 'reachable' });
  assert.deepEqual(classifyModelTest(404, { error: { code: 'model_not_found' } }, true),
    { outcome: 'failed', reachability: 'reachable' });
  assert.equal(upstreamHost('https://user:secret@example.test:8443/v1?key=secret'), 'example.test:8443');
});
