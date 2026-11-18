import test from 'node:test';
import assert from 'node:assert/strict';
import { classifyModelTest, executionLocation, upstreamHost } from '../src/gateway/model-test.js';

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

test('browser reload restores only the last test for the gateway identity and model', async () => {
  const values = new Map();
  const original = globalThis.window;
  globalThis.window = {
    localStorage: {
      getItem: key => values.get(key),
      setItem: (key, value) => values.set(key, value),
    },
    dispatchEvent() {},
  };
  try {
    const module = await import('../src/gateway/model-test.js?before-reload');
    const auth = sub => ({auth_method:'vos_oidc', identity:{sub}});
    const scope = module.modelTestScope('/api/scheduler', auth('a'));
    const url = 'https://example.test:8000/v1?token=secret';
    module.rememberModelTest(scope, 'm112', 'gpt-oss:20b', {outcome:'failed', reachability:'unknown'}, url);
    module.rememberModelTest(scope, 'm112', 'gpt-oss:20b', {outcome:'passed', reachability:'reachable'}, url);
    const afterReload = await import('../src/gateway/model-test.js?after-reload');
    const result = afterReload.lastModelTest(scope, 'm112', 'gpt-oss:20b');
    assert.equal(result.outcome, 'passed');
    assert.equal(result.host, 'example.test:8000');
    assert.ok(Number.isFinite(Date.parse(result.at)));
    assert.equal(afterReload.lastModelTest(module.modelTestScope('/api/scheduler', auth('b')), 'm112', 'gpt-oss:20b'), undefined);
    assert.equal(afterReload.lastModelTest(scope, 'different-upstream', 'gpt-oss:20b'), undefined);
    assert.ok(!JSON.stringify([...values]).includes('secret'));
  } finally {
    globalThis.window = original;
  }
});

test('execution location distinguishes built-in Ollama, an endpoint, and missing information', () => {
  assert.deepEqual(executionLocation({id:'v-scheduler/ollama-qa', source:'v-scheduler'}, 'vos.test'),
    {label:'本机 VOS', host:'vos.test'});
  assert.deepEqual(executionLocation({id:'m112', base_url:'http://m112.test:8000/v1'}, 'vos.test'),
    {label:'上游主机', host:'m112.test:8000'});
  assert.deepEqual(executionLocation(undefined, 'vos.test'), {label:'执行位置未确认', host:''});
});
