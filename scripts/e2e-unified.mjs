// Run after e2e-statistics.mjs with KEEP_TEST_SERVICES=1 on the build host.
// All credentials and persistent files belong to that isolated test fixture.
import assert from 'node:assert/strict'
import {createServer} from 'node:http'
import {spawn} from 'node:child_process'
import {readFileSync,writeFileSync,openSync,copyFileSync} from 'node:fs'
import {networkInterfaces} from 'node:os'
import path from 'node:path'
const run=process.env.SCHEDULER_TEST_RUN
assert(run,'SCHEDULER_TEST_RUN must identify the retained statistics fixture')
const fixture=JSON.parse(readFileSync(path.join(run,'services.json')))
const config=JSON.parse(readFileSync(path.join(run,'config/controller.json')))
const token=config.principals.find(p=>p.role==='admin').token
const gatewayPort=Number(process.env.GATEWAY_TEST_PORT||38782),mockPort=gatewayPort+1
const host=Object.values(networkInterfaces()).flat().find(n=>n.family==='IPv4'&&!n.internal)?.address
assert(host,'an IPv4 interface is required for the isolated fake upstream')
const mock=createServer(async(req,res)=>{
 res.setHeader('Content-Type','application/json')
 if(req.url==='/userinfo'){
  const principal=config.principals.find(p=>req.headers.authorization==='Bearer '+p.token)
  if(!principal){res.writeHead(401);res.end('{}');return}
  res.end(JSON.stringify({sub:principal.app_id,preferred_username:principal.app_id}));return
 }
 if(req.url==='/v1/models'){res.end(JSON.stringify({object:'list',data:[{id:'unified-echo'}]}));return}
 if(req.url==='/v1/chat/completions'){
  let body='';for await(const chunk of req)body+=chunk
  const input=JSON.parse(body);assert.equal(input.model,'unified-echo')
  if(input.stream){res.setHeader('Content-Type','text/event-stream');for(const content of ['Unified ','gateway works'])res.write('data: '+JSON.stringify({choices:[{delta:{content}}]})+'\n\n');res.end('data: '+JSON.stringify({choices:[],usage:{prompt_tokens:3,completion_tokens:2,total_tokens:5}})+'\n\ndata: [DONE]\n\n')}
  else res.end(JSON.stringify({choices:[{message:{role:'assistant',content:'Unified gateway works'}}],usage:{prompt_tokens:3,completion_tokens:2,total_tokens:5}}))
  return
 }
 res.writeHead(404);res.end('{}')
})
await new Promise(resolve=>mock.listen(mockPort,'0.0.0.0',resolve))
const base=`http://127.0.0.1:${gatewayPort}`
const state=path.join(run,'gateway-state.json'),usage=path.join(run,'gateway-usage.jsonl')
const log=openSync(path.join(run,'gateway.log'),'a',0o600)
const gateway=spawn(process.env.GATEWAY_BINARY||path.join(process.env.HOME,'dev/build/V-Scheduler/gateway/v-router'),['-addr',`127.0.0.1:${gatewayPort}`,'-state',state],{env:{...process.env,V_ROUTER_CONTROLLER_URL:`http://127.0.0.1:${fixture.port}`,VOS_OIDC_USERINFO_URL:`http://127.0.0.1:${mockPort}/userinfo`,V_ROUTER_USAGE_PATH:usage,V_ROUTER_IMAGE_STUDIO_URL:'',V_ROUTER_V_VOICE_URLS:'',V_ROUTER_V_MUSIC_URLS:'',V_ROUTER_ML_SERVER_URLS:'',V_ROUTER_VIDEO_STRUCTURE_URLS:'',V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS:''},stdio:['ignore',log,log]})
const checks=[]
function pass(message){checks.push(message);console.log('PASS: '+message)}
async function call(endpoint,method='GET',body,auth=token){const response=await fetch(base+endpoint,{method,headers:{...(auth?{Authorization:'Bearer '+auth}:{}),'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body)});const text=await response.text();let data;try{data=JSON.parse(text)}catch{data=text}return {status:response.status,data}}
try{
 for(let i=0;i<100;i++){try{if((await fetch(base+'/healthz')).ok)break}catch{}await new Promise(r=>setTimeout(r,100))}
 assert.equal((await call('/api/v1/status','GET',undefined,'')).status,401)
 assert.equal((await call('/api/v1/status')).status,200)
 assert.equal((await call('/scheduler/api/v1/nodes','GET',undefined,'')).status,401)
 assert.equal((await call('/scheduler/api/v1/nodes')).status,200)
 const submitter=config.principals.find(p=>p.role==='submitter')
 if(submitter){
  assert.equal((await call('/api/v1/status','GET',undefined,submitter.token)).status,200)
  assert.equal((await call('/scheduler/api/v1/nodes','GET',undefined,submitter.token)).status,403)
 }
 assert.equal((await call('/scheduler/api/v1/discovery')).status,200)
 assert.equal((await call('/scheduler/api/v1/openapi.json')).status,200)
 assert.equal((await call('/api/v1/nodes')).status,404)
 assert.equal((await call('/')).status,200)
 pass('one UI and prefixed controller API preserve authentication; gateway routes keep their existing API')
 const created=await call('/api/v1/upstreams','POST',{kind:'cloud',name:'Unified fixture',base_url:`http://${host}:${mockPort}/v1`,visibility:'public'})
 assert.equal(created.status,201,JSON.stringify(created.data));const id=created.data.id
 const models=(await call('/v1/models')).data.data
 const model=models.find(m=>m.id.endsWith('/unified-echo'))?.id;assert(model)
 assert.equal((await call('/api/v1/upstreams/'+id,'PUT',{kind:'cloud',name:'Unified fixture edited',base_url:`http://${host}:${mockPort}/v1`,visibility:'public'})).status,200)
 assert.equal((await call('/api/v1/upstreams/'+id+'/refresh','POST',{})).status,200)
 const answer=await call('/v1/chat/completions','POST',{model,stream:true,messages:[{role:'user',content:'test'}]})
 assert.equal(answer.status,200,JSON.stringify(answer.data));assert(answer.data.includes('Unified '));assert(answer.data.includes('[DONE]'))
 assert.equal((await call('/api/v1/usage?period=all')).status,200)
 const docs=await call('/api/v1/apidoc');assert.equal(docs.status,200)
 const spec=await call('/api/v1/openapi.json');assert.equal(spec.status,200)
 pass('retained gateway model discovery, upstream edit/refresh, streaming, usage and live API documentation work together')
 copyFileSync(state,path.join(run,'gateway-state-before-restart.json'))
 assert.equal((await call('/api/v1/upstreams/'+id,'DELETE')).status,204)
 assert(!(await call('/v1/models')).data.data.some(m=>m.id===model))
 pass('deleting an upstream removes its advertised models')
 // Leave a real upstream and the built UI available for browser interaction.
 assert.equal((await call('/api/v1/upstreams','POST',{kind:'cloud',name:'Browser fixture',base_url:`http://${host}:${mockPort}/v1`,visibility:'public'})).status,201)
 writeFileSync(path.join(run,'unified-evidence.json'),JSON.stringify({checks,url:base}),{mode:0o600})
 if(process.env.KEEP_TEST_SERVICES==='1'){console.log('Unified fixture retained: '+base);await new Promise(resolve=>{process.once('SIGTERM',resolve);process.once('SIGINT',resolve)})}
}finally{gateway.kill('SIGTERM');mock.close()}
