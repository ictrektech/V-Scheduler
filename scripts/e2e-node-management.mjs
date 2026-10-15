// Isolated protocol acceptance: real controller/Agents, PG17, previous release
// migration, ModelInfer-shaped inventory and a controlled CPU-only model stub.
// Four Agent identities run on one build host: this is not a four-Spark benchmark.
import assert from 'node:assert/strict'
import {spawn,execFileSync} from 'node:child_process'
import {randomBytes} from 'node:crypto'
import {mkdirSync,writeFileSync,readFileSync,openSync,cpSync} from 'node:fs'
import path from 'node:path'
import {createServer} from 'node:http'
import {networkInterfaces} from 'node:os'
process.umask(0o077)
const build=process.env.SCHEDULER_BUILD||path.join(process.env.HOME,'dev/build/V-Scheduler/node-management-020')
const run=path.join(build,'acceptance-'+Date.now());mkdirSync(run,{recursive:true,mode:0o700})
const prefix='vs020-'+Date.now(),network=prefix+'-net',db=prefix+'-pg',legacy=prefix+'-legacy',gateway=prefix+'-gateway'
const port=Number(process.env.SCHEDULER_TEST_BASE_PORT||38920),mockPort=port+1,gatewayPort=port+2
const binary=process.env.SCHEDULER_BINARY||path.join(build,'cargo/debug/v-scheduler')
const legacyImage=process.env.SCHEDULER_LEGACY_IMAGE||'swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-scheduler:amd_20261015'
const gatewayImage=process.env.SCHEDULER_GATEWAY_IMAGE||'swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-router-backend:amd_20261015'
const token=()=>randomBytes(32).toString('hex')
const keys={admin:token(),legacy:token(),router:token(),worker:token(),platform:token(),database:token(),gateway:token(),adapter:token()}
const base=`http://127.0.0.1:${port}/api/v1`,gate=`http://127.0.0.1:${gatewayPort}`
const host=Object.values(networkInterfaces()).flat().find(n=>n.family==='IPv4'&&!n.internal)?.address
assert(host,'A build-host IPv4 address is required for the isolated model endpoint')
const mockOrigin=`http://${host}:${mockPort}`,upstream=mockOrigin+'/v1'
const resource={cpu_millicores:100,memory_mib:128,gpu_slots:0,gpu_memory_mib:0}
const capacity={cpu_millicores:4000,memory_mib:8192,gpu_slots:0,gpu_memory_mib:0}
const sourceNodes=Array.from({length:4},(_,i)=>({id:'source-spark-'+(i+1),name:'Spark protocol fixture '+(i+1),arch:'aarch64',gpu_model:'NVIDIA GB10',gpu_devices:[{index:0,name:'NVIDIA GB10',memory_total_mib:131072}],status:'online',busy:true,monitor_state:'deployed',ssh_user:'must-not-copy',has_key:true,monitor_last_error:'must-not-copy'}))
const configDir=path.join(run,'config');mkdirSync(configDir,{mode:0o700})
const configFile=path.join(configDir,'controller.json')
const configuration={principals:[{token:keys.admin,role:'admin',app_id:'admin'},{token:keys.legacy,role:'submitter',app_id:'legacy'},{token:keys.router,role:'submitter',app_id:'v-router'},{token:keys.worker,role:'worker',app_id:'control-agent',node_id:'control'}],nodes:[{id:'control',capacities:capacity}],executors:[{id:'legacy.task',app_id:'legacy',node_id:'control',task_type:'fixture.run',contract_version:'1',resources:resource}],local_agent:{node_id:'control',executors:[],automatic_capacity:false}}
function file(name,value){writeFileSync(path.join(run,name),typeof value==='string'?value:JSON.stringify(value,null,2),{mode:0o600})}
writeFileSync(configFile,JSON.stringify(configuration),{mode:0o600});file('postgres.env',`POSTGRES_USER=postgres\nPOSTGRES_DB=node_management\nPOSTGRES_PASSWORD=${keys.database}\n`)
const agents=[],checks=[],jobs=new Map();let controller,backendToken='',dbIP='',modelCalls=0,failInventory=false
const delay=ms=>new Promise(r=>setTimeout(r,ms))
function docker(args){return execFileSync('docker',args,{encoding:'utf8',stdio:['ignore','pipe','pipe']})}
function sql(query){return docker(['exec','-u','postgres',db,'psql','-U','postgres','-d','node_management','-Atc',query]).trim()}
function pass(message){checks.push(message);console.log('PASS '+message)}
async function request(endpoint,{auth=keys.admin,method='GET',body,headers={}}={}){
 const url=endpoint.startsWith('http')?endpoint:base+endpoint
 const response=await fetch(url,{method,headers:{...(auth?{Authorization:'Bearer '+auth}:{}),'Content-Type':'application/json',...headers},body:body===undefined?undefined:JSON.stringify(body)})
 const raw=await response.text();let payload;try{payload=JSON.parse(raw)}catch{payload={code:'NON_JSON'}}
 return {status:response.status,data:payload.data,code:payload.code,raw,headers:response.headers}
}
async function expect(endpoint,status,options={}){const result=await request(endpoint,options);assert.equal(result.status,status,endpoint+' code='+result.code);return result}
async function until(fn,message,attempts=120){for(let i=0;i<attempts;i++){const value=await fn();if(value)return value;await delay(250)}throw Error(message)}
async function stop(child){if(!child||child.exitCode!==null||child.signalCode!==null)return;child.kill('SIGINT');await Promise.race([new Promise(resolve=>child.once('exit',resolve)),delay(3000)]);if(child.exitCode===null&&child.signalCode===null){child.kill('SIGKILL');await new Promise(resolve=>child.once('exit',resolve))}}
function startController(){const fd=openSync(path.join(run,'controller.log'),'a',0o600);controller=spawn(binary,['controller','--config',configFile,'--listen',`127.0.0.1:${port}`,'--ui-dir',path.join(run,'ui')],{env:{...process.env,SCHEDULER_DATABASE_URL:`postgres://scheduler:${keys.database}@${dbIP}:5432/node_management`,SCHEDULER_EMBEDDED_AGENT:'true',SCHEDULER_VOS_URL:mockOrigin},stdio:['ignore',fd,fd]})}
function startAgent(index,config){const name='agent-'+index+'.json';file(name,config);const fd=openSync(path.join(run,'agent-'+index+'.log'),'a',0o600);const child=spawn(binary,['agent','--config',path.join(run,name)],{stdio:['ignore',fd,fd]});agents[index-1]=child;return child}
async function management(){return (await expect('/management',200)).data}
async function createInvitation(id,source=''){return (await expect('/enrollments',200,{method:'POST',body:{node_id:id,ttl_seconds:900,...(source?{discovery_source:'modelinfer',discovery_id:source}:{})}})).data}
async function approve(index){const id='spark-0'+index;const entry=(await management()).nodes.find(n=>n.id===id);return expect('/nodes/'+id+'/approve',200,{method:'POST',body:{version:entry.version,display_name:'Spark protocol fixture '+index,capacities:capacity,resident_baseline:{},safety_reserve:{}}})}
const asyncDefinition={id:'fixture.async',node_id:'spark-01',app_id:'dynamic-app',contract_version:'1',task_type:'fixture.run',resources:resource,upstream_urls:[],retry_mode:'idempotent',max_attempts:2}
const inferDefinition={id:'v-router.cluster.infer',node_id:'spark-01',app_id:'v-router',contract_version:'1',task_type:'model.infer',resources:resource,placements:[2,3,4].map(i=>({node_id:'spark-0'+i,resources:resource})),upstream_urls:[upstream],retry_mode:'none',max_attempts:1}
const agentConfigs=[]
const mock=createServer(async(req,res)=>{
 res.setHeader('Content-Type','application/json')
 try{
  if(req.url==='/v1000/user/check'){
   if(req.headers.authorization!=='Bearer '+keys.platform){res.writeHead(401);res.end('{}');return}
   res.end(JSON.stringify({code:0,data:{id:'fixture-administrator',role_code:'system_admin',enable:1,delete:0}}));return
  }
  if(req.url.startsWith('/v1000/app/list')){res.end(JSON.stringify({code:0,data:{total:1,data:[{app_id:'com.ictrek.modelinfer',installed_app_id:'fixture-install',state:'installed',version:'test'}]}}));return}
  if(req.url.startsWith('/v1000/app/detail')){res.end(JSON.stringify({code:0,data:{state:'installed',configs_json:{VOS_MODELINFER_PORT:String(mockPort),VOS_MODELINFER_URL_BASE:'/app/com.ictrek.modelinfer'}}}));return}
  if(req.url==='/v1000/cluster/status'){res.end(JSON.stringify({code:0,data:{initialized:false}}));return}
  if(/^\/app\/com\.ictrek\.modelinfer\/api\/nodes\/[^/]+\/metrics$/.test(req.url)){
   const now=Date.now()/1000;res.end(JSON.stringify({code:0,data:{groups:[{key:'cpu_usage',series:[{labels:{instance:'must-not-copy'},points:[[now,12.5]]}]},{key:'mem_usage',series:[{points:[[now,42]]}]},{key:'gpu_util',series:[{points:[[now,3]]}]}]}}));return
  }
  if(req.url.startsWith('/app/com.ictrek.modelinfer/api/nodes')){
   if(failInventory){res.writeHead(503);res.end('{}');return}
   res.end(JSON.stringify({code:0,data:{data:sourceNodes,total:4,pages:1,page_num:1,page_size:100}}));return
  }
  if(req.url==='/health'){res.end('{"healthy":true}');return}
  if(req.url==='/v1/models'){res.end(JSON.stringify({object:'list',data:[{id:'cluster-chat'}]}));return}
  if(req.url==='/v1/chat/completions'){
   let input='';for await(const chunk of req)input+=chunk
   const body=JSON.parse(input);assert.equal(body.model,'cluster-chat');modelCalls++
   assert(req.headers['x-vscheduler-permit-id']);assert(backendToken)
   const verified=await expect('/permits/'+req.headers['x-vscheduler-permit-id']+'/verify',200,{auth:backendToken,method:'POST',body:{executor_id:inferDefinition.id,upstream_url:upstream,model_id:'cluster-chat',execution_token:req.headers['x-vscheduler-execution-token'],execution_id:'model-fixture-'+modelCalls,claim:true}})
   assert.equal(verified.data.node_id,'spark-01')
   res.end(JSON.stringify({id:'fixture-answer',object:'chat.completion',choices:[{index:0,message:{role:'assistant',content:'Four-node admission verified'},finish_reason:'stop'}]}));return
  }
  const match=req.url.match(/^\/scheduler\/executions\/([^/]+)\/(start|inspect|cancel)$/)
  if(match){
   if(req.headers.authorization!=='Bearer '+keys.adapter){res.writeHead(401);res.end('{}');return}
   let text='';for await(const chunk of req)text+=chunk;const body=JSON.parse(text);const [_,id,action]=match
   let job=jobs.get(id)
   if(action==='start'&&!job){job={at:Date.now(),hold:body.parameters.hold===true,status:'running',starts:1};jobs.set(id,job)}
   if(action==='cancel'){job??={at:Date.now(),hold:false,status:'cancelled',starts:0};job.status='cancelled';jobs.set(id,job)}
   if(job&&!job.hold&&Date.now()-job.at>300&&job.status==='running')job.status='succeeded'
   res.end(JSON.stringify({status:job?.status||'unknown',stopped:!!job&&['succeeded','cancelled'].includes(job.status),result:{fixture:true},progress:{percent:job?.status==='succeeded'?1:0.5}}));return
  }
  res.writeHead(404);res.end('{}')
 }catch{res.writeHead(500);res.end('{"code":"FIXTURE_ASSERTION"}')}
})
await new Promise(resolve=>mock.listen(mockPort,'0.0.0.0',resolve))
try{
 docker(['network','create',network]);docker(['run','-d','--name',db,'--network',network,'--env-file',path.join(run,'postgres.env'),'postgres:17-bookworm'])
 await until(async()=>{try{return docker(['exec',db,'pg_isready','-h','127.0.0.1','-U','postgres','-d','node_management']).includes('accepting')}catch{return false}},'PG17 did not start')
 sql(`CREATE ROLE scheduler LOGIN PASSWORD '${keys.database}'; ALTER DATABASE node_management OWNER TO scheduler;`)
 dbIP=Object.values(JSON.parse(docker(['inspect',db]))[0].NetworkSettings.Networks)[0].IPAddress
 file('legacy.env',`SCHEDULER_DATABASE_URL=postgres://scheduler:${keys.database}@${dbIP}:5432/node_management\nSCHEDULER_EMBEDDED_AGENT=false\n`)
 docker(['run','-d','--name',legacy,'--network',network,'--runtime','runc','--env-file',path.join(run,'legacy.env'),'-v',configDir+':/config',legacyImage,'controller','--config','/config/controller.json','--listen','0.0.0.0:38120'])
 const legacyIP=Object.values(JSON.parse(docker(['inspect',legacy]))[0].NetworkSettings.Networks)[0].IPAddress
 const oldBase=`http://${legacyIP}:38120/api/v1`
 await until(async()=>{try{return (await fetch(`http://${legacyIP}:38120/health`)).ok}catch{return false}},'previous controller did not start')
 assert.equal(sql("SELECT count(*) FROM information_schema.columns WHERE table_name='vrs_nodes' AND column_name='management_enabled'"),'0')
 const oldTask=await expect(oldBase+'/tasks',202,{auth:keys.legacy,method:'POST',headers:{'Idempotency-Key':'legacy-history'},body:{protocol_version:1,app_task_id:'legacy-history',executor:{id:'legacy.task',contract_version:'1',task_type:'fixture.run'},parameters:{},inputs:[]}})
 await expect(oldBase+'/tasks/'+oldTask.data.task_id+'/cancel',200,{auth:keys.legacy,method:'POST',body:{}})
 docker(['rm','-f',legacy])
 const ui=path.join(run,'ui');cpSync(process.env.SCHEDULER_UI_DIR||path.join(build,'ui/dist'),ui,{recursive:true});const index=path.join(ui,'index.html');writeFileSync(index,readFileSync(index,'utf8').replace('<head>',`<head><script>window.__VOS_ACCESS_TOKEN__=${JSON.stringify(keys.platform)};</script>`),{mode:0o600})
 startController()
 await until(async()=>{try{return (await request('/discovery',{auth:''})).status===200}catch{return false}},'new controller did not start')
 assert.equal((await expect('/tasks/'+oldTask.data.task_id,200,{auth:keys.legacy})).data.status,'cancelled')
 pass('previous 0.0.19 database migrates and keeps its task history and service identities')
 let nodes=(await expect('/nodes',200,{auth:keys.platform})).data
 assert.equal(nodes.filter(n=>n.managed===false).length,4);assert.equal(nodes.filter(n=>n.managed!==false).length,1)
 assert(!JSON.stringify(nodes).includes('must-not-copy'));assert.equal((await management()).modelinfer_discovery.node_count,4)
 failInventory=true;sql("UPDATE vrs_observations SET observed_at=now()-interval '2 minutes' WHERE id='modelinfer:status'")
 nodes=(await expect('/nodes',200,{auth:keys.platform})).data;assert.equal(nodes.filter(n=>n.modelinfer_source?.fresh===false).length,4)
 failInventory=false;sql("UPDATE vrs_observations SET observed_at=now()-interval '2 minutes' WHERE id='modelinfer:status'");await expect('/nodes',200,{auth:keys.platform})
 pass('ModelInfer-shaped four-node inventory is read-only, redacted, fresh/stale aware and never grants execution')
 const plan={version:0,display_name:'VOS App source fixture',capacities:capacity,resident_baseline:{},safety_reserve:{}}
 const planned=await expect('/node-sources/modelinfer/source-spark-1',200,{method:'POST',body:plan});assert.equal(planned.data.execution_enabled,false)
 await expect('/node-sources/modelinfer/source-spark-1',409,{method:'POST',body:plan})
 await expect('/node-sources/modelinfer/missing-source',404,{method:'POST',body:plan})
 await expect('/node-sources/modelinfer/source-spark-1',403,{auth:keys.legacy,method:'POST',body:plan})
 await expect('/node-sources/modelinfer/source-spark-1/control',501,{method:'POST',body:{action:'pause'}})
 await expect('/node-sources/modelinfer/source-spark-1/control',422,{method:'POST',body:{action:'shell'}})
 let plannedNode=(await expect('/nodes',200)).data.find(n=>n.discovery_id==='source-spark-1')
 assert.equal(plannedNode.managed,false);assert.equal(plannedNode.capacities.gpu_slots,0);assert.equal(plannedNode.source_settings.version,1)
 assert.equal(plannedNode.source_control.execution_enabled,false)
 assert.equal(plannedNode.modelinfer_source.node.telemetry.measurements.cpu_usage[0].value,12.5)
 assert(!JSON.stringify(plannedNode).includes('must-not-copy'))
 pass('VOS App-only source management reuses existing monitor metrics; plans persist without granting execution, concurrent writes conflict and unverified control returns 501')

 await expect('/management',401,{auth:''});await expect('/enrollments',403,{auth:keys.legacy,method:'POST',body:{node_id:'forbidden'}})
 const expired=await createInvitation('expired-node');sql(`UPDATE vrs_enrollments SET expires_at=now()-interval '1 second' WHERE id='${expired.id}'`)
 const invalidRegistration={protocol_version:1,node_id:'expired-node',display_name:'Expired',instance_id:'expired-instance',boot_id:'fixture-boot',session_id:'expired-session',worker_token:token()}
 await expect('/workers/register',401,{auth:expired.enrollment_token,method:'POST',body:invalidRegistration})
 const revoked=await createInvitation('revoked-invite');await expect('/enrollments/'+revoked.id+'/revoke',200,{method:'POST',body:{}})
 await expect('/workers/register',401,{auth:revoked.enrollment_token,method:'POST',body:{...invalidRegistration,node_id:'revoked-invite'}})
 for(let i=1;i<=4;i++){
  const invite=await createInvitation('spark-0'+i,'source-spark-'+i)
  const cfg={controller:base,node_id:'spark-0'+i,display_name:'Spark protocol fixture '+i,enrollment_token:invite.enrollment_token,journal:path.join(run,'agent-'+i,'journal.json'),executors:i===1?[{id:asyncDefinition.id,url:mockOrigin,health_url:mockOrigin+'/health',token:keys.adapter}]:[],registrations:i===1?[asyncDefinition,inferDefinition]:[]}
  agentConfigs.push(cfg);startAgent(i,cfg)
 }
 await until(async()=>((await management()).nodes.filter(n=>n.state==='pending').length===4),'four Agents did not request enrollment')
 const firstIdentity=JSON.parse(readFileSync(path.join(run,'agent-1/journal.identity.json'),'utf8'));const firstJournal=JSON.parse(readFileSync(path.join(run,'agent-1/journal.json'),'utf8'))
 await expect('/workers/spark-01/heartbeat',401,{auth:firstIdentity.token,method:'POST',body:{session_id:firstJournal.session_id,report:{}}})
 const retry={protocol_version:1,node_id:'spark-01',display_name:'Spark protocol fixture 1',instance_id:firstJournal.session_id,boot_id:'retry-boot',session_id:firstJournal.session_id,worker_token:firstIdentity.token}
 await expect('/workers/register',200,{auth:agentConfigs[0].enrollment_token,method:'POST',body:retry})
 await expect('/workers/register',409,{auth:agentConfigs[0].enrollment_token,method:'POST',body:{...retry,worker_token:token()}})
 for(let i=1;i<=4;i++)await approve(i)
 await until(async()=>{const n=(await request('/nodes')).data;return n.filter(n=>n.managed!==false&&n.heartbeat&&Date.now()-Date.parse(n.heartbeat)<12000).length===5},'approved collectors did not report')
 nodes=(await expect('/nodes',200)).data;assert.equal(nodes.length,5);assert.equal(nodes.filter(n=>n.managed===false).length,0)
 await expect('/workers/spark-02/heartbeat',403,{auth:firstIdentity.token,method:'POST',body:{session_id:firstJournal.session_id,report:{}}})
 pass('real Agents durably enroll pending, retry idempotently, wait for approval and stay scoped to their own node')
 await until(async()=>((await management()).executors.filter(e=>e.state==='pending').length===2),'Agent executor proposals not received',160)
 for(const id of [asyncDefinition.id,inferDefinition.id]){const entry=(await management()).executors.find(e=>e.id===id);await expect('/executors/'+id,200,{method:'POST',body:{version:entry.version,enabled:true,definition:entry.definition}})}
 const identity=await expect('/service-identities',200,{method:'POST',body:{app_id:'dynamic-app'}});assert.equal(identity.headers.get('cache-control'),'no-store');const appToken=identity.data.token
 backendToken=(await expect('/service-identities',200,{method:'POST',body:{app_id:'vllm-backend',permit_executors:[inferDefinition.id]}})).data.token
 const overview=await management();assert(!JSON.stringify(overview).includes('token_hash'));for(const secret of [...Object.values(keys),firstIdentity.token,appToken,backendToken])assert(!JSON.stringify(overview).includes(secret))
 await expect('/management',403,{auth:appToken});await expect('/enrollments',403,{auth:appToken,method:'POST',body:{node_id:'no-admin'}})
 const taskBody={protocol_version:1,app_task_id:'dynamic-completion',executor:{id:asyncDefinition.id,contract_version:'1',task_type:'fixture.run'},parameters:{},inputs:[]}
 const submit=await expect('/tasks',202,{auth:appToken,method:'POST',body:taskBody,headers:{'Idempotency-Key':'dynamic-completion'}})
 await expect('/tasks/'+submit.data.task_id,404,{auth:keys.legacy})
 await until(async()=>((await request('/tasks/'+submit.data.task_id,{auth:appToken})).data?.status==='succeeded'),'dynamic task failed to complete')
 assert.equal(jobs.size,1);assert.equal([...jobs.values()][0].starts,1)
 const duplicate=await expect('/tasks',200,{auth:appToken,method:'POST',body:taskBody,headers:{'Idempotency-Key':'dynamic-completion'}});assert.equal(duplicate.data.task_id,submit.data.task_id)
 pass('approved dynamic executor runs a durable task exactly once through the real Agent and controlled adapter')
 const permitBody={executor_id:inferDefinition.id,upstream_url:upstream,model_id:'cluster-chat'}
 const acquire=key=>expect('/permits',200,{auth:keys.router,method:'POST',body:permitBody,headers:{'Idempotency-Key':key}})
 const held=await acquire('all-four-hold');let reserved=(await expect('/reservations',200)).data.filter(r=>r.attempt_id===held.data.permit_id);assert.equal(reserved.length,4);assert.equal(new Set(reserved.map(r=>r.node_id)).size,4)
 await expect('/nodes/spark-04/revoke',409,{method:'POST',body:{}})
 const peer=(await management()).nodes.find(n=>n.id==='spark-04')
 await expect('/nodes/spark-04',409,{method:'POST',body:{version:peer.version,display_name:peer.display_name,...peer.definition}})
 await expect('/permits/'+held.data.permit_id+'/delegate',409,{auth:keys.router,method:'POST',body:{task_id:submit.data.task_id,app_task_id:taskBody.app_task_id}})
 await expect('/permits/'+held.data.permit_id+'/release',200,{auth:keys.router,method:'POST',body:{stopped:true,evidence:'fixture_never_started_model'}})
 assert.equal((await expect('/reservations',200)).data.filter(r=>r.attempt_id===held.data.permit_id).length,0)
 // A single unavailable peer must leave no partial head or peer allocation.
 await expect('/nodes/spark-03/drain',200,{method:'POST',body:{draining:true}})
 const before=Number(sql('SELECT count(*) FROM vrs_permits'))
 await expect('/permits',429,{auth:keys.router,method:'POST',body:permitBody,headers:{'Idempotency-Key':'one-peer-draining'}})
 assert.equal(Number(sql('SELECT count(*) FROM vrs_permits')),before)
 await expect('/nodes/spark-03/drain',200,{method:'POST',body:{draining:false}})
 const suspect=await acquire('all-four-unknown');await expect('/permits/'+suspect.data.permit_id+'/release',200,{auth:keys.router,method:'POST',body:{stopped:false,evidence:'fixture_unknown_stop'}})
 assert.equal((await expect('/reservations',200)).data.filter(r=>r.attempt_id===suspect.data.permit_id&&r.state==='suspect').length,4)
 await expect('/permits/'+suspect.data.permit_id+'/confirm-stopped',200,{method:'POST',body:{stopped:true,evidence:'fixture_confirms_no_compute'}})
 pass('distributed inference reserves four separate pools atomically; one drained peer rejects all, unknown stop retains all, and peer edits/revocation cannot free held resources')
 const gatewayDir=path.join(run,'gateway');mkdirSync(gatewayDir,{mode:0o700})
 writeFileSync(path.join(gatewayDir,'scheduler.local.json'),JSON.stringify({url:base,token:keys.router,require_model_policy:true,models:{'registered-vllm-cluster/cluster-chat':{mode:'permit',executor_id:inferDefinition.id,completion:'synchronous',max_input_bytes:1048576,limits:{max_tokens:64,n:1}}}}),{mode:0o600})
 file('gateway.env',`V_ROUTER_CONTROLLER_URL=http://127.0.0.1:${port}\nV_ROUTER_ADDR=127.0.0.1:${gatewayPort}\nV_ROUTER_API_KEY=${keys.gateway}\nV_ROUTER_REQUIRE_KEY=true\nV_ROUTER_SCHEDULER_REQUIRED=true\nV_ROUTER_STATE_PATH=/data/state.json\nV_ROUTER_USAGE_PATH=/data/usage.jsonl\nV_ROUTER_MODEL_HUB_URL=\nV_ROUTER_IMAGE_STUDIO_URL=\nV_ROUTER_V_SUB_URLS=\nV_ROUTER_V_VOICE_URLS=\nV_ROUTER_V_MUSIC_URLS=\nV_ROUTER_ML_SERVER_URLS=\nV_ROUTER_VIDEO_STRUCTURE_URLS=\nV_ROUTER_VIDEO_STRUCTURE_DETECT_URLS=\n`)
 docker(['run','-d','--name',gateway,'--network','host','--env-file',path.join(run,'gateway.env'),'-v',gatewayDir+':/data',gatewayImage])
 await until(async()=>{try{return (await fetch(gate+'/healthz')).ok}catch{return false}},'gateway did not start')
 await expect(gate+'/api/v1/registrations',200,{auth:keys.gateway,method:'POST',body:{service_id:'vllm-cluster',base_url:upstream,protocol:'chat_completions',visibility:'public',model:'cluster-chat',model_kind:'chat',endpoints:['chat/completions'],ttl_seconds:3600}})
 const inference=()=>expect(gate+'/v1/chat/completions',200,{auth:keys.gateway,method:'POST',body:{model:'registered/cluster-chat',messages:[{role:'user',content:'fixture'}],max_tokens:64,n:1}})
 const answer=await inference();assert(answer.raw.includes('Four-node admission verified'));assert.equal(modelCalls,1)
 const peerHoldDefinition={...inferDefinition,id:'fixture.peer-hold',node_id:'spark-03',placements:[],resources:{...resource,cpu_millicores:4000}}
 await expect('/executors/'+peerHoldDefinition.id,200,{method:'POST',body:{version:0,enabled:true,definition:peerHoldDefinition}})
 const blocker=await expect('/permits',200,{auth:keys.router,method:'POST',body:{executor_id:peerHoldDefinition.id,upstream_url:upstream,model_id:'cluster-chat'},headers:{'Idempotency-Key':'busy-third-peer'}})
 await expect(gate+'/v1/chat/completions',429,{auth:keys.gateway,method:'POST',body:{model:'registered/cluster-chat',messages:[{role:'user',content:'fixture'}],max_tokens:64,n:1}});assert.equal(modelCalls,1)
 await expect('/permits/'+blocker.data.permit_id+'/release',200,{auth:keys.router,method:'POST',body:{stopped:true,evidence:'fixture_budget_only'}})
 pass('real unified gateway forwards only after all four nodes admit; a busy non-head peer never reaches the model; backend claims the same permit without reserving twice')
 // Persisted budgets and approved identities survive a controller restart.
 const nodeEntry=(await management()).nodes.find(n=>n.id==='spark-02')
 const edit={version:nodeEntry.version,display_name:nodeEntry.display_name,...nodeEntry.definition,capacities:{...capacity,cpu_millicores:3500}}
 await expect('/nodes/spark-02',200,{method:'POST',body:edit});await expect('/nodes/spark-02',409,{method:'POST',body:edit})
 const identityBefore=readFileSync(path.join(run,'agent-2/journal.identity.json'),'utf8')
 await stop(controller);startController();await until(async()=>{try{return (await request('/discovery',{auth:''})).status===200}catch{return false}},'controller restart failed')
 await until(async()=>((await request('/nodes')).data?.filter(n=>n.heartbeat&&Date.now()-Date.parse(n.heartbeat)<12000).length===5),'Agent sessions not restored')
 assert.equal((await management()).nodes.find(n=>n.id==='spark-02').definition.capacities.cpu_millicores,3500)
 assert.equal(readFileSync(path.join(run,'agent-2/journal.identity.json'),'utf8'),identityBefore)
 assert.equal((await expect('/tasks/'+submit.data.task_id,200,{auth:appToken})).data.status,'succeeded')
 await inference()
 assert.equal((await expect('/nodes',200)).data.find(n=>n.modelinfer_source?.node.id==='source-spark-1').source_settings.version,1)
 pass('approved nodes, source plans, scoped credentials, executor placements, explicit budgets and old/new history survive restart without duplicate execution')
 // A peer losing heartbeat quarantines the whole distributed permit.
 const uncertain=await acquire('lost-peer');agents[2].kill('SIGSTOP');sql("UPDATE vrs_nodes SET heartbeat=now()-interval '1 minute' WHERE id='spark-03'")
 await until(async()=>((await request('/permits',{auth:keys.router})).data.items.find(p=>p.permit_id===uncertain.data.permit_id)?.state==='suspect'),'lost peer did not quarantine permit')
 assert.equal((await expect('/reservations',200)).data.filter(r=>r.attempt_id===uncertain.data.permit_id&&r.state==='suspect').length,4)
 await expect('/permits',429,{auth:keys.router,method:'POST',body:permitBody,headers:{'Idempotency-Key':'stale-third-peer'}})
 await expect('/permits/'+uncertain.data.permit_id+'/confirm-stopped',200,{method:'POST',body:{stopped:true,evidence:'fixture_no_execution_started'}});agents[2].kill('SIGCONT')
 await until(async()=>{const n=(await request('/nodes')).data.find(n=>n.id==='spark-03');return n.heartbeat&&Date.now()-Date.parse(n.heartbeat)<12000},'peer did not recover')
 await expect('/service-identities/'+identity.data.id+'/revoke',200,{method:'POST',body:{}})
 await expect('/executors',401,{auth:appToken})
 // Leave the fourth node pending re-enrollment so browser QA can approve it.
 await expect('/nodes/spark-04/revoke',200,{method:'POST',body:{}})
 const oldFourth=JSON.parse(readFileSync(path.join(run,'agent-4/journal.identity.json'),'utf8')).token
 await stop(agents[3]);const invitation=await createInvitation('spark-04','source-spark-4');agentConfigs[3].enrollment_token=invitation.enrollment_token;startAgent(4,agentConfigs[3])
 await until(async()=>((await management()).nodes.find(n=>n.id==='spark-04')?.state==='pending'),'fourth node not pending reapproval')
 await expect('/workers/spark-04/heartbeat',401,{auth:oldFourth,method:'POST',body:{session_id:'old-session',report:{}}})
 pass('lost-peer reservations stay quarantined across every participant, revoked credentials fail, and re-enrollment keeps history while requiring fresh approval')
 file('evidence.json',{date:new Date().toISOString(),checks,real_hardware_scope:'Four isolated Agent identities on one AMD build host; model/ModelInfer are protocol fixtures, no production model or device changed.',ui_url:gate+'/#nodes'})
 file('services.json',{pid:process.pid,controller_pid:controller.pid,agent_pids:agents.map(p=>p.pid),db,gateway,network,port,mockPort,gatewayPort,run})
 file('fixture-access.json',{admin:keys.admin,platform:keys.platform,router:keys.router,backend:backendToken,gateway:keys.gateway})
 console.log('Evidence '+run)
 if(process.env.KEEP_TEST_SERVICES==='1'){console.log('Isolated fixture retained for UI QA.');await new Promise(resolve=>{process.once('SIGTERM',resolve);process.once('SIGINT',resolve)})}
}catch(error){file('failure.txt',error.stack||String(error));try{file('database.log',docker(['logs',db]))}catch{}throw error}
finally{
 for(const child of agents){if(child?.exitCode===null){try{child.kill('SIGCONT')}catch{}await stop(child)}}
 await stop(controller);await new Promise(resolve=>mock.close(resolve))
 for(const name of [gateway,legacy,db]){try{docker(['rm','-f','-v',name])}catch{}}
 try{docker(['network','rm',network])}catch{}
}
