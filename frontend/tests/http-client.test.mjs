import {test} from 'node:test';import assert from 'node:assert/strict';
import {apiBases,apiError,jsonResponse,authenticatedFetch} from '../src/http-client.js';
test('all service bases use the same VOS namespace',()=>{
 for(const path of ['/app/com.ictrek.v-scheduler/','/api/com.ictrek.v-scheduler/'])assert.deepEqual(apiBases(path),{gateway:'/api/com.ictrek.v-scheduler',scheduler:'/api/com.ictrek.v-scheduler/scheduler/api/v1/',models:'/api/com.ictrek.v-scheduler/model-hub'});
 assert.equal(apiBases('/').scheduler,'/scheduler/api/v1/');
});
test('all legacy error formats preserve machine reason and request correlation',()=>{
 for(const body of [{code:'WAIT_STOP_CONFIRMATION',msg:'wait',request_id:'trace'},{code:'WAIT_STOP_CONFIRMATION',detail:'wait',request_id:'trace'},{code:'WAIT_STOP_CONFIRMATION',error:{message:'wait'},request_id:'trace'}]){
  const e=apiError(body,429);assert.equal(e.code,'WAIT_STOP_CONFIRMATION');assert.equal(e.requestId,'trace');assert.match(e.message,/wait.*WAIT_STOP_CONFIRMATION.*trace/);
 }
});
test('success shapes and empty DELETE responses stay compatible',async()=>{
 for(const body of [[{id:'model'}],{category:'v-scheduler',name:'legacy metadata'},{code:'OK',category:'v-scheduler',data:{items:[]}}])assert.deepEqual(await jsonResponse(new Response(JSON.stringify(body))),body);
 assert.equal(await jsonResponse(new Response(null,{status:204})),null);
 await assert.rejects(jsonResponse(new Response('bad')),{code:'INVALID_RESPONSE'});
 await assert.rejects(jsonResponse(new Response('{"code":"FORBIDDEN","category":"v-scheduler"}')),{code:'FORBIDDEN'});
});
test('all requests follow token rotation and retry a 401 exactly once',async()=>{
 let token='old';const calls=[];const auth={token:async()=>token,clearToken:old=>{assert.equal(old,'old');token='new'},fetch:async(url,init)=>{calls.push(init.headers.get('Authorization'));return new Response('{}',{status:calls.length===1?401:200})}};
 const response=await authenticatedFetch('/api',{},auth);assert.equal(response.status,200);assert.deepEqual(calls,['Bearer old','Bearer new']);
 let count=0;await authenticatedFetch('/api',{}, {...auth,clearToken:()=>{},fetch:async()=>{count++;return new Response('{}',{status:401})}});assert.equal(count,2);
});
test('permission errors do not renew credentials or replay mutations',async()=>{
 let calls=0;const response=await authenticatedFetch('/api',{method:'POST',body:'{}'},{token:async()=>'',clearToken:()=>assert.fail('403 renewal'),fetch:async()=>{calls++;return new Response('{}',{status:403})}});assert.equal(response.status,403);assert.equal(calls,1);
});

test('an upstream service identity failure does not renew a browser identity',async()=>{
 let calls=0;const response=await authenticatedFetch('/v1/chat/completions',{method:'POST',body:'{}'},{token:async()=>'caller',clearToken:()=>assert.fail('service identity is not browser identity'),fetch:async()=>{calls++;return new Response('{"code":"UNAUTHENTICATED","category":"v-scheduler"}',{status:503})}});assert.equal(response.status,503);assert.equal(calls,1);
});

test('a provider credential failure does not trigger a browser token refresh',async()=>{
 let calls=0;const response=await authenticatedFetch('/v1/chat/completions',{method:'POST',body:'{}'},{token:async()=>'browser-token',clearToken:()=>assert.fail('provider key is separate'),fetch:async()=>{calls++;return new Response('{"category":"v-scheduler","error":{"code":"PROVIDER_AUTH_REQUIRED","type":"authentication_error"}}',{status:401})}});assert.equal(response.status,401);assert.equal(calls,1);
 const err=apiError({error:{type:'authentication_error',message:'provider credentials invalid'}},401);assert.equal(err.sessionError,false);
});
