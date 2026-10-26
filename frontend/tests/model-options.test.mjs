import {test} from 'node:test';
import assert from 'node:assert/strict';
import {CAPABILITIES, ENDPOINTS, PROVIDER_PRESETS, effectiveMetadata, systemOneRequest} from '../src/gateway/model-options.js';

test('provider presets have unique identities and endpoints, and never invent model IDs', () => {
  assert.equal(new Set(PROVIDER_PRESETS.map(p => p.id)).size, PROVIDER_PRESETS.length);
  assert.equal(new Set(PROVIDER_PRESETS.map(p => p.base)).size, PROVIDER_PRESETS.length);
  for (const p of PROVIDER_PRESETS) {
    assert.equal(new URL(p.base).protocol, 'https:');
    assert.ok(['openai', 'chat_completions', 'anthropic', 'typesafe', 'ollama'].includes(p.protocol));
    assert.ok(!p.model || p.id === 'typesafe');
  }
  for (const options of [CAPABILITIES, ENDPOINTS]) assert.equal(new Set(options.map(([id]) => id)).size, options.length);
});
test('unknown chat models receive a usable endpoint without claiming tools or vision', () => {
  const model = effectiveMetadata({kind: 'unknown'});
  assert.equal(model.kind, 'chat');
  assert.deepEqual(model.endpoints, ['chat_completions']);
  assert.deepEqual(model.capabilities, ['completion', 'chat']);
  assert.deepEqual(effectiveMetadata({}, 'anthropic').endpoints, ['messages']);
  assert.ok(effectiveMetadata({capabilities:['vision']}).capabilities.includes('vision'));
});
test('specialized models and explicit endpoint restrictions are preserved', () => {
  for (const [cap, endpoint] of [['embedding','embeddings'],['rerank','rerank'],['image_generation','images/generations'],['asr','audio/transcriptions'],['decision','systemone']]) {
    const model = effectiveMetadata({capabilities:[cap]});
    assert.deepEqual(model.endpoints, [endpoint]);
    assert.ok(!model.capabilities.includes('chat'));
  }
  assert.deepEqual(effectiveMetadata({endpoints:['responses']}).endpoints,['responses']);
  assert.deepEqual(effectiveMetadata({capabilities:['vendor-extension']}).capabilities,['vendor-extension']);
  assert.deepEqual(effectiveMetadata({capabilities:['vendor-extension']}).endpoints,[]);
});
test('Jev remains a native decision model, not an agent chat model', () => {
  assert.deepEqual(effectiveMetadata({capabilities:['chat'],endpoints:['chat_completions']},'typesafe'),{kind:'unknown',capabilities:['decision'],endpoints:['systemone']});
  const request = {state:{ticket:'charged twice'},questions:{billing:{type:'noul',instructions:'Is this about billing?'}},assets:{},model:'other'};
  assert.deepEqual(systemOneRequest('jev-latest',JSON.stringify(request)),{...request,model:'jev-latest'});
  assert.throws(()=>systemOneRequest('jev-latest','hello'),/state.*questions/);
  assert.throws(()=>systemOneRequest('jev-latest','{"state":"x","questions":[]}'),/state.*questions/);
  assert.throws(()=>systemOneRequest('jev-latest',JSON.stringify({...request,stream:true})),/流式/);
});
