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
 config=private('config/controller.json',{'principals':[{'token':keys['other'],'role':'submitter','app_id':'other'},{'token':keys['admin'],'role':'admin','app_id':'test'},{'token':keys['worker'],'role':'worker','node_id':'local','app_id':'worker'},{'token':keys['batch'],'role':'submitter','app_id':'batch'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':1000,'memory_mib':8192}}],'executors':[{'id':'batch.fixture','contract_version':'1','task_type':'batch.test','node_id':'local','app_id':'batch','resources':{'cpu_millicores':1000,'memory_mib':128},'retry_mode':'none','max_attempts':1},{'id':'batch.infer','contract_version':'1','task_type':'model.infer','node_id':'local','app_id':'batch','upstream_urls':['http://fixture:9999/v1'],'resources':{'cpu_millicores':1000,'memory_mib':128}}],'local_agent':{'node_id':'local','automatic_capacity':False,'executors':[]}})
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
 def heartbeat():
  while not stop.is_set():
   now=datetime.datetime.now(datetime.timezone.utc).isoformat()
   try:request(ctl+'/api/v1/workers/local/heartbeat',{'session_id':'feature-test','report':{'observed_at':now,'cpu_logical_count':4,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':12288,'gpus':[]}},keys['worker'])
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
 registration={'protocol_version':1,'revision':0,'executors':['batch.infer'],'stop_protocol':'permit-native-exit-v1'}
 check(call('/integrations',{**registration,'resources':{'memory_mib':1}})[0]==422,'client cannot self-approve resource budget')
 check(call('/integrations',{**registration,'executors':['builtin-unknown']})[0]==403,'unapproved executor denied')
 code,value=call('/integrations',registration)
 check(code==200 and value['data']['revision']==1 and value['data']['budget_granted'] is False,'register approved execution contract without budget escalation')
 registration['revision']=1
 check(call('/integrations',registration)[1]['data']['revision']==1,'same registration idempotent with current revision')
 check(call('/integrations',{**registration,'revision':0})[0]==409,'stale registration revision rejected')
 code,view=call('/integrations')
 check(code==200 and len(view['data']['items'])==1 and view['data']['items'][0]['executors'][0]['node']['fresh'],'application sees its approved budget and live node together')
 check(call('/integrations',token=keys['gateway'])[0] in (401,403),'gateway key is not scheduler application identity')
 check(call('/integrations',token=keys['other'])[1]['data']['items']==[],'other application cannot see registrations')
 check(call('/integrations',token=keys['worker'])[0]==403,'worker cannot impersonate application registration')
 check(call('/integrations/heartbeat',{'revision':1,'report':{'memory_available_mib':999999}})[0]==422,'application heartbeat cannot forge node telemetry')
 check(call('/integrations/heartbeat',{'revision':1})[0]==200,'periodic application heartbeat accepted')
 check(call('/integrations/heartbeat',{'revision':1})[1]['data']['complete_execution_snapshot'] is True,'idle heartbeat explicitly carries a complete execution snapshot')
 sql("UPDATE vrs_integrations SET heartbeat=now()-interval '25 seconds'")
 execution=secrets.token_hex(32)
 body={'executor_id':'batch.infer','upstream_url':'http://fixture:9999/v1','model_id':'fixture','execution_token':execution}
 def acquire(key):return call('/permits',body,headers={'Idempotency-Key':key})
 check(acquire('stale')[0]==503,'stale application heartbeat prevents new execution')
 check(call('/integrations/heartbeat',{'revision':1})[0]==200,'heartbeat restores eligibility without manual intervention')
 code,result=acquire('execution')
 check(code==200,'fresh application and node admit one approved execution')
 permit=result['data']['permit_id']
 verify={**body,'execution_id':'native-1','claim':True}
 sql("UPDATE vrs_integrations SET heartbeat=now()-interval '25 seconds'")
 check(call('/permits/'+permit+'/verify',verify)[0]==503,'stale application cannot claim an already allocated execution')
 call('/integrations/heartbeat',{'revision':1})
 check(call('/permits/'+permit+'/verify',verify)[0]==200,'native backend claims exact execution')
 snapshot=call('/integrations/heartbeat',{'revision':1})[1]['data']
 check(snapshot['complete_execution_snapshot'] and any(row['permit_id']==permit and row['execution_id']=='native-1' for row in snapshot['executions']),'heartbeat retains claimed execution until verified release')
 check('execution_token' not in json.dumps(snapshot),'heartbeat snapshot never discloses execution proof')
 check(call('/permits/'+permit+'/stop',{},token=keys['worker'])[0]==403,'worker cannot request model stop as a user')
 check(call('/permits/'+permit+'/stop',{},token=keys['other'])[0]==404,'other application cannot stop this execution')
 code,reply=call('/permits/'+permit+'/stop',{})
 check(code==200 and reply['data']['resources_released'] is False,'stop intent never releases reservation')
 check(call('/permits/'+permit+'/release',{'stopped':True,'evidence':'HTTP completed'})[0]==409,'ordinary completion cannot bypass pending native stop proof')
 check(call('/integrations',method='DELETE')[0]==409,'cannot unregister while execution or stop is pending')
 code,reply=call('/integrations/heartbeat',{'revision':1});command=reply['data']['stop_requests'][0]
 check(command['permit_id']==permit and command['execution_id']=='native-1' and command['action']=='cancel-and-unload','application heartbeat delivers exact model stop command')
 docker('restart',app)
 until(lambda:request(ctl+'/health')[0]==200,'controller restart failed')
 check(call('/integrations')[1]['data']['items'][0]['revision']==1,'application registration survives controller restart')
 again=call('/integrations/heartbeat',{'revision':1})[1]['data']['stop_requests'][0]
 check(again['nonce']==command['nonce'],'unacknowledged stop command is redelivered unchanged')
 check(call('/permits/'+permit+'/verify',{**verify,'claim':False})[0]==409,'stop intent fences subsequent execution validation')
 check(call('/permits/'+permit+'/renew',{})[0]==409,'stop intent blocks permit renewal')
 ack={'nonce':command['nonce'],'execution_id':'native-1','execution_token':execution,'native_exit_barrier':True,'model_unloaded':True,'evidence':'fixture native process exited and weights unloaded'}
 check(call('/permits/'+permit+'/stopped',{**ack,'nonce':'wrong'})[0]==403,'wrong nonce cannot release resources')
 check(call('/permits/'+permit+'/stopped',{**ack,'execution_id':'wrong'})[0]==403,'wrong execution cannot release resources')
 check(call('/permits/'+permit+'/stopped',{**ack,'execution_token':'x'*64})[0]==403,'wrong execution token cannot release resources')
 check(call('/permits/'+permit+'/stopped',{**ack,'model_unloaded':False})[0]==422,'cancel without unloaded weights cannot free full budget')
 check(sql("SELECT state FROM vrs_permits WHERE id='"+permit+"'")!='released','failed proofs retain original reservation')
 check(call('/permits/'+permit+'/stopped',ack)[0]==200,'matched native exit and unloading receipt releases reservation')
 check(call('/permits/'+permit+'/stopped',ack)[0]==200,'acknowledgement retry is idempotent')
 check(call('/integrations/heartbeat',{'revision':1})[1]['data']['stop_requests']==[],'confirmed stop disappears from pending commands')
 check(not any(row['permit_id']==permit for row in call('/integrations/heartbeat',{'revision':1})[1]['data']['executions']),'released native execution leaves heartbeat snapshot')
 stop.set();time.sleep(.6)
 sql("UPDATE vrs_nodes SET heartbeat=now()-interval '20 seconds'")
 check(acquire('stale-node')[0]==429,'stale node telemetry prevents new execution independently of app heartbeat')
 now=datetime.datetime.now(datetime.timezone.utc).isoformat()
 request(ctl+'/api/v1/workers/local/heartbeat',{'session_id':'feature-test','report':{'observed_at':now,'cpu_logical_count':4,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':12288,'gpus':[]}},keys['worker'])
 code,unclaimed=acquire('unclaimed-stop');unclaimed=unclaimed['data']['permit_id']
 check(call('/permits/'+unclaimed+'/stop',{})[0]==200,'stop before native claim remains fenced')
 pending=call('/integrations/heartbeat',{'revision':1})[1]['data']['stop_requests'][0]
 check(pending['execution_id'] is None,'unclaimed execution is explicitly identified without invented handle')
 check(call('/permits/'+unclaimed+'/verify',{**verify,'execution_id':'too-late'})[0]==409,'late native claim cannot race an accepted stop')
 check(call('/permits/'+unclaimed+'/stopped',{**ack,'nonce':pending['nonce'],'execution_id':None})[0]==200,'confirmed never-started execution can release safely')
 check(acquire('after-stop')[0]==200,'next request recovers after confirmed stop and restored telemetry')
 # Simulate an unknown outcome: expiry must retain a suspect commitment.
 sql("UPDATE vrs_permits SET lease_until=now()-interval '1 minute' WHERE idem='after-stop'")
 until(lambda:sql("SELECT state FROM vrs_permits WHERE idem='after-stop'")=='suspect','expired permit not held suspect')
 check(sql("SELECT count(*) FROM vrs_stop_requests WHERE confirmed_at IS NOT NULL")=='2','stop confirmation persists across requests')
 sql("UPDATE vrs_permits SET state='released',evidence='fixture native exit confirmed' WHERE idem='after-stop'")
 check(call('/integrations',method='DELETE')[0]==200,'idle application can unregister without leaving a stale liveness gate')
 check(call('/integrations',method='DELETE')[0]==200,'unregister retry is idempotent')
 check(call('/integrations/heartbeat',{'revision':1})[0]==404,'removed registration cannot be resurrected by a stale heartbeat')
 private('result.json',{'passed':checks,'native_fixture':True,'real_controller':True,'real_database':True})
 print('PASS',len(checks),'checks; evidence',run,flush=True)
finally:
 stop.set()
 for name in reversed(owned):
  try:docker('rm','-f',name)
  except subprocess.CalledProcessError:pass
 try:docker('network','rm',network)
 except subprocess.CalledProcessError:pass
