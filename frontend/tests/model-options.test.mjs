import {NATIVE_PROVIDERS} from '../src/gateway/native-providers.js';
import {MODELS_DEV_PROVIDERS} from '../src/gateway/models-dev-providers.js';
import {test} from 'node:test';
import assert from 'node:assert/strict';
import {CAPABILITIES, ENDPOINTS, PROVIDER_PRESETS, effectiveMetadata, systemOneRequest, validateProviderAddress, replaceProviderCatalog} from '../src/gateway/model-options.js';

test('provider presets have unique identities and endpoints, and never invent model IDs', () => {
  assert.equal(new Set(PROVIDER_PRESETS.map(p => p.id)).size, PROVIDER_PRESETS.length);
  for (const p of PROVIDER_PRESETS) {
    if(p.base&&!p.base.includes('${')){const url=new URL(p.base.replace(/\$\{[^}]+\}/g,'example'));assert.ok(['https:','http:'].includes(url.protocol));if(url.protocol==='http:')assert.ok(['127.0.0.1','localhost'].includes(url.hostname));}
    assert.ok(['openai', 'chat_completions', 'anthropic', 'typesafe', 'ollama', 'pi'].includes(p.protocol));
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

test('Models.dev expands the directory while native SDK defaults and compatible protocols remain intact',()=>{
 assert.ok(MODELS_DEV_PROVIDERS.length>200);
 assert.equal(new Set(MODELS_DEV_PROVIDERS.map(p=>p.id)).size,MODELS_DEV_PROVIDERS.length);
 assert.equal(PROVIDER_PRESETS.find(p=>p.id==='deepinfra').protocol,'chat_completions');
 assert.equal(PROVIDER_PRESETS.find(p=>p.id==='anthropic').base,'https://api.anthropic.com');
 assert.equal(PROVIDER_PRESETS.find(p=>p.id==='anthropic').protocol,'pi');
 assert.equal(PROVIDER_PRESETS.find(p=>p.id==='fireworks-ai').dshIds[0],'fireworks');
 assert.equal(PROVIDER_PRESETS.find(p=>p.id==='ollama-cloud').protocol,'ollama');
 for(const id of ['openai-codex','radius'])assert.ok(PROVIDER_PRESETS.some(p=>p.id===id));
 assert.ok(NATIVE_PROVIDERS.some(p=>p.id==='qwen-token-plan-individual'));
 assert.throws(()=>validateProviderAddress('https://YOUR-RESOURCE-NAME.openai.azure.com/openai/v1'),/替换/);
 assert.throws(()=>validateProviderAddress(''),/地址/);
 validateProviderAddress('https://example.invalid/v1');
 const native=JSON.stringify(NATIVE_PROVIDERS);
 replaceProviderCatalog([{id:'fixture',name:'Fixture',base:'https://example.invalid/v1',registrationProtocol:'chat_completions',registrationBase:'https://example.invalid/v1'}]);
 assert.equal(PROVIDER_PRESETS.filter(p=>p.id==='fixture').length,1);
 assert.equal(JSON.stringify(NATIVE_PROVIDERS),native,'catalog refresh never rewrites SDK identities or URL defaults');
 replaceProviderCatalog(MODELS_DEV_PROVIDERS);
});
