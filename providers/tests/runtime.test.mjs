import {test} from 'node:test';
import assert from 'node:assert/strict';
import http from 'node:http';
import {spawn} from 'node:child_process';
import {once} from 'node:events';
import fs from 'node:fs/promises';
import os from 'node:os';
import path from 'node:path';
import {contextFromChat,streamOptions,completion} from '../bridge.mjs';
import presets from '../presets.json' with {type:'json'};
import ids from '../catalog.json' with {type:'json'};
import {builtinProviders} from '@earendil-works/pi-ai/providers/all';
const worker=new URL('../worker.mjs',import.meta.url);
function run(input,preload=false) {
 const args=preload?["--import",new URL("./oauth-fixture.mjs",import.meta.url).href,worker.pathname]:[worker.pathname];
 const child=spawn(process.execPath,args,{env:{PATH:process.env.PATH,HOME:'/nonexistent'},stdio:['pipe','pipe','pipe']});
 let out='',err='';child.stdout.on('data',b=>out+=b);child.stderr.on('data',b=>err+=b);
 child.stdin.end(JSON.stringify(input));
 return {child,result:once(child,'exit').then(()=>{assert.equal(err,'');const frames=out.split('\n');if(frames[0]==='{"ready":true}')frames.shift();return {head:JSON.parse(frames.shift()),body:frames.join('\n'),raw:out};})};
}
async function fixture(fn) {
 const requests=[];const server=http.createServer(async(req,res)=>{let raw='';for await(const b of req)raw+=b;requests.push({path:req.url,headers:req.headers,body:raw?JSON.parse(raw):{}});await fn(req,res,requests.at(-1));});
 server.listen(0,'127.0.0.1');await once(server,'listening');
 return {requests,base:`http://127.0.0.1:${server.address().port}/v1`,close:()=>{server.closeAllConnections();server.close();}};
}
const fixtureModel=builtinProviders().find(p=>p.id==='deepseek').getModels()[0].id;
const openai={provider:'deepseek',api_key:'fixture-private-key',operation:'chat',body:{model:fixtureModel,messages:[{role:'user',content:'hello'}],max_tokens:16}};
function sse(res,delta={content:'hello'}) {
 res.writeHead(200,{'content-type':'text/event-stream'});
 for(const v of [{choices:[{index:0,delta:{role:'assistant',...delta},finish_reason:null}]},{choices:[{index:0,delta:{},finish_reason:delta.tool_calls?'tool_calls':'stop'}],usage:{prompt_tokens:4,completion_tokens:2,total_tokens:6}}])res.write('data: '+JSON.stringify({id:'upstream',object:'chat.completion.chunk',model:'gpt-4o-mini',...v})+'\n\n');
 res.end('data: [DONE]\n\n');
}
test('deduped dsh catalog identities are all supplied by pinned runtime',()=>{
 assert.equal(ids.length,42);const builtin=new Set(builtinProviders().map(p=>p.id));assert.equal(new Set(ids).size,ids.length);for(const id of ids)assert.ok(builtin.has(id),id);
 assert.equal(new Set(presets.map(p=>p.id)).size,presets.length);
});
test('context retains images, tool history and structured arguments; rejects unsupported inputs',()=>{
 const model={api:'openai-completions',provider:'openai',id:'fixture'};
 const ctx=contextFromChat({messages:[{role:'system',content:'be helpful'},{role:'user',content:[{type:'text',text:'look'},{type:'image_url',image_url:{url:'data:image/png;base64,YQ=='}}]},{role:'assistant',content:null,tool_calls:[{id:'call1',type:'function',function:{name:'weather',arguments:'{"city":"x"}'}}]},{role:'tool',tool_call_id:'call1',content:'sunny'}]},model);
 assert.equal(ctx.messages[1].content[1].data,'YQ==');assert.equal(ctx.messages[3].toolName,'weather');
 assert.throws(()=>contextFromChat({messages:[{role:'user',content:[{type:'image_url',image_url:{url:'http://private/image'}}]}]},model),/base64/);
 assert.throws(()=>streamOptions({response_format:{type:'json_object'}},{api:'anthropic-messages'}),/requires/);
 assert.equal(streamOptions({max_tokens:23,reasoning_effort:'high'},model).maxTokens,23);
});
test('native OpenAI-compatible provider uses actual SDK, preserves tools, usage and private key without leaking it',async()=>{
 const f=await fixture((req,res)=>sse(res,{tool_calls:[{index:0,id:'call1',type:'function',function:{name:'weather',arguments:'{"city":"x"}'}}]}));
 try {
  const r=await run({...openai,base_url:f.base,body:{...openai.body,tools:[{type:'function',function:{name:'weather',parameters:{type:'object',properties:{city:{type:'string'}}}}}]}}).result;
  assert.equal(r.head.status,200,r.body);const body=JSON.parse(r.body);assert.equal(body.choices[0].finish_reason,'tool_calls');assert.equal(body.choices[0].message.tool_calls[0].function.arguments,'{"city":"x"}');assert.equal(body.usage.total_tokens,6);
  assert.equal(f.requests[0].headers.authorization,'Bearer fixture-private-key');assert.equal(f.requests[0].body.tools[0].function.name,'weather');assert.ok(!r.raw.includes('fixture-private-key'));
 }finally{f.close();}
});
test('streamed native chat terminates with DONE and emits usage',async()=>{
 const f=await fixture((req,res)=>sse(res));
 try {const r=await run({...openai,base_url:f.base,body:{...openai.body,stream:true,stream_options:{include_usage:true}}}).result;assert.equal(r.head.content_type,'text/event-stream');assert.match(r.body,/"content":"hello"/);assert.match(r.body,/"total_tokens":6/);assert.match(r.body,/data: \[DONE\]/);}finally{f.close();}
});
for(const status of [401,429])test(`provider HTTP ${status} survives normalization and error bodies cannot leak keys`,async()=>{
 const f=await fixture((req,res)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify({error:{message:'secret fixture-private-key',type:'error'}}));});
 try {const r=await run({...openai,base_url:f.base}).result;assert.equal(r.head.status,status,r.body);assert.ok(!r.raw.includes('fixture-private-key'));assert.equal(f.requests.length,1,'no hidden SDK retry');}finally{f.close();}
});
test('Anthropic native request and response pass through the same OpenAI boundary',async()=>{
 const f=await fixture((req,res)=>{
  res.writeHead(200,{'content-type':'text/event-stream'});
  const events=[['message_start',{message:{id:'m',type:'message',role:'assistant',model:'claude-sonnet-4-5',content:[],usage:{input_tokens:4,output_tokens:0}}}],['content_block_start',{index:0,content_block:{type:'text',text:''}}],['content_block_delta',{index:0,delta:{type:'text_delta',text:'native anthropic'}}],['content_block_stop',{index:0}],['message_delta',{delta:{stop_reason:'end_turn'},usage:{output_tokens:2}}],['message_stop',{}]];
  for(const [type,v] of events)res.write(`event: ${type}\ndata: ${JSON.stringify({type,...v})}\n\n`);res.end();
 });
 try {const r=await run({...openai,provider:'anthropic',base_url:f.base.replace('/v1',''),body:{...openai.body,model:'claude-sonnet-4-5'}}).result;assert.equal(r.head.status,200,r.body);assert.equal(JSON.parse(r.body).choices[0].message.content,'native anthropic');assert.equal(f.requests[0].path.split('?')[0],'/v1/messages');assert.equal(f.requests[0].headers['x-api-key'],'fixture-private-key');}finally{f.close();}
});
test('native discovery gives capabilities but incomplete OAuth stays unauthorized',async()=>{
 const listed=await run({...openai,provider:'openai',operation:'models'}).result;assert.equal(listed.head.status,200,listed.body);const rows=JSON.parse(listed.body).models;assert.ok(rows.some(m=>m.capabilities.includes('vision')));assert.ok(rows.every(m=>m.endpoints.includes('chat_completions')));
 const missing=await run({...openai,provider:'openai-codex',api_key:''}).result;assert.equal(missing.head.status,401);assert.ok(!missing.raw.includes('refresh'));
});
test('client cancellation kills child and closes the native upstream connection',async()=>{
 let closed;const disconnected=new Promise(resolve=>closed=resolve);
 const f=await fixture((req,res)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.write(':waiting\n\n');res.on('close',closed);});
 try {
  const r=run({...openai,base_url:f.base,body:{...openai.body,stream:true}});
  while(!f.requests.length)await new Promise(resolve=>setTimeout(resolve,20));
  r.result.catch(()=>{});r.child.kill('SIGTERM');await once(r.child,'exit');await Promise.race([disconnected,new Promise((_,reject)=>setTimeout(()=>reject(Error('upstream not cancelled')),3000))]);
 }finally{f.close();}
});

test('OAuth refresh is persisted privately and a subsequent worker uses the rotated token',async()=>{
 const root=await fs.mkdtemp(path.join(os.tmpdir(),'vs-provider-oauth-'));
 const credential_path=path.join(root,'credential.json');
 const input={provider:'anthropic',operation:'models',credential_path,credential:{type:'oauth',access:'fixture-expired-access',refresh:'fixture-refresh',expires:1}};
 try {
  const first=await run(input,true).result;assert.equal(first.head.status,200,first.body);
  const stored=JSON.parse(await fs.readFile(credential_path,'utf8'));assert.equal(stored.refresh,'fixture-rotated-refresh');assert.ok(stored.expires>Date.now());assert.equal((await fs.stat(credential_path)).mode&0o777,0o600);
  const second=await run(input,true).result;assert.equal(second.head.status,200,second.body);assert.ok(!second.raw.includes('fixture-renewed-access'));
 }finally{await fs.rm(root,{recursive:true,force:true});}
});

test('Bedrock uses scoped AWS SigV4 signing through the native SDK',async()=>{
 const f=await fixture((req,res)=>{res.writeHead(403,{'content-type':'application/json','x-amzn-errortype':'AccessDeniedException'});res.end('{"message":"fixture denied"}');});
 const model=builtinProviders().find(p=>p.id==='amazon-bedrock').getModels()[0].id;
 try {
  const r=await run({operation:'chat',provider:'amazon-bedrock',base_url:f.base.replace('/v1',''),credential:{type:'api_key',env:{AWS_ACCESS_KEY_ID:'fixture-access-id',AWS_SECRET_ACCESS_KEY:'fixture-secret-key',AWS_REGION:'us-east-1',AWS_BEDROCK_FORCE_HTTP1:'1'}},body:{...openai.body,model}}).result;
  assert.equal(r.head.status,403,r.body);assert.match(f.requests[0].headers.authorization,/AWS4-HMAC-SHA256/);assert.match(f.requests[0].path,/converse-stream/);assert.ok(!r.raw.includes('fixture-secret-key'));
 }finally{f.close();}
});

test('merged Qwen configuration retains models from both dsh aliases',async()=>{
 const expected=new Set(builtinProviders().filter(p=>['qwen-token-plan','qwen-token-plan-individual'].includes(p.id)).flatMap(p=>p.getModels().map(m=>m.id)));
 const result=await run({...openai,operation:'models',provider:'qwen-token-plan'}).result;
 assert.equal(result.head.status,200,result.body);assert.deepEqual(new Set(JSON.parse(result.body).models.map(m=>m.id)),expected);
});

test('native OpenAI Responses SDK produces chat output through the shared boundary',async()=>{
 const f=await fixture((req,res)=>{
  res.writeHead(200,{'content-type':'text/event-stream'});
  const message={id:'msg1',type:'message',role:'assistant',status:'completed',content:[{type:'output_text',text:'native responses',annotations:[]}]};
  const events=[
   {type:'response.created',response:{id:'resp1',model:'gpt-4o-mini',status:'in_progress',output:[]}},
   {type:'response.output_item.added',output_index:0,item:{...message,status:'in_progress',content:[]}},
   {type:'response.content_part.added',output_index:0,content_index:0,part:{type:'output_text',text:'',annotations:[]}},
   {type:'response.output_text.delta',output_index:0,content_index:0,delta:'native responses'},
   {type:'response.output_item.done',output_index:0,item:message},
   {type:'response.completed',response:{id:'resp1',model:'gpt-4o-mini',status:'completed',output:[message],usage:{input_tokens:4,output_tokens:2,total_tokens:6}}},
  ];
  for(const e of events)res.write('data: '+JSON.stringify(e)+'\n\n');res.end();
 });
 try {
  const result=await run({...openai,provider:'openai',base_url:f.base,body:{...openai.body,model:'gpt-4o-mini'}}).result;
  assert.equal(result.head.status,200,result.body);assert.equal(JSON.parse(result.body).choices[0].message.content,'native responses');assert.equal(f.requests[0].path,'/v1/responses');
 }finally{f.close();}
});

test('native Gemini RPC SDK produces chat output and scoped API-key authentication',async()=>{
 const model=builtinProviders().find(p=>p.id==='google').getModels()[0].id;
 const f=await fixture((req,res)=>{res.writeHead(200,{'content-type':'text/event-stream'});res.end('data: '+JSON.stringify({candidates:[{content:{role:'model',parts:[{text:'native gemini'}]},finishReason:'STOP'}],usageMetadata:{promptTokenCount:4,candidatesTokenCount:2,totalTokenCount:6}})+'\n\n');});
 try {
  const result=await run({...openai,provider:'google',base_url:f.base,body:{...openai.body,model}}).result;
  assert.equal(result.head.status,200,result.body);assert.equal(JSON.parse(result.body).choices[0].message.content,'native gemini');assert.match(f.requests[0].path,/:streamGenerateContent/);assert.equal(f.requests[0].headers['x-goog-api-key'],'fixture-private-key');
 }finally{f.close();}
});
