#!/usr/bin/env python3
"""Application registration, telemetry gating and verified stop lifecycle with real Rust/PG.
Synthetic native peers run on a private Docker network. No installed data or weights.
"""
import concurrent.futures, datetime, json, os, pathlib, secrets, subprocess, threading, time, urllib.error, urllib.request, uuid
root=pathlib.Path(os.environ.get('SCHEDULER_TEST_BUILD_ROOT',str(pathlib.Path.home()/'dev/build/V-Scheduler/features-20261020')))
run=root/('management-'+uuid.uuid4().hex);run.mkdir(parents=True,mode=0o700)
repo=pathlib.Path(__file__).resolve().parents[1]
network='vs-management-'+uuid.uuid4().hex[:10];owned=[];stop=threading.Event();checks=[]
keys={k:secrets.token_hex(32) for k in ['db','admin','worker','gateway','batch','other']}
def docker(*args,input=None):return subprocess.check_output(['docker',*args],input=input,text=True,stderr=subprocess.PIPE).strip()
def private(name,value):
 p=run/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(value if isinstance(value,str) else json.dumps(value));p.chmod(0o600);return p
def launch(suffix,*args):
 name=network+'-'+suffix;owned.append(name);docker('run','-d','--name',name,'--network',network,*args);return name
def address(name,port):return 'http://'+json.loads(docker('inspect',name))[0]['NetworkSettings']['Networks'][network]['IPAddress']+':'+str(port)
def request(url,body=None,token=None,method=None,headers=None,timeout=15):
 req=urllib.request.Request(url,data=None if body is None else json.dumps(body).encode(),method=method or ('GET' if body is None else 'POST'),headers={'Content-Type':'application/json',**({'Authorization':'Bearer '+token} if token else {}),**(headers or {})})
 try:res=urllib.request.urlopen(req,timeout=timeout)
 except urllib.error.HTTPError as e:res=e
 with res:raw=res.read();status=res.status
 try:return status,json.loads(raw)
 except ValueError:return status,raw.decode()
def until(fn,label,seconds=35):
 deadline=time.monotonic()+seconds
 while time.monotonic()<deadline:
  try:
   result=fn()
   if result:return result
  except (OSError,ValueError,KeyError,subprocess.CalledProcessError):pass
  time.sleep(.1)
 raise AssertionError(label)
def check(ok,label):assert ok,label;checks.append(label);print('PASS:',label,flush=True)
def sql(text):return docker('exec','-i',pg,'psql','-U','postgres','-d','test','-At','-v','ON_ERROR_STOP=1',input=text)
fixture=private('fixture.mjs',r'''
import {createServer} from 'node:http';
let active=0,peak=0,calls=0,unloads=0,valid=true,loaded=new Set(),history=[],mutations=[];
const models=[{name:'chat:latest',size:1048576},{name:'other:latest',size:1048576}];
const handler=async(req,res)=>{res.setHeader('Content-Type','application/json');let text='';for await(const c of req)text+=c;const body=text?JSON.parse(text):{};
const reply=(value,status=200)=>{res.statusCode=status;res.end(JSON.stringify(value));};
if(req.url==='/api/tags')return reply({models});
if(req.url==='/api/pull'){
 mutations.push({kind:'pull',active,loaded:[...loaded]});
 await new Promise(r=>setTimeout(r,1500));
 const model=models.find(m=>m.name===body.model);if(model)model.size+=1048576;
 res.setHeader('Content-Type','application/x-ndjson');return res.end(JSON.stringify({status:'success'})+'\n');
}
if(req.url==='/api/delete'){
 mutations.push({kind:'delete',active,loaded:[...loaded]});
 const index=models.findIndex(m=>m.name===body.model);if(index>=0)models.splice(index,1);
 return reply({});
}
if(req.url==='/api/show')return reply({capabilities:['completion']});
if(req.url==='/v1/models')return reply({data:models.map(m=>({id:m.name}))});
if(req.url==='/api/version')return reply({version:'fixture'});
if(req.url==='/api/ps')return reply({models:[...loaded].map(name=>({name}))});
if(req.url==='/stats')return reply({active,peak,calls,unloads,loaded:[...loaded],history,mutations});
if(req.url==='/proof-mode'){valid=body.valid;return reply({});}
if(req.url==='/api/scheduler/confirm-stopped'){
 if(active)return reply({error:'active'},409);
 if(valid){unloads++;loaded.delete(body.model);}
 return reply({protocol_version:1,model:body.model,nonce:valid?body.nonce:'stale',stopped:valid,native_exit_barrier:valid});
}
if(req.url==='/v1000/user/check')return reply({code:0,data:{id:'fixture-user',role_code:'user',enable:1,delete:0}});
if(req.url==='/v1000/oauth2/userinfo')return reply({sub:req.headers.authorization?.endsWith('bob')?'bob':'alice',preferred_username:'fixture'});
if(req.url==='/v1/chat/completions'){
 if(!req.headers['x-vscheduler-permit-id'])return reply({error:'no permit'},500);
 active++;peak=Math.max(peak,active);calls++;loaded.add(body.model);history.push({model:body.model,permit:req.headers['x-vscheduler-permit-id'],keep_alive:body.keep_alive});
 if(body.messages?.[0]?.content==='hold')await new Promise(r=>setTimeout(r,3000));
 active--;if(body.keep_alive===0)loaded.delete(body.model);
 if(body.stream){res.setHeader('Content-Type','text/event-stream');return res.end('data: {"choices":[{"delta":{"content":"hello"}}]}\n\ndata: [DONE]\n\n');}
 return reply({choices:[{message:{role:'assistant',content:'hello'},finish_reason:'stop'}]});
}
reply({},404);};createServer(handler).listen(11434);createServer(handler).listen(11535);
''')
try:
 docker('network','create','--internal',network)
 pg=launch('pg','--env-file',str(private('pg.env',f'POSTGRES_USER=postgres\nPOSTGRES_DB=test\nPOSTGRES_PASSWORD={keys["db"]}\n')),os.environ.get('SCHEDULER_TEST_PG_IMAGE','postgres:18'))
 until(lambda:'accepting connections' in docker('exec',pg,'pg_isready','-U','postgres'),'PG not ready')
 peer=launch('peer','--network-alias','v-scheduler-ollama-qa','--network-alias','v-scheduler-ollama-embedding','--network-alias','v-scheduler-ollama-rerank','-v',f'{fixture}:/fixture.mjs:ro','--entrypoint','node',os.environ.get('SCHEDULER_TEST_NODE_IMAGE','node:24-bookworm-slim'),'/fixture.mjs')
 config=private('config/controller.json',{'principals':[{'token':keys['other'],'role':'submitter','app_id':'other'},{'token':keys['admin'],'role':'admin','app_id':'test'},{'token':keys['worker'],'role':'worker','node_id':'local','app_id':'worker'},{'token':keys['batch'],'role':'submitter','app_id':'batch'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':1000,'memory_mib':8192}}],'executors':[{'id':'batch.fixture','contract_version':'1','task_type':'batch.test','node_id':'local','app_id':'batch','resources':{'cpu_millicores':1000,'memory_mib':128},'retry_mode':'idempotent','max_attempts':3},{'id':'batch.legacy','contract_version':'1','task_type':'batch.test','node_id':'local','app_id':'batch','resources':{'cpu_millicores':1000,'memory_mib':128},'retry_mode':'none','max_attempts':1},{'id':'batch.infer','contract_version':'1','task_type':'model.infer','node_id':'local','app_id':'batch','upstream_urls':['http://fixture:9999/v1'],'resources':{'cpu_millicores':1000,'memory_mib':128}}],'local_agent':{'node_id':'local','automatic_capacity':False,'executors':[]}})
 data=run/'data';data.mkdir(mode=0o700)
 models=run/'models';(models/'registry').mkdir(parents=True,mode=0o700)
 private('models/registry/models.json',{'ollama://chat:latest':{'model_id':'ollama://chat:latest','name':'Chat','source':'ollama','status':'ready','size':1048576},'hf://fixture/weights':{'model_id':'hf://fixture/weights','name':'Selected weights','source':'hf','status':'ready','files':['weights.gguf'],'path':'/models/export/hf/fixture/weights/current'}})
 env={'SCHEDULER_DATABASE_URL':f'postgres://postgres:{keys["db"]}@{address(pg,5432)[7:]}/test','SCHEDULER_EMBEDDED_AGENT':'false','VOS_APP_VERSION':'feature-acceptance','SCHEDULER_GATEWAY_DATA':'/gateway-data','V_ROUTER_STATE_PATH':'/gateway-data/state.json','V_ROUTER_USAGE_PATH':'/gateway-data/usage.jsonl','V_ROUTER_ADDR':':8080','V_ROUTER_REQUIRE_KEY':'true','V_ROUTER_API_KEY':keys['gateway'],'V_ROUTER_SCHEDULER_REQUIRED':'true','V_ROUTER_DISCOVERY_INTERVAL':'1s','SCHEDULER_EMBEDDED_MODEL_HUB':'true','MODEL_HUB_LISTEN':'0.0.0.0:6005','MODELS_BASE_DIR':'/models','V_SCHEDULER_MODEL_MANAGEMENT_URL':'http://v-scheduler:6005','SCHEDULER_VOS_URL':address(peer,11434)}
 for key in ['MODEL_HUB_URL','IMAGE_STUDIO_URL','V_VOICE_URLS','V_MUSIC_URLS','V_SUB_URLS','ML_SERVER_URLS','VIDEO_STRUCTURE_URLS','VIDEO_STRUCTURE_DETECT_URLS']:env['V_ROUTER_'+key]=''
 for role in ['EMBEDDING','RERANK']:env['V_SCHEDULER_OLLAMA_'+role+'_URL']=''
 binary=os.environ.get('SCHEDULER_BINARY',str(pathlib.Path.home()/'dev/build/V-Scheduler/target/debug/v-scheduler'))
 app=launch('app','--network-alias','v-scheduler','--env-file',str(private('app.env','\n'.join(f'{k}={v}' for k,v in env.items())+'\n')),'-v',f'{config.parent}:/config','-v',f'{data}:/gateway-data','-v',f'{models}:/models','-v',f'{binary}:/binary:ro','--entrypoint','/binary',os.environ.get('SCHEDULER_TEST_SDK','v-scheduler-p0-test-sdk:20261019'),'controller','--config','/config/controller.json','--listen','0.0.0.0:38120')
 ctl=address(app,38120);gate=address(app,8080);peerurl=address(peer,11434)
 until(lambda:request(ctl+'/health')[0]==200,'controller not ready')
 test_memory=[12288];test_stale=[False]
 def heartbeat():
  while not stop.is_set():
   now=(datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(seconds=60 if test_stale[0] else 0)).isoformat()
   try:request(ctl+'/api/v1/workers/local/heartbeat',{'session_id':'feature-test','report':{'observed_at':now,'cpu_logical_count':4,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':test_memory[0],'gpus':[]}},keys['worker'])
   except (OSError,TimeoutError):pass
   stop.wait(.5)
 threading.Thread(target=heartbeat,daemon=True).start()
 until(lambda:len(list((data/'ollama-bindings').glob('*.json')))==6,'auto bindings not ready')
 until(lambda:len(request(gate+'/v1/models',token=keys['gateway'])[1].get('data',[]))==2,'catalog not ready')
 def api(path,body=None,method=None,token=None):return request(gate+path,body,token or keys['gateway'],method)
 def chat(name='chat:latest',stream=False,text='hello',queue=None,priority='interactive',identity=None):
  headers={} if queue is None else {'X-VScheduler-Queue-Wait-Seconds':'5','X-VScheduler-Priority':priority,'X-VScheduler-Request-ID':queue}
  return request(gate+'/v1/chat/completions',{'model':'v-scheduler/ollama-qa/'+name,'stream':stream,'messages':[{'role':'user','content':text}]},identity or keys['gateway'],headers=headers)

 def call(path,body=None,token=None,method=None,headers=None):
  return request(ctl+'/api/v1'+path,body,token or keys['batch'],method,headers)
 def submit_task(label, options=None, executor='batch.fixture'):
  body={'protocol_version':1,'app_task_id':label,'executor':{'id':executor,'contract_version':'1','task_type':'batch.test'},'parameters':{},'inputs':[]}
  if options:body['scheduling']=options
  return call('/tasks',body,headers={'Idempotency-Key':label})
 def task(label,options=None):
  code,v=submit_task(label,options);check(code==202,'submit '+label);return v['data']
 def claim():
  code,v=call('/workers/local/commands/claim',{'session_id':'feature-test'},token=keys['worker']);assert code==200,(code,v);return v['data']['commands']
 def report(c,status='succeeded',stopped=True):return call('/attempts/'+c['attempt_id']+'/report',{'session_id':'feature-test','fence':c['fence'],'status':status,'stopped':stopped,'result':{}},token=keys['worker'])
 def finish(c):check(report(c)[0]==200,'confirmed terminal '+c['task_id'])
 def cancel(t):return call('/tasks/'+t['task_id']+'/cancel',{})
 def held():return int(sql("SELECT count(*) FROM vrs_reservations WHERE state<>'released'"))
 low=task('fifo-low');high=task('fifo-high',{'priority':'high'});c=claim()[0];check(c['task_id']==high['task_id'],'queued high priority overtakes background');finish(c);c=claim()[0];check(c['task_id']==low['task_id'],'background executes after high');finish(c)
 low=task('protected-low');c=claim()[0];high=task('protected-high',{'priority':'high','allow_preemption':True});c=claim()[0];check(c['task_id']==low['task_id'] and not c['cancel'],'default running work never preempted');cancel(high);finish(c)
 low=task('no-request-low',{'preemptible':True});c=claim()[0];high=task('no-request-high',{'priority':'high'});check(not claim()[0]['cancel'],'high priority alone cannot preempt');cancel(high);finish(c)
 low=task('restart-low',{'preemptible':True});old=claim()[0];high=task('restart-high',{'priority':'high','allow_preemption':True});command=claim()[0]
 check(command['task_id']==low['task_id'] and command['cancel'] and command['preempted'],'two-sided consent sends cooperative preemption command')
 check(held()==1,'preemption request keeps old reservation')
 check(report(command,'cancelled',False)[0]==409 and held()==1,'missing native stop proof blocks release and restart')
 check(report(command,'cancelled')[1]['data']['status']=='queued' and held()==0,'native stop proof requeues async task and releases old attempt')
 c=claim()[0];check(c['task_id']==high['task_id'],'high priority starts only after old native exit');finish(c)
 c=claim()[0];check(c['task_id']==low['task_id'] and c['attempt_id']!=old['attempt_id'] and c['fence']>old['fence'] and not c.get('preempted',False),'low priority restarts with new attempt fence');finish(c)
 low=task('cancel-wins-low',{'preemptible':True});claim();high=task('cancel-wins-high',{'priority':'high','allow_preemption':True});c=claim()[0];cancel(low)
 check(report(c,'cancelled')[1]['data']['status']=='cancelled','explicit user cancellation wins over preemption restart');finish(claim()[0]);check(not claim(),'cancelled task is not replayed')
 low=task('success-race-low',{'preemptible':True});claim();high=task('success-race-high',{'priority':'high','allow_preemption':True});c=claim()[0]
 check(report(c,'succeeded')[1]['data']['status']=='succeeded','native success racing preemption never causes duplicate generation');finish(claim()[0]);check(not claim(),'successful old task never restarts')
 check(submit_task('unsupported-restart',{'preemptible':True},'batch.legacy')[0]==422,'executor without restart contract rejects opt in')
 old=task('aged-low',{'preemptible':True});sql("UPDATE vrs_tasks SET created_at=now()-interval '90 seconds' WHERE id='"+old['task_id']+"'");high=task('aged-high',{'priority':'high','allow_preemption':True});c=claim()[0]
 check(c['task_id']==old['task_id'],'aging prevents starvation under repeated high arrivals');check(not claim()[0]['cancel'],'aged running task is protected from repeated preemption');finish(c);finish(claim()[0])
 # Real permit stop protocol, not a synthetic reservation delete.
 registration={'protocol_version':1,'revision':0,'executors':['batch.infer'],'stop_protocol':'permit-native-exit-v1'}
 check(call('/integrations',registration)[0]==200,'register native permit stop adapter')
 def acquire(label,options=None,headers=None):
  body={'executor_id':'batch.infer','upstream_url':'http://fixture:9999/v1','model_id':'fixture','execution_token':keys['batch']}
  if options:body['scheduling']=options
  return call('/permits',body,headers={'Idempotency-Key':label,**(headers or {})})
 code,v=acquire('sync-low',{'preemptible':True});check(code==200,'synchronous low priority admitted with consent');pid=v['data']['permit_id']
 proof={'executor_id':'batch.infer','upstream_url':'http://fixture:9999/v1','model_id':'fixture','execution_token':keys['batch'],'execution_id':'native-priority','claim':True}
 check(call('/permits/'+pid+'/verify',proof)[0]==200,'actual execution identity claimed')
 high={'priority':'high','allow_preemption':True}
 check(acquire('sync-high',high)[1]['code']=='WAIT_STOP_CONFIRMATION','high permit waits for matched native stop receipt')
 commands=call('/integrations/heartbeat',{'revision':1})[1]['data']['stop_requests'];check(len(commands)==1,'preemption delivered to unique native receiver')
 command=commands[0];ack={'nonce':command['nonce'],'execution_id':'native-priority','execution_token':keys['batch'],'native_exit_barrier':True,'model_unloaded':True,'evidence':'fixture native exit and unload'}
 check(call('/permits/'+pid+'/stopped',{**ack,'model_unloaded':False})[0]==422,'preemption cannot bypass weight-unload confirmation')
 check(call('/permits/'+pid+'/stopped',ack)[0]==200,'matched native proof releases preempted synchronous execution')
 code,v=acquire('sync-high',high);check(code==200,'caller retry starts high execution after stop proof');call('/permits/'+v['data']['permit_id']+'/release',{'stopped':True,'evidence':'fixture native exit'})
 # Soft override retains hard resource accounting and liveness checks.
 force={'force_admission':True};operator={'X-VScheduler-Admin-Authorization':'Bearer '+keys['admin']}
 check(acquire('force-denied',force)[0]==403,'ordinary application cannot self-authorize force')
 test_memory[0]=1800
 until(lambda:sql("SELECT report->>'memory_available_mib' FROM vrs_nodes WHERE id='local'")=='1800','pressure sample not applied')
 check(acquire('soft-pressure')[1]['code']=='WAIT_MEMORY_HEADROOM','normal admission respects automatic headroom')
 force_body={'protocol_version':1,'app_task_id':'force-async','executor':{'id':'batch.fixture','contract_version':'1','task_type':'batch.test'},'parameters':{},'inputs':[],'scheduling':force}
 check(call('/tasks',force_body,headers={'Idempotency-Key':'force-async'})[0]==403,'fresh forced async task requires verified administrator')
 code,reply=call('/tasks',force_body,headers={'Idempotency-Key':'force-async',**operator});check(code==202,'administrator authorizes async force once')
 check(call('/tasks',force_body,headers={'Idempotency-Key':'force-async'})[0]==200,'accepted forced async receipt recovers without persisting operator credential')
 c=claim()[0];check(c['task_id']==reply['data']['task_id'],'async force relaxes soft headroom under approved budget');finish(c)
 code,v=acquire('force-admin',force,operator);check(code==200,'separately verified administrator can relax soft headroom')
 check(acquire('force-capacity',force,operator)[0]==429,'force cannot double-book actual approved capacity')
 call('/permits/'+v['data']['permit_id']+'/release',{'stopped':True,'evidence':'fixture native exit'})
 test_memory[0]=1100
 until(lambda:sql("SELECT report->>'memory_available_mib' FROM vrs_nodes WHERE id='local'")=='1100','floor sample not applied')
 check(acquire('force-hard-floor',force,operator)[0]==429,'force retains at least one GiB plus model budget')
 test_memory[0]=12288;test_stale[0]=True
 until(lambda:request(ctl+'/api/v1/nodes',token=keys['admin'])[1]['data'][0]['report']['observed_at']< (datetime.datetime.now(datetime.timezone.utc)-datetime.timedelta(seconds=30)).isoformat(),'stale report not applied')
 check(acquire('force-stale',force,operator)[0]==429,'force cannot override stale node telemetry')
 check(int(sql("SELECT count(*) FROM vrs_permits WHERE state<>'released'"))==0 and held()==0,'priority tests leave no resource or native-stop leak')
 check(int(sql("SELECT count(*) FROM vrs_management_events WHERE action='scheduling.force'"))>=1,'successful force request has administrator audit')
 private('result.json',{'passed':checks,'real_controller':True,'real_database':True,'native_fixture':True});print('PASS',len(checks),'checks; evidence',run,flush=True)
finally:
 stop.set()
 for name in reversed(owned):
  try:docker('rm','-f',name)
  except subprocess.CalledProcessError:pass
 try:docker('network','rm',network)
 except subprocess.CalledProcessError:pass
