// Run on an authorized Linux build host after rsync/build. Uses only core PostgreSQL.
import {spawn,execFileSync} from 'node:child_process'
import {randomBytes} from 'node:crypto'
import {mkdirSync,writeFileSync,readFileSync,openSync,cpSync} from 'node:fs'
import path from 'node:path'
import assert from 'node:assert/strict'
const build=process.env.SCHEDULER_BUILD||path.join(process.env.HOME,'dev/build/V-Scheduler')
const run=path.join(build,'statistics-'+Date.now());mkdirSync(run,{recursive:true,mode:0o700})
const port=Number(process.env.SCHEDULER_TEST_BASE_PORT||38780)
assert(Number.isInteger(port)&&port>1024&&port<65534)
const api=`http://127.0.0.1:${port}/api/v1`,network='v-scheduler-statistics-'+process.pid,db=network+'-pg',legacy=network+'-legacy'
const tokens=Object.fromEntries(['admin','alpha','beta','db','uiworker'].map(k=>[k,randomBytes(32).toString('hex')]))
const resources={cpu_millicores:100,memory_mib:128,gpu_slots:0,gpu_memory_mib:0}
function file(name,value){const p=path.join(run,name);writeFileSync(p,typeof value==='string'?value:JSON.stringify(value),{mode:0o600});return p}
function docker(args,input){return execFileSync('docker',args,{input,encoding:'utf8',stdio:['pipe','pipe','pipe']})}
const installed=docker(['ps','--filter','name=vos-app-com-ictrek-v-scheduler','--format','{{.Names}}']).trim().split('\n').find(n=>n.includes('controller-'))
const legacyImage=process.env.SCHEDULER_LEGACY_IMAGE||JSON.parse(docker(['inspect',installed]))[0].Image
const cfg={principals:[{role:'admin',app_id:'admin',token:tokens.admin},{role:'submitter',app_id:'alpha',token:tokens.alpha,delegated_executors:['alpha-async']},{role:'submitter',app_id:'beta',token:tokens.beta}],nodes:[{id:'statistics-node',capacities:{cpu_millicores:8000,memory_mib:16384,gpu_slots:0,gpu_memory_mib:0}}],executors:['alpha','beta'].map(app=>({id:app+'-model',app_id:app,node_id:'statistics-node',task_type:'model.infer',contract_version:'1',resources,upstream_urls:['http://127.0.0.1:39999']})),local_agent:{node_id:'statistics-node',automatic_capacity:false,executors:[]}}
cfg.executors.push({id:'alpha-async',app_id:'alpha',node_id:'statistics-node',task_type:'test.generate',contract_version:'1',resources})
cfg.principals.push({role:'worker',app_id:'timeline-agent',node_id:'timeline-node',token:tokens.uiworker})
cfg.nodes.push({id:'timeline-node',capacities:{cpu_millicores:4000,memory_mib:8192,gpu_slots:0,gpu_memory_mib:0}})
cfg.executors.push({id:'alpha-timeline',app_id:'alpha',node_id:'timeline-node',task_type:'test.timeline',contract_version:'1',resources,retry_mode:'idempotent',max_attempts:3})
const configDir=path.join(run,'config');mkdirSync(configDir,{mode:0o700});const configFile=file('config/controller.json',cfg)
const pgEnv=file('pg.env',`POSTGRES_USER=postgres\nPOSTGRES_PASSWORD=${tokens.db}\nPOSTGRES_DB=statistics\n`)
const legacyEnv=file('legacy.env',`SCHEDULER_DATABASE_URL=postgres://scheduler:${tokens.db}@pg:5432/statistics\nSCHEDULER_EMBEDDED_AGENT=false\n`)
const delay=ms=>new Promise(resolve=>setTimeout(resolve,ms))
async function request(url,token,method='GET',body,extra={}){const r=await fetch(url,{method,headers:{...(token?{Authorization:'Bearer '+token}:{}),'Content-Type':'application/json',...extra},body:body===undefined?undefined:JSON.stringify(body)});return {status:r.status,...await r.json()}}
const call=(endpoint,token=tokens.admin,method='GET',body,extra={})=>request(api+endpoint,token,method,body,extra)
function sql(text){return docker(['exec','-i',db,'psql','-U','postgres','-d','statistics','-v','ON_ERROR_STOP=1','-At'],text)}
async function ready(url){let last='';for(let i=0;i<100;i++){try{const r=await fetch(url,{signal:AbortSignal.timeout(2000)});if(r.ok)return;last='HTTP '+r.status}catch(e){last=e.cause?.code||e.name}await delay(200)}throw Error('service did not become ready: '+last)}
const checks=[];let controller
function pass(text){checks.push(text);console.log('PASS: '+text)}
function near(actual,expected){assert(Math.abs(actual-expected)<.001,`${actual} != ${expected}`)}
function seedTask(id,app,created,assigned,stopped,status){return `INSERT INTO vrs_tasks(id,app,idem,hash,body,status,node,created_at,updated_at) VALUES('${id}','${app}','${id}','test','{}','${status}','statistics-node',now()+interval '${created} seconds',now()+interval '${stopped} seconds');
INSERT INTO vrs_reservations(attempt,task,node,resources,state,created_at,stopped_at) VALUES('${id}-attempt','${id}','statistics-node','{}','released',now()+interval '${assigned} seconds',now()+interval '${stopped} seconds');
INSERT INTO vrs_attempts(id,task,fence,node,status,created_at,stopped_at) VALUES('${id}-attempt','${id}',1,'statistics-node','${status}',now()+interval '${assigned} seconds',now()+interval '${stopped} seconds');`}
try{
 docker(['network','create','--internal',network])
 docker(['run','-d','--name',db,'--network',network,'--network-alias','pg','--runtime','runc','--memory','256m','--tmpfs','/var/lib/postgresql:rw,size=256m','--env-file',pgEnv,'registry.ictrek.internal/library/postgres:18'])
 for(let i=0;i<100;i++){try{docker(['exec',db,'pg_isready','-h','127.0.0.1','-U','postgres']);break}catch{await delay(200)}}
 sql(`CREATE ROLE scheduler LOGIN PASSWORD '${tokens.db}'; ALTER DATABASE statistics OWNER TO scheduler;`)
 // Apply the actual previous release before upgrading its database. The old
 // release must not already contain this migration.
 docker(['run','-d','--name',legacy,'--network',network,'--runtime','runc','--memory','512m','--env-file',legacyEnv,'-v',`${configDir}:/config`,legacyImage,'controller','--config','/config/controller.json','--listen','0.0.0.0:38120'])
 const legacyIP=Object.values(JSON.parse(docker(['inspect',legacy]))[0].NetworkSettings.Networks)[0].IPAddress
 await ready(`http://${legacyIP}:38120/health`)
 assert.equal(sql("SELECT count(*) FROM information_schema.columns WHERE table_name='vrs_permits' AND column_name='released_at';").trim(),'0','use the previous release image for migration coverage')
 docker(['rm','-f',legacy])
 sql("INSERT INTO vrs_permits(id,app,idem,executor,node,resources,state,evidence,created_at,lease_until) VALUES('legacy-release','alpha','legacy-release','alpha-model','statistics-node','{}','released','legacy_stop',now()-interval '2 hours',now()+interval '1 hour');")
 const dbIP=Object.values(JSON.parse(docker(['inspect',db]))[0].NetworkSettings.Networks)[0].IPAddress
 const ui=path.join(run,'ui');cpSync(process.env.SCHEDULER_UI_DIR||path.join(build,'ui-verify/dist'),ui,{recursive:true})
 const index=path.join(ui,'index.html');writeFileSync(index,readFileSync(index,'utf8').replace('<head>',`<head><script>window.__VOS_ACCESS_TOKEN__=${JSON.stringify(tokens.admin)};</script>`),{mode:0o600})
 const log=openSync(path.join(run,'controller.log'),'a',0o600)
 controller=spawn(process.env.SCHEDULER_BINARY||path.join(build,'cargo/debug/v-scheduler'),['controller','--config',configFile,'--listen',`127.0.0.1:${port}`,'--ui-dir',ui],{env:{...process.env,SCHEDULER_DATABASE_URL:`postgres://scheduler:${tokens.db}@${dbIP}:5432/statistics`,SCHEDULER_EMBEDDED_AGENT:'true'},stdio:['ignore',log,log]})
 await ready(`http://127.0.0.1:${port}/health`)
 for(let i=0;i<50;i++){const nodes=(await call('/nodes')).data;if(nodes?.[0]?.health_state==='ready'&&nodes[0].report.cpu_used_percent!=null)break;await delay(200)}
 assert.equal((await call('/permits',tokens.alpha)).data.items.find(p=>p.permit_id==='legacy-release').released_at,null)
 assert.equal((await call('/statistics',tokens.alpha)).data.permits.occupancy.samples,0)
 assert.deepEqual(sql("SELECT extname FROM pg_extension ORDER BY extname;").trim().split('\n'),['plpgsql'])
 pass('previous-release database migration keeps undated history unknown; only core PostgreSQL is used')
 let text='BEGIN;'+seedTask('alpha-success','alpha',-60,-50,-40,'succeeded')+seedTask('alpha-failed','alpha',-55,-50,-30,'failed')+seedTask('alpha-cancelled','alpha',-70,-45,-15,'cancelled')+seedTask('beta-failed','beta',-500,-400,-200,'failed')+seedTask('alpha-future','alpha',100,120,140,'failed')+seedTask('alpha-invalid-clock','alpha',-70,-80,-90,'failed')+seedTask('alpha-old','alpha',-300000,-299000,-298000,'failed')
 text+="INSERT INTO vrs_reservations(attempt,task,node,resources,state,created_at,stopped_at) VALUES('alpha-retry','alpha-success','statistics-node','{}','released',now()-interval '35 seconds',now()-interval '25 seconds'); INSERT INTO vrs_attempts(id,task,fence,node,status,created_at,stopped_at) VALUES('alpha-retry','alpha-success',2,'statistics-node','succeeded',now()-interval '35 seconds',now()-interval '25 seconds'); UPDATE vrs_tasks SET deleted_at=now() WHERE id='alpha-failed';"
 text+="INSERT INTO vrs_permits(id,app,idem,executor,node,resources,state,created_at,released_at) VALUES('alpha-timed','alpha','alpha-timed','alpha-model','statistics-node','{}','released',now()-interval '14 seconds',now()-interval '10 seconds'),('beta-timed','beta','beta-timed','beta-model','statistics-node','{}','released',now()-interval '200 seconds',now()-interval '100 seconds');COMMIT;"
 sql(text)
 const alpha=(await call('/statistics',tokens.alpha)).data
 assert.equal(alpha.first_queue_wait.samples,3);near(alpha.first_queue_wait.p50_ms,10000);assert.equal(alpha.first_queue_wait.p95_ms,null)
 assert.equal(alpha.attempts.succeeded,2);assert.equal(alpha.attempts.failed,1);assert.equal(alpha.attempts.cancelled,1);near(alpha.attempts.failure_rate,1/3);near(alpha.attempts.occupancy.p50_ms,15000)
 assert.equal(alpha.permits.occupancy.samples,1);near(alpha.permits.occupancy.p50_ms,4000)
 const beta=(await call('/statistics',tokens.beta)).data;assert.equal(beta.attempts.failed,1);assert.equal(beta.first_queue_wait.samples,1);near(beta.permits.occupancy.p50_ms,100000)
 assert.equal((await call('/statistics?app_id=beta',tokens.alpha)).data.attempts.failed,1)
 const worker=JSON.parse(readFileSync(configFile,'utf8')).principals.find(p=>p.role==='worker').token
 assert.equal((await call('/statistics','')).status,401);assert.equal((await call('/statistics',worker)).status,403)
 pass('queue and occupancy percentiles use real timestamps; retries count once for first wait; cancelled/invalid/old/future samples and app scope are correct')
 const history=(await call('/permits?state=released&limit=1',tokens.alpha)).data;assert.equal(history.items.length,1);assert.equal(history.items[0].app_id,'alpha');assert.equal(history.items[0].state,'released');assert.equal(history.next_cursor,'1')
 assert.equal((await call('/permits?state=invalid',tokens.alpha)).status,422)
 const acquired=await call('/permits',tokens.alpha,'POST',{executor_id:'alpha-model',upstream_url:'http://127.0.0.1:39999',model_id:'statistics-model'},{'Idempotency-Key':'statistics-release'});assert.equal(acquired.status,200);const id=acquired.data.permit_id
 assert.equal((await call(`/permits/${id}/renew`,tokens.alpha,'POST',{})).status,200)
 await call(`/permits/${id}/release`,tokens.alpha,'POST',{stopped:false,evidence:'statistics_unknown'})
 let record=(await call('/permits?state=suspect',tokens.alpha)).data.items.find(p=>p.permit_id===id);assert(record);assert.equal(record.released_at,null)
 await call(`/permits/${id}/confirm-stopped`,tokens.admin,'POST',{stopped:true,evidence:'test_did_not_start_compute'})
 record=(await call('/permits',tokens.alpha)).data.items.find(p=>p.permit_id===id);const releasedAt=record.released_at;assert(releasedAt);assert(Date.parse(releasedAt)<Date.parse(record.lease_until))
 await delay(10);await call(`/permits/${id}/release`,tokens.alpha,'POST',{stopped:true,evidence:'idempotent_stop'})
 assert.equal((await call('/permits',tokens.alpha)).data.items.find(p=>p.permit_id===id).released_at,releasedAt)
 pass('permit filters paginate within app scope; suspect has no stop time; renewals and repeated releases cannot invent or move stop timestamps')
 const bridge=await call('/permits',tokens.alpha,'POST',{executor_id:'alpha-model',upstream_url:'http://127.0.0.1:39999',model_id:'statistics-handoff'},{'Idempotency-Key':'statistics-handoff-permit'});assert.equal(bridge.status,200)
 const task=await call('/tasks',tokens.alpha,'POST',{protocol_version:1,app_task_id:'statistics-handoff',executor:{id:'alpha-async',contract_version:'1',task_type:'test.generate'},parameters:{},inputs:[]},{'Idempotency-Key':'statistics-handoff'});assert.equal(task.status,202)
 const handoff=()=>call(`/permits/${bridge.data.permit_id}/delegate`,tokens.alpha,'POST',{task_id:task.data.task_id,app_task_id:'statistics-handoff'})
 assert.equal((await handoff()).data.state,'delegated')
 const firstStop=(await call('/permits',tokens.alpha)).data.items.find(p=>p.permit_id===bridge.data.permit_id).released_at;assert(firstStop)
 await handoff();assert.equal((await call('/permits',tokens.alpha)).data.items.find(p=>p.permit_id===bridge.data.permit_id).released_at,firstStop)
 sql(`UPDATE vrs_permits SET released_at=NULL WHERE id='${bridge.data.permit_id}';`);await handoff();assert.equal((await call('/permits',tokens.alpha)).data.items.find(p=>p.permit_id===bridge.data.permit_id).released_at,null)
 pass('handoff records the first release exactly once and never backfills legacy releases on replay')
 sql('BEGIN;'+Array.from({length:17},(_,i)=>seedTask('alpha-sample-'+i,'alpha',-30,-28,-25,'succeeded')).join('')+'COMMIT;')
 const enough=(await call('/statistics',tokens.alpha)).data;assert.equal(enough.first_queue_wait.samples,20);near(enough.first_queue_wait.p50_ms,2000);near(enough.first_queue_wait.p95_ms,10750);assert.equal(enough.attempts.occupancy.samples,21);near(enough.attempts.occupancy.p95_ms,20000);near(enough.attempts.failure_rate,.05)
 assert.equal((await call('/statistics')).data.attempts.occupancy.samples,22)
 pass('P95 becomes available only at the stated sample threshold; admin and service aggregates remain correctly scoped')
 sql("INSERT INTO vrs_events(task,app,status,version,progress,created_at) SELECT 'alpha-success','alpha','running',i,jsonb_build_object('percent',i/100.0,'stage','decode','message','timeline checkpoint '||i),now()-interval '1 minute'+i*interval '1 second' FROM generate_series(1,55) i;")
 const timeline=(await call('/tasks/alpha-success/events?limit=50',tokens.alpha)).data
 assert.equal(timeline.items.length,50);assert.equal(timeline.items[0].version,55);assert.equal(timeline.items[0].progress.stage,'decode');assert(timeline.items[0].created_at)
 const older=(await call('/tasks/alpha-success/events?limit=50&cursor='+timeline.next_cursor,tokens.alpha)).data
 assert.equal(older.items.length,5);assert.equal(older.items[0].version,5);assert.equal(older.next_cursor,null)
 assert.equal((await call('/tasks/alpha-success/events',tokens.beta)).status,404)
 assert.equal((await call('/tasks/alpha-success/events',worker)).status,403)
 assert.equal((await call('/tasks/alpha-success/events','')).status,401)
 assert.equal((await call('/tasks/alpha-success/events?cursor=-1',tokens.alpha)).status,422)
 const retained=(await call('/tasks/alpha-success/events?limit=50',tokens.alpha)).data
 assert.deepEqual(retained.items,timeline.items)
 pass('task timeline keeps timestamped progress snapshots, paginates newest first and enforces task visibility')
 // Leave representative active/suspect rows for browser QA, without starting compute.
 sql("INSERT INTO vrs_permits(id,app,idem,executor,node,resources,state,metadata) VALUES('ui-active','alpha','ui-active','alpha-model','statistics-node','{\"cpu_millicores\":100,\"memory_mib\":128,\"gpu_slots\":0,\"gpu_memory_mib\":0}','active','{\"model_id\":\"statistics-live-model\"}'),('ui-suspect','alpha','ui-suspect','alpha-model','statistics-node','{\"cpu_millicores\":0,\"memory_mib\":0,\"gpu_slots\":0,\"gpu_memory_mib\":0}','suspect','{\"model_id\":\"statistics-unconfirmed-model\"}');")
 // Complete zero budgets and long-lived UI fixtures keep the unrelated
 // resource panels well-formed while browser pagination is exercised.
 sql(`UPDATE vrs_permits SET resources='${JSON.stringify({...resources,cpu_millicores:0,memory_mib:0})}' WHERE resources='{}';
 UPDATE vrs_reservations SET resources='${JSON.stringify({...resources,cpu_millicores:0,memory_mib:0})}' WHERE resources='{}';
 UPDATE vrs_permits SET lease_until=now()+interval '1 hour' WHERE id='ui-active';
 INSERT INTO vrs_permits(id,app,idem,executor,node,resources,state,metadata)
 SELECT 'ui-page-'||i,'alpha','ui-page-'||i,'alpha-model','statistics-node','${JSON.stringify({...resources,cpu_millicores:0,memory_mib:0})}','released','{"model_id":"pagination-test"}' FROM generate_series(1,51) i;`)
 // A manual, CPU-only protocol worker lets browser QA drive real state
 // transitions without launching a model or touching any installed application.
 const uiTask=await call('/tasks',tokens.alpha,'POST',{protocol_version:1,app_task_id:'experience-live-task',executor:{id:'alpha-timeline',contract_version:'1',task_type:'test.timeline'},parameters:{},inputs:[]},{'Idempotency-Key':'experience-live-task'})
 assert.equal(uiTask.status,202)
 file('ui-task.json',{task_id:uiTask.data.task_id})
 file('evidence.json',{date:new Date().toISOString(),checks,alpha_statistics:enough,ui_url:`http://127.0.0.1:${port}/`})
 file('services.json',{pid:process.pid,controller_pid:controller.pid,db,legacy,network,port})
 console.log('Evidence: '+run)
 if(process.env.KEEP_TEST_SERVICES==='1'){console.log('Isolated services retained for browser QA.');await new Promise(resolve=>{process.once('SIGTERM',resolve);process.once('SIGINT',resolve)})}
}catch(error){
 for(const name of [legacy,db]){try{file(name+'.log',docker(['logs',name]))}catch{}}
 throw error
}finally{
 if(controller){controller.kill('SIGTERM');await new Promise(resolve=>{if(controller.exitCode!==null)resolve();else controller.once('exit',resolve)})}
 for(const name of [legacy,db]){try{docker(['rm','-f','-v',name])}catch{}}
 try{docker(['network','rm',network])}catch{}
}
