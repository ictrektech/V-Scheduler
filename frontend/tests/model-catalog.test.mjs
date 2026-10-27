import {test} from 'node:test';
import assert from 'node:assert/strict';
import {groupCatalog} from '../src/gateway/model-catalog.js';
const upstreams=[{id:'a',source:'api',name:'Remote',base_url:'https://example.invalid/v1'},{id:'b',source:'v-voice',name:'Voice',base_url:'http://v-voice:7070/v1'},{id:'empty',source:'api',name:'Needs configuration'}];
const models=[{id:'chat',qualified:'remote/chat',upstream_id:'a',capabilities:['vision']},{id:'voice',upstream_id:'b',capabilities:['tts']},{id:'orphan',upstream_id:'not-listed',upstream_url:'http://standalone:8080/v1'}];
test('grouping preserves empty upstreams, model identity and orphan model addresses',()=>{
 const grouped=groupCatalog(upstreams,models);
 assert.equal(grouped.length,4);assert.equal(grouped.reduce((n,g)=>n+g.count,0),3);
 assert.equal(grouped.find(g=>g.key==='upstream:a').upstreams[0].catalogModels[0],models[0]);
 assert.equal(grouped.find(g=>g.key==='upstream:not-listed').upstreams[0].base_url,'http://standalone:8080/v1');
 assert.equal(upstreams[0].catalogModels,undefined);
});
test('application grouping and search do not silently discard configuration-only sources',()=>{
 const byApp=groupCatalog(upstreams,models,'application');
 assert.equal(byApp.find(g=>g.key==='application:api').upstreams.length,2);
 assert.equal(groupCatalog(upstreams,models,'upstream','example.invalid')[0].count,1);
 assert.equal(groupCatalog(upstreams,models,'upstream','vision')[0].count,1);
 assert.equal(groupCatalog(upstreams,models,'upstream','needs')[0].count,0);
 assert.deepEqual(groupCatalog(upstreams,models,'upstream','no match'),[]);
});

test('capability and source searches match both IDs and visible localized labels',()=>{
 const sources=[{id:'cloud',name:'Vendor',source:'api',base_url:'https://example.invalid'}];
 const models=[{id:'vision-model',upstream_id:'cloud',capabilities:['vision','chat']}];
 assert.equal(groupCatalog(sources,models,'upstream','图片')[0].count,1);
 const translate=text=>({'图片理解':'Image understanding','外部上游':'External providers'})[text]||text;
 assert.equal(groupCatalog(sources,models,'upstream','image',translate)[0].count,1);
 assert.equal(groupCatalog(sources,models,'upstream','external',translate)[0].count,1);
});
