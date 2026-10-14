// Execute on an authorized Linux build host after rsync/build. No Git operations.
import {spawn,execFileSync} from 'node:child_process'
import {randomBytes} from 'node:crypto'
import {mkdirSync,writeFileSync,readFileSync,existsSync,openSync} from 'node:fs'
import path from 'node:path'
import {createServer} from 'node:http'
import assert from 'node:assert/strict'
const root=process.env.SCHEDULER_SOURCE||path.resolve(import.meta.dirname,'..')
const build=process.env.SCHEDULER_BUILD||path.join(process.env.HOME,'dev/build/V-Scheduler')
const run=path.join(build,'e2e-'+Date.now());mkdirSync(run,{recursive:true,mode:0o700})
const tokens=Object.fromEntries(['admin','service','worker','executor','other','db','router','protocol'].map(k=>[k,randomBytes(32).toString('hex')]))
function file(name,value){const p=path.join(run,name);writeFileSync(p,typeof value==='string'?value:JSON.stringify(value),{mode:0o600});return p}
const port=Number(process.env.SCHEDULER_TEST_BASE_PORT||38220)
assert(Number.isInteger(port)&&port>=1024&&port<=65528)
const nodeId='tc232-test';const api=`http://127.0.0.1:${port}/api/v1`;const image=`http://127.0.0.1:${port+2}`
const dbName='v-scheduler-e2e-pg-'+process.pid;const imageName='v-scheduler-e2e-image-'+process.pid
const config={principals:[{token:tokens.admin,role:'admin',app_id:'admin'},{token:tokens.service,role:'submitter',app_id:'image-studio'},{token:tokens.other,role:'submitter',app_id:'other'},{token:tokens.worker,role:'worker',app_id:'agent',node_id:nodeId}],nodes:[{id:nodeId,capacities:{cpu_millicores:4000,memory_mib:16384,gpu_slots:1,gpu_memory_mib:16384}}],executors:[{id:'image-studio.generate',contract_version:'1',task_type:'image.generate',node_id:nodeId,app_id:'image-studio',resources:{cpu_millicores:2000,memory_mib:8192,gpu_slots:1,gpu_memory_mib:8192}}]}
config.principals.push({token:tokens.router,role:'submitter',app_id:'v-router',delegated_executors:['image-studio.generate','protocol-test']})
config.executors.push({id:'v-router.local.infer',contract_version:'1',task_type:'model.infer',node_id:nodeId,app_id:'v-router',upstream_urls:[`http://127.0.0.1:${port+4}`],resources:{cpu_millicores:1000,memory_mib:1024,gpu_slots:1,gpu_memory_mib:1024}})
let sourceFailed=false
const vos=createServer((req,res)=>{assert.equal(req.method,'GET');res.setHeader('Content-Type','application/json');if(sourceFailed){res.statusCode=503;res.end('{}');return}const data=req.url==='/api/nodes'?{nodes:[{member_id:'member-bound',display_name:'Test GPU',role:'worker',node_ip:'127.0.0.1'},{member_id:'member-observe',display_name:'Observe',role:'worker',node_ip:'192.0.2.1'}]}:req.url==='/v1000/nas/device/info'?{cpu_logical_count:8,memory:{size:17179869184,unit:'B'}}:req.url==='/v1000/nas/device_usage'?{cpu:{used:25},ram:{size:17179869184,used:2147483648,unit:'B'},gpu:{used:0}}:req.url==='/v1000/app/list'?{total:1,data:[{app_id:'com.ictrek.image-studio',installed_app_id:'local-instance',state:'installed',version:'test',owner_user_id:'redacted'}]}:{};res.end(JSON.stringify({code:0,data}))})
await new Promise(resolve=>vos.listen(0,'127.0.0.1',resolve))
const vosUrl='http://127.0.0.1:'+vos.address().port
config.vos_cluster={url:vosUrl};config.nodes[0].vos={url:vosUrl};config.nodes[0].vos_member_id='member-bound'
config.nodes.push({id:'protocol-node',capacities:{cpu_millicores:1500,memory_mib:4096,gpu_slots:0,gpu_memory_mib:0},resident_baseline:{cpu_millicores:500}})
config.principals.push({token:tokens.protocol,role:'worker',app_id:'agent',node_id:'protocol-node'})
config.executors.push({id:'protocol-test',contract_version:'1',task_type:'protocol.test',node_id:'protocol-node',app_id:'other',resources:{cpu_millicores:1000,memory_mib:128,gpu_slots:0,gpu_memory_mib:0},retry_mode:'idempotent',max_attempts:3})
config.executors.push({id:'v-router.image.bridge',contract_version:'1',task_type:'model.infer',node_id:nodeId,app_id:'v-router',upstream_urls:[image],resources:{cpu_millicores:1000,memory_mib:1024,gpu_slots:1,gpu_memory_mib:8192}})
config.executors.push({id:'v-router.protocol.infer',contract_version:'1',task_type:'model.infer',node_id:'protocol-node',app_id:'v-router',upstream_urls:[`http://127.0.0.1:${port+5}`],resources:{cpu_millicores:1000,memory_mib:64}})
const controllerConfig=file('controller.json',config);file('admin-token',tokens.admin)
const agentConfig=file('agent.json',{controller:api,node_id:nodeId,token:tokens.worker,journal:path.join(run,'journal/agent.json'),executors:[{id:'image-studio.generate',url:image+'/api/v1',token:tokens.executor}]})
const dbEnv=file('postgres.env',`POSTGRES_DB=vscheduler\nPOSTGRES_USER=vscheduler\nPOSTGRES_PASSWORD=${tokens.db}\n`)
const imgEnv=file('image.env',`IMAGE_STUDIO_SCHEDULER_URL=${api}\nIMAGE_STUDIO_SCHEDULER_TOKEN=${tokens.service}\nIMAGE_STUDIO_SCHEDULER_EXECUTOR_TOKEN=${tokens.executor}\nIMAGE_STUDIO_VOS_SSO_ENABLED=false\nIMAGE_STUDIO_IMMICH_AUTO_PROVISION=false\nIMAGE_STUDIO_MOCK_WHEN_RUNTIME_MISSING=false\nMODEL_HUB_API_URL=http://127.0.0.1:35005\nMODEL_HUB_SHARED_MODELS_PATH=/modelhub\nIMAGE_STUDIO_DATA_DIR=/data\n`)
const bin=process.env.SCHEDULER_BINARY||path.join(build,'cargo/debug/v-scheduler');const children=[]
function launch(args,name){const fd=openSync(path.join(run,name+'.log'),'a',0o600);const proc=spawn(bin,args,{detached:true,env:{...process.env,SCHEDULER_DATABASE_URL:`postgres://vscheduler:${tokens.db}@127.0.0.1:${port+3}/vscheduler`},stdio:['ignore',fd,fd]});children.push(proc);return proc}
const delay=ms=>new Promise(r=>setTimeout(r,ms))
async function call(url,token,method='GET',body,extra={}){const r=await fetch(url,{method,headers:{...(token?{Authorization:'Bearer '+token}:{}),'Content-Type':'application/json',...extra},body:body===undefined?undefined:JSON.stringify(body)});const text=await r.text();let data;try{data=JSON.parse(text)}catch{data=text}return {status:r.status,data:data?.data??data}}
async function ready(url){for(let i=0;i<60;i++){try{if((await fetch(url)).ok)return}catch{}await delay(1000)}throw Error('service not ready: '+url)}
async function task(id){return (await call(api+'/tasks/'+id,tokens.admin)).data}
async function until(id,states,timeout=180000){const end=Date.now()+timeout;while(Date.now()<end){const t=await task(id);if(states.includes(t.status))return t;await delay(500)}throw Error('timeout waiting '+states)}
let controller,agent
try{
 execFileSync('docker',['run','-d','--name',dbName,'--env-file',dbEnv,'-p',`127.0.0.1:${port+3}:5432`,'-v',path.join(run,'pg')+':/var/lib/postgresql','registry.ictrek.internal/library/postgres:18'],{stdio:'pipe'})
 for(let i=0;i<30;i++){try{execFileSync('docker',['exec',dbName,'pg_isready','-h','127.0.0.1','-U','vscheduler'],{stdio:'pipe'});break}catch{await delay(1000)}}
 controller=launch(['controller','--config',controllerConfig,'--listen',`127.0.0.1:${port}`,'--ui-dir',path.join(build,'ui')],'controller');await ready(`http://127.0.0.1:${port}/health`)
 await delay(1000)
 const nodeView=(await call(api+'/nodes',tokens.admin)).data
 assert.equal(nodeView.find(n=>n.id==='vos:member-observe').managed,false)
 const mapped=nodeView.find(n=>n.id===nodeId);assert.equal(mapped.vos_metrics.data.memory_used_mib,2048);assert.equal(mapped.vos_metrics.data.gpu_usage,null);assert.equal(mapped.vos_applications.data[0].installed_app_id,'local-instance');assert(!JSON.stringify(mapped).includes('owner_user_id'))
 console.log('PASS: VOS contract fixtures, units, observe-only discovery, app placement and metadata sanitization')
 const session={session_id:'protocol-session'};const beat=healthy=>call(api+'/workers/protocol-node/heartbeat',tokens.protocol,'POST',{...session,report:{memory_available_mib:8192,memory_total_mib:8192,executor_health:{'protocol-test':{healthy,observed_at:new Date().toISOString()}}}});const claim=async()=>{await beat(true);return (await call(api+'/workers/protocol-node/commands/claim',tokens.protocol,'POST',session)).data.commands[0]}
 const job=(await call(api+'/tasks',tokens.other,'POST',{protocol_version:1,app_task_id:'retry-proof',executor:{id:'protocol-test',contract_version:'1',task_type:'protocol.test'},parameters:{owner_id:'test-owner'},inputs:[]},{'Idempotency-Key':'retry-proof'})).data.task_id
 assert.deepEqual((await task(job)).wait_reasons,['WAIT_NODE_OFFLINE'])
 await call(api+'/nodes/protocol-node/drain',tokens.admin,'POST',{draining:true});assert.deepEqual((await task(job)).wait_reasons,['WAIT_NODE_DRAINING'])
 await call(api+'/nodes/protocol-node/drain',tokens.admin,'POST',{draining:false})
 await beat(false);assert.equal((await call(api+'/nodes',tokens.admin)).data.find(n=>n.id==='protocol-node').health_state,'degraded')
 assert.deepEqual((await task(job)).wait_reasons,['WAIT_EXECUTOR_UNHEALTHY'])
 assert.equal((await call(api+'/workers/protocol-node/commands/claim',tokens.protocol,'POST',session)).data.commands.length,0)
 await call(api+'/workers/protocol-node/heartbeat',tokens.protocol,'POST',{...session,report:{observed_at:new Date(Date.now()-60000).toISOString(),memory_available_mib:512}})
 assert.deepEqual((await task(job)).wait_reasons,['WAIT_TELEMETRY_STALE'])
 assert.equal((await call(api+'/workers/protocol-node/commands/claim',tokens.protocol,'POST',session)).data.commands.length,0)
 let cmd=await claim();assert.equal((await call(api+'/tasks/'+job,tokens.other,'DELETE')).status,409)
 const waiting=(await call(api+'/tasks',tokens.other,'POST',{protocol_version:1,app_task_id:'capacity-proof',executor:{id:'protocol-test',contract_version:'1',task_type:'protocol.test'},parameters:{},inputs:[]},{'Idempotency-Key':'capacity-proof'})).data.task_id
 assert.deepEqual((await task(waiting)).wait_reasons,['WAIT_CAPACITY'])
 await call(api+'/tasks/'+waiting+'/cancel',tokens.other,'POST',{});await call(api+'/tasks/'+waiting,tokens.other,'DELETE')
 const running=await call(api+'/attempts/'+cmd.attempt_id+'/report',tokens.protocol,'POST',{...session,fence:cmd.fence,status:'running',stopped:false,progress:{percent:0.5},stage:'decode',message:'segment 2/4'})
 const verifyBody={task_id:job,executor_id:'v-router.protocol.infer',owner_id:'test-owner',upstream_url:`http://127.0.0.1:${port+5}`,model_id:'cpu-test'}
 const linked=(body,key)=>call(api+'/delegations/verify',tokens.router,'POST',body,{'Idempotency-Key':key})
 assert.equal((await linked({...verifyBody,owner_id:'wrong-owner'},'wrong-owner')).status,403)
 assert.equal((await linked({...verifyBody,upstream_url:'http://127.0.0.1:1'},'wrong-url')).status,403)
 const child=await linked(verifyBody,'linked-child');assert.equal(child.status,200,JSON.stringify(child));const childId=child.data.permit_id
 assert.equal((await linked(verifyBody,'linked-child')).data.permit_id,childId)
 assert.equal((await linked(verifyBody,'over-budget')).status,429)
 assert.equal((await call(api+'/permits',tokens.other)).data.items.length,0)
 assert.equal((await call(api+'/permits',tokens.router)).data.items.find(p=>p.permit_id===childId).model_id,'cpu-test')
 assert.equal(running.status,200);assert.deepEqual((await task(job)).progress,{percent:0.5,stage:'decode',message:'segment 2/4'})
 const progressEvent=(await call(api+'/events',tokens.other)).data.items.find(e=>e.task_id===job&&e.progress?.stage==='decode')
 assert.equal(progressEvent.progress.percent,0.5)
 const failure=c=>call(api+'/attempts/'+c.attempt_id+'/report',tokens.protocol,'POST',{...session,fence:c.fence,status:'failed',stopped:true,result:{retryable:true,code:'TEST_TRANSIENT'}})
 await failure(cmd);assert.equal((await task(job)).status,'retry_wait');assert.equal((await call(api+'/reservations',tokens.admin)).data.find(r=>r.task_id===job).state,'suspect')
 assert.equal((await call(api+'/permits/'+childId+'/renew',tokens.router,'POST',{})).status,409)
 await beat(true);assert.equal((await call(api+'/workers/protocol-node/commands/claim',tokens.protocol,'POST',session)).data.commands.length,0)
 assert.equal((await call(api+'/permits/'+childId+'/release',tokens.router,'POST',{stopped:true,evidence:'test_child_stopped'})).status,200)
 await delay(2500);assert.equal((await call(api+'/reservations',tokens.admin)).data.length,0)
 console.log('PASS: delegated endpoint/owner/budget admission, scoped permit history and parent reservation retained until child stop')
 assert.deepEqual((await call(api+'/events',tokens.other)).data.items.find(e=>e.cursor===progressEvent.cursor).progress,progressEvent.progress)
 await until(job,['queued']);cmd=await claim();assert.equal(cmd.fence,2);await call(api+'/attempts/'+cmd.attempt_id+'/report',tokens.protocol,'POST',{...session,fence:cmd.fence,status:'failed',stopped:true,result:{retryable:false}})
 const retry=()=>call(api+'/tasks/'+job+'/retry',tokens.other,'POST',{}, {'Idempotency-Key':'manual-retry-once'});assert.equal((await retry()).status,200);assert.equal((await retry()).status,200);cmd=await claim();assert.equal(cmd.fence,3);await failure(cmd);assert.equal((await task(job)).status,'failed');assert.equal((await call(api+'/tasks/'+job+'/attempts',tokens.other)).data.length,3)
 assert.equal((await call(api+'/tasks/'+job,tokens.other,'DELETE')).status,200);assert.equal((await call(api+'/tasks/'+job,tokens.other)).status,404)
 console.log('PASS: progress snapshot events, bounded automatic backoff, idempotent manual retry, attempt history and stopped-only soft deletion')
 const lost=(await call(api+'/tasks',tokens.other,'POST',{protocol_version:1,app_task_id:'lease-proof',executor:{id:'protocol-test',contract_version:'1',task_type:'protocol.test'},parameters:{},inputs:[]},{'Idempotency-Key':'lease-proof'})).data.task_id
 const lostCommand=await claim()
 const held=(await call(api+'/tasks',tokens.other,'POST',{protocol_version:1,app_task_id:'held-proof',executor:{id:'protocol-test',contract_version:'1',task_type:'protocol.test'},parameters:{},inputs:[]},{'Idempotency-Key':'held-proof'})).data.task_id
 await until(lost,['recovering'],45000)
 assert.equal((await call(api+'/reservations',tokens.admin)).data.find(r=>r.task_id===lost).state,'suspect')
 assert.deepEqual((await task(held)).wait_reasons,['WAIT_NODE_OFFLINE'])
 assert.equal((await call(api+'/nodes',tokens.admin)).data.find(n=>n.id==='protocol-node').health_state,'unavailable')
 await beat(true);assert.equal((await call(api+'/workers/protocol-node/commands/claim',tokens.protocol,'POST',session)).data.commands[0].task_id,lost)
 assert.deepEqual((await task(held)).wait_reasons,['WAIT_CAPACITY'])
 await call(api+'/attempts/'+lostCommand.attempt_id+'/report',tokens.protocol,'POST',{...session,fence:lostCommand.fence,status:'failed',stopped:true,result:{retryable:false}})
 const next=await claim();assert.equal(next.task_id,held)
 await call(api+'/attempts/'+next.attempt_id+'/report',tokens.protocol,'POST',{...session,fence:next.fence,status:'failed',stopped:true,result:{retryable:false}})
 for(const id of [lost,held])await call(api+'/tasks/'+id,tokens.other,'DELETE')
 console.log('PASS: offline node retains suspect reservation; recovery waits for confirmed stop')
 await beat(true)
 const bridgeBody={executor_id:'v-router.protocol.infer',upstream_url:`http://127.0.0.1:${port+5}`,model_id:'cpu-test'}
 await call(api+'/workers/protocol-node/heartbeat',tokens.protocol,'POST',{...session,report:{observed_at:new Date().toISOString(),memory_available_mib:2048,memory_total_mib:8192}})
 assert.equal((await call(api+'/permits',tokens.router,'POST',bridgeBody,{'Idempotency-Key':'host-headroom'})).status,429)
 await call(api+'/workers/protocol-node/heartbeat',tokens.protocol,'POST',{...session,report:{observed_at:new Date().toISOString(),memory_available_mib:8192,memory_total_mib:8192,cpu_used_percent:95}})
 assert.equal((await call(api+'/permits',tokens.router,'POST',bridgeBody,{'Idempotency-Key':'cpu-pressure'})).status,429)
 assert.equal((await fetch(`http://127.0.0.1:${port}/health`)).status,200)
 await beat(true)
 console.log('PASS: system RAM headroom and CPU pressure deny compute while health remains available')
 const bridge=await call(api+'/permits',tokens.router,'POST',bridgeBody,{'Idempotency-Key':'bridge'});assert.equal(bridge.status,200,JSON.stringify(bridge))
 const queued=(await call(api+'/tasks',tokens.other,'POST',{protocol_version:1,app_task_id:'handoff-proof',executor:{id:'protocol-test',contract_version:'1',task_type:'protocol.test'},parameters:{},inputs:[]},{'Idempotency-Key':'handoff-proof'})).data.task_id
 assert.equal((await call(api+'/permits/'+bridge.data.permit_id+'/delegate',tokens.router,'POST',{task_id:queued,app_task_id:'wrong'})).status,403)
 assert.deepEqual((await task(queued)).wait_reasons,['WAIT_CAPACITY'])
 assert.equal((await call(api+'/permits/'+bridge.data.permit_id+'/delegate',tokens.router,'POST',{task_id:queued,app_task_id:'handoff-proof'})).status,200)
 const handoff=await claim();assert.equal(handoff.task_id,queued)
 await call(api+'/attempts/'+handoff.attempt_id+'/report',tokens.protocol,'POST',{...session,fence:handoff.fence,status:'succeeded',stopped:true,result:{}})
 await call(api+'/tasks/'+queued,tokens.other,'DELETE')
 console.log('PASS: bridge permit transfers only to verified durable task without double resource charge')
 if(process.env.STATUS_ONLY==='1'){
   file('status-evidence.json',{date:new Date().toISOString(),checks:['progress','queue','node_health','resource_baseline','recovery']})
   console.log('PASS: isolated controller protocol checks; GPU work skipped')
 }else{
 const installed=process.env.IMAGE_STUDIO_CONTAINER||'vos-app-com-ictrek-image-studio-01m4pwcrg1a4tpbv9wmf9jxrcw-image-studio-backend-amd-cuda-1'
 const mounts=JSON.parse(execFileSync('docker',['inspect',installed,'--format','{{json .Mounts}}']).toString());const model=mounts.find(m=>m.Destination==='/modelhub').Source
 mkdirSync(path.join(run,'image-data'),{recursive:true})
 const imageRepo=process.env.IMAGE_STUDIO_SOURCE||path.join(process.env.HOME,'dev/repos/image-studio')
 execFileSync('docker',['run','-d','--name',imageName,'--network','host','--gpus','device=0','--env-file',imgEnv,'-v',path.join(imageRepo,'app')+':/app/app:ro','-v',model+':/modelhub:ro','-v',path.join(run,'image-data')+':/data','swr.cn-southwest-2.myhuaweicloud.com/ictrek/image-studio-backend:amd_cu128_20261009','python3','-m','uvicorn','app.main:app','--host','127.0.0.1','--port',String(port+2)],{stdio:'pipe'})
 await ready(image+'/health')
 const create=()=>call(image+'/api/v1/generate',null,'POST',{model_key:'sd-turbo',prompt:'A small red ceramic cube on a white table',width:256,height:256,steps:4,cfg_scale:1,sampler:'euler',seed:42,batch_count:1},{'X-User-ID':'scheduler-e2e'})
 const j1=await create();assert.equal(j1.status,200,JSON.stringify(j1.data));
 const j2=await create();assert.equal(j2.status,200,JSON.stringify(j2.data));
 const list=(await call(api+'/tasks',tokens.admin)).data.items;assert.equal(list.length,2);assert(list.every(t=>t.status==='queued'))
 const ids=list.map(t=>t.task_id);const first=ids[1],second=ids[0]
 const body={protocol_version:1,app_task_id:j1.data.job_id,executor:{id:'image-studio.generate',contract_version:'1',task_type:'image.generate'},parameters:{user_key:j1.data.user_key,job_id:j1.data.job_id},inputs:[]}
 const dup=await call(api+'/tasks',tokens.service,'POST',body,{'Idempotency-Key':j1.data.job_id});assert.equal(dup.status,200)
 const conflict=await call(api+'/tasks',tokens.service,'POST',{...body,app_task_id:'different'},{'Idempotency-Key':j1.data.job_id});assert.equal(conflict.status,409)
 assert.equal((await call(api+'/tasks/'+first,tokens.other)).status,404)
 assert.equal((await call(api+'/nodes',tokens.service)).status,403)
 console.log('PASS: real Image Studio job submission, durable queue, idempotency and scope')
 agent=launch(['agent','--config',agentConfig],'agent')
 await until(first,['running','succeeded','failed'])
 const secondDuring=await task(second);assert.equal(secondDuring.status,'queued')
 const res=(await call(api+'/reservations',tokens.admin)).data;assert.equal(res.length,1);assert.equal(res[0].resources.gpu_slots,1)
 console.log('PASS: one-slot resource reservation keeps second task queued')
 const finished=await until(first,['succeeded','failed']);assert.equal(finished.status,'succeeded',JSON.stringify(finished.result))
 const finished2=await until(second,['succeeded','failed']);assert.equal(finished2.status,'succeeded',JSON.stringify(finished2.result))
 const output=path.join(run,'image-data/sessions',j1.data.user_key,j1.data.job_id,'output.png');assert(existsSync(output));const bytes=readFileSync(output);assert.equal(bytes.subarray(0,8).toString('hex'),'89504e470d0a1a0a')
 assert.equal((await call(api+'/reservations',tokens.admin)).data.length,0)
 console.log('PASS: sd-cli real CUDA generation produced PNG and released reservation')
 if(process.env.ROUTER_BINARY){
  const routerDir=path.join(run,'router');mkdirSync(routerDir,{recursive:true,mode:0o700})
  const routerConfig=file('router-scheduler.json',{url:api,token:tokens.router,require_model_policy:true,models:{'app-image-studio/sd-turbo':{mode:'delegated',executor_id:'v-router.image.bridge',max_input_bytes:1048576,limits:{width:256,height:256,steps:4,batch_count:1,n:1}}}})
  const fd=openSync(path.join(run,'router.log'),'a',0o600)
  const router=spawn(process.env.ROUTER_BINARY,['--addr',`127.0.0.1:${port+6}`,'--state',path.join(routerDir,'state.json'),'--image-studio',image,'--v-voice','','--v-music','','--ml-server','','--video-structure','','--video-structure-detect',''],{env:{...process.env,V_ROUTER_STATE_PATH:path.join(routerDir,'state.json'),V_ROUTER_SCHEDULER_CONFIG:routerConfig,V_ROUTER_SCHEDULER_REQUIRED:'true',V_ROUTER_AUTH_DISABLED:'true'},stdio:['ignore',fd,fd]});children.push(router)
  const routerUrl=`http://127.0.0.1:${port+6}`;await ready(routerUrl+'/healthz')
  for(let i=0;i<30;i++){const m=await call(routerUrl+'/v1/models');if((Array.isArray(m.data)?m.data:m.data.data)?.some(m=>m.id==='image-studio/sd-turbo'))break;await delay(1000)}
  const viaRouter=await call(routerUrl+'/v1/images/generations',null,'POST',{model:'image-studio/sd-turbo',prompt:'A red ceramic cube on a white table',size:'256x256',steps:4,n:1})
  assert.equal(viaRouter.status,202,JSON.stringify(viaRouter))
  let linkedTask;for(let i=0;i<30;i++){linkedTask=(await call(api+'/tasks',tokens.admin)).data.items.find(t=>t.app_task_id===viaRouter.data.id);if(linkedTask)break;await delay(500)}
  assert(linkedTask,'gateway creation did not create durable scheduler task')
  assert.equal((await until(linkedTask.task_id,['succeeded','failed'])).status,'succeeded')
  const history=(await call(api+'/permits',tokens.router)).data.items
  assert(history.some(p=>p.model_id==='sd-turbo'&&p.state==='released'&&p.evidence==='delegated_task:'+linkedTask.task_id),'gateway bridge ownership transfer missing')
  const routedJob=viaRouter.data.image_studio_job
  assert(existsSync(path.join(run,'image-data/sessions',routedJob.user_key,routedJob.job_id,'output.png')))
  assert.equal((await call(api+'/reservations',tokens.admin)).data.length,0)
  console.log('PASS: NEW V-Router -> NEW V-Scheduler -> real sd-turbo CUDA PNG, verified handoff and released resources')
  router.kill('SIGTERM');await new Promise(resolve=>router.once('exit',resolve))
 }

 controller.kill('SIGINT');await new Promise(r=>controller.once('exit',r));controller=launch(['controller','--config',controllerConfig,'--listen',`127.0.0.1:${port}`,'--ui-dir',path.join(build,'ui')],'controller-restarted');await ready(`http://127.0.0.1:${port}/health`);assert.equal((await task(first)).status,'succeeded')
 console.log('PASS: controller restart retains tasks and result')
 // A queued cancel must never launch or reserve compute.
 await call(api+'/nodes/'+nodeId+'/drain',tokens.admin,'POST',{draining:true});const j3=await create();assert.equal(j3.status,200)
 const all=(await call(api+'/tasks',tokens.admin)).data.items;const third=all[0].task_id;assert.equal((await task(third)).status,'queued');await call(api+'/tasks/'+third+'/cancel',tokens.service,'POST',{});assert.equal((await task(third)).status,'cancelled')
 assert.equal((await call(api+'/reservations',tokens.admin)).data.length,0)
 console.log('PASS: drain and queued cancellation do not allocate compute')
 await call(api+'/nodes/'+nodeId+'/drain',tokens.admin,'POST',{draining:false})
 const permit=await call(api+'/permits',tokens.router,'POST',{executor_id:'v-router.local.infer',upstream_url:`http://127.0.0.1:${port+4}`,model_id:'test-model'},{'Idempotency-Key':'online-001'});assert.equal(permit.status,200);const pid=permit.data.permit_id
 const jp=await create();assert.equal(jp.status,200);const blocked=(await call(api+'/tasks',tokens.admin)).data.items[0].task_id;await delay(4000);assert.equal((await task(blocked)).status,'queued')
 await call(api+'/permits/'+pid+'/release',tokens.router,'POST',{stopped:false,evidence:'e2e_unknown'});assert.equal((await call(api+'/reservations',tokens.admin)).data[0].state,'suspect');await delay(2500);assert.equal((await task(blocked)).status,'queued')
 await call(api+'/permits/'+pid+'/confirm-stopped',tokens.admin,'POST',{stopped:true,evidence:'e2e_no_compute_started'});assert.equal((await until(blocked,['succeeded','failed'])).status,'succeeded')
 console.log('PASS: online permit and image tasks share capacity; unknown completion retains resource')
 const longJob=()=>call(image+'/api/v1/generate',null,'POST',{model_key:'sd-turbo',prompt:'A small green ceramic cube',width:512,height:512,steps:120,cfg_scale:1,sampler:'euler',seed:17,batch_count:1},{'X-User-ID':'scheduler-e2e'})
 const j4=await longJob();assert.equal(j4.status,200);const fourth=(await call(api+'/tasks',tokens.admin)).data.items[0].task_id;await until(fourth,['running']);await call(api+'/tasks/'+fourth+'/cancel',tokens.service,'POST',{});assert.equal((await until(fourth,['cancelled','succeeded','failed'])).status,'cancelled');assert.equal((await call(api+'/reservations',tokens.admin)).data.length,0)
 console.log('PASS: running cancellation confirms child stop before resource release')
 const j5=await longJob();assert.equal(j5.status,200);const fifth=(await call(api+'/tasks',tokens.admin)).data.items[0].task_id;await until(fifth,['running']);sourceFailed=true;agent.kill('SIGKILL');await new Promise(r=>agent.once('exit',r));await delay(34000);assert.equal((await task(fifth)).status,'recovering');assert.equal((await call(api+'/reservations',tokens.admin)).data.length,1)
 agent=launch(['agent','--config',agentConfig],'agent-restarted');await until(fifth,['cancelled','failed','succeeded']);assert.equal((await task(fifth)).fence,1);assert.equal((await call(api+'/reservations',tokens.admin)).data.length,0)
 const stale=(await call(api+'/nodes',tokens.admin)).data.find(n=>n.id===nodeId);assert.equal(stale.vos_metrics.fresh,false);assert.equal(stale.vos_metrics.data.memory_used_mib,2048)
 console.log('PASS: VOS source outage retains last observation and marks it stale')
 console.log('PASS: Agent crash, expired lease reservation retention and journal reconciliation without replay')
 file('evidence.json',{date:new Date().toISOString(),first,second,third,image_job_id:j1.data.job_id,png_bytes:bytes.length,checks:['real_cuda_png','queue','reservation','idempotency','scope','restart','drain','queued_cancel','running_cancel','agent_crash_watchdog','journal_recovery','shared_online_permit','suspect_permit','bounded_retry','manual_retry_idempotency','attempt_history','stopped_only_soft_delete','vos_inventory_contract_fixture','vos_app_placement_contract_fixture','vos_source_outage_cache']})
 console.log('Evidence: '+run)
 if(process.env.KEEP_TEST_SERVICES==='1'){console.log('Test services retained for UI validation; stop with recorded PIDs/container names.');file('services.json',{controller_pid:controller.pid,agent_pid:agent.pid,dbName,imageName});for(const p of children)p.unref();process.exit(0)}
 }
}finally{
 vos.close()
 if(process.env.KEEP_TEST_SERVICES!=='1'){for(const p of children)p.kill('SIGTERM');for(const name of [imageName,dbName]){try{execFileSync('docker',['rm','-f',name],{stdio:'pipe'})}catch{}}}
}
