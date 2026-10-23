#!/usr/bin/env python3
"""Queue, warm reuse and model linkage against the real Rust process and PostgreSQL.
Synthetic native peers run on a private Docker network. No installed data or weights.
"""
import concurrent.futures, datetime, json, os, pathlib, secrets, subprocess, threading, time, urllib.error, urllib.request, uuid
root=pathlib.Path(os.environ.get('SCHEDULER_TEST_BUILD_ROOT',str(pathlib.Path.home()/'dev/build/V-Scheduler/features-20261020')))
run=root/('management-'+uuid.uuid4().hex);run.mkdir(parents=True,mode=0o700)
repo=pathlib.Path(__file__).resolve().parents[1]
network='vs-management-'+uuid.uuid4().hex[:10];owned=[];stop=threading.Event();checks=[]
keys={k:secrets.token_hex(32) for k in ['db','admin','worker','gateway','batch']}
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
let active=0,peak=0,calls=0,loads=0,unloads=0,valid=true,loaded=new Set(),history=[],mutations=[];
const models=[{name:'chat:latest',size:1048576},{name:'other:latest',size:1048576}];
const handler=async(req,res)=>{res.setHeader('Content-Type','application/json');let text='';for await(const c of req)text+=c;const body=text?JSON.parse(text):{};
const reply=(value,status=200)=>{res.statusCode=status;res.end(JSON.stringify(value));};
if(req.url==='/metrics')return reply({active_slots:active});
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
if(req.url==='/api/ps')return reply({models:[...loaded].map(name=>({name,size:1073741824,size_vram:0}))});
if(req.url==='/stats')return reply({active,peak,calls,loads,unloads,loaded:[...loaded],history,mutations});
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
 active++;peak=Math.max(peak,active);calls++;if(!loaded.has(body.model))loads++;loaded.add(body.model);history.push({model:body.model,permit:req.headers['x-vscheduler-permit-id'],keep_alive:body.keep_alive});
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
 native_fixture_url='http://v-voice-backend:7070/v1' if os.environ.get('SCHEDULER_TEST_NATIVE_QUEUE')=='1' else 'http://fixture:9999/v1'
 config=private('config/controller.json',{'principals':[{'token':keys['admin'],'role':'admin','app_id':'test'},{'token':keys['worker'],'role':'worker','node_id':'local','app_id':'worker'},{'token':keys['batch'],'role':'submitter','app_id':'batch'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':3000 if os.environ.get('SCHEDULER_TEST_NATIVE_QUEUE')=='1' else 1000,'memory_mib':8192}}],'executors':[{'id':'batch.fixture','contract_version':'1','task_type':'batch.test','node_id':'local','app_id':'batch','resources':{'cpu_millicores':1000,'memory_mib':128},'retry_mode':'none','max_attempts':1},{'id':'batch.infer','contract_version':'1','task_type':'model.infer','node_id':'local','app_id':'batch','upstream_urls':[native_fixture_url],'resources':{'cpu_millicores':1000,'memory_mib':128}}],'local_agent':{'node_id':'local','automatic_capacity':False,'executors':[]}})
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
 available_memory=12288
 def heartbeat():
  while not stop.is_set():
   now=datetime.datetime.now(datetime.timezone.utc).isoformat()
   try:request(ctl+'/api/v1/workers/local/heartbeat',{'session_id':'feature-test','report':{'observed_at':now,'cpu_logical_count':4,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':available_memory,'gpus':[]}},keys['worker'])
   except (OSError,TimeoutError):pass
   stop.wait(.5)
 threading.Thread(target=heartbeat,daemon=True).start()
 until(lambda:len(list((data/'ollama-bindings').glob('*.json')))==6,'auto bindings not ready')
 until(lambda:len(request(gate+'/v1/models',token=keys['gateway'])[1].get('data',[]))==2,'catalog not ready')
 def api(path,body=None,method=None,token=None):return request(gate+path,body,token or keys['gateway'],method)
 def chat(name='chat:latest',stream=False,text='hello',queue=None,priority='interactive',identity=None):
  headers={} if queue is None else {'X-VScheduler-Queue-Wait-Seconds':'5','X-VScheduler-Priority':priority,'X-VScheduler-Request-ID':queue}
  return request(gate+'/v1/chat/completions',{'model':'v-scheduler/ollama-qa/'+name,'stream':stream,'messages':[{'role':'user','content':text}]},identity or keys['gateway'],headers=headers)
 first=chat();private('first-response.json',first)
 check(first[0]==200,'first inference succeeds with a resource permit')
 resident=api('/api/v1/model-residency')[1]['resident'];assert resident
 first_id=resident['permit_id']
 check(sql("SELECT count(*) FROM vrs_permits WHERE state='active';")=='1','warm memory retains its resource reservation')
 check(chat(stream=True)[0]==200 and api('/api/v1/model-residency')[1]['resident']['permit_id']==first_id,'same model SSE reuses reserved warm model')
 check(sql("SELECT metadata->>'resident_reuses' FROM vrs_permits WHERE state='active';")=='1','controller records reuse with rotated execution proof')
 if os.environ.get('SCHEDULER_TEST_NATIVE_QUEUE')=='1':
  api('/api/v1/model-residency',{'idle_seconds':0},'PUT')
  until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','initial resident not stopped')
  def native_body(priority='interactive',force=False):
   return {'executor_id':'batch.infer','upstream_url':native_fixture_url,'model_id':'shared-native-fixture','execution_token':secrets.token_hex(32),'scheduling':{'priority':priority,'force_admission':force}}
  held=request(ctl+'/api/v1/permits',native_body(),keys['batch'],headers={'Idempotency-Key':'native-held'})
  assert held[0]==200,held
  order=[]
  def queued(label,priority,force=False):
   result=request(ctl+'/api/v1/permits/wait',{'request':native_body(priority,force),'wait_seconds':5},keys['batch'],headers={'Idempotency-Key':label,**({'X-VScheduler-Admin-Authorization':'Bearer '+keys['admin']} if force else {})})
   if result[0]==200:
    order.append(label)
    request(ctl+'/api/v1/permits/'+result[1]['data']['permit_id']+'/release',{'stopped':True,'evidence':'inference_request_not_started'},keys['batch'])
   return result
  with concurrent.futures.ThreadPoolExecutor() as pool:
   low=pool.submit(queued,'native-low','background')
   until(lambda:int(sql("SELECT count(*) FROM vrs_inference_waiters;"))==1,'native application did not enter the waiting queue',seconds=8)
   high=pool.submit(queued,'native-high','high')
   until(lambda:int(sql("SELECT count(*) FROM vrs_inference_waiters;"))==2,'high priority native application did not queue')
   forced=pool.submit(queued,'native-forced','background',True)
   until(lambda:int(sql("SELECT count(*) FROM vrs_inference_waiters;"))==3,'authorized forced native application did not queue')
   check(sql("SELECT count(*) FROM vrs_permits WHERE state='active';")=='1','waiting native requests reserve no duplicate model memory')
   request(ctl+'/api/v1/permits/'+held[1]['data']['permit_id']+'/release',{'stopped':True,'evidence':'inference_request_not_started'},keys['batch'])
   outcomes=[low.result(),high.result(),forced.result()]
   check(all(r[0]==200 for r in outcomes),'native applications complete bounded admission without retrying inference')
  check(order==['native-forced','native-high','native-low'],'authorized force first, then task priority, regardless of model identity')
  until(lambda:sql("SELECT count(*) FROM vrs_inference_waiters;")=='0','native queue hints not cleaned')
  check(sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','native queue completion releases every test permit')
  denied=request(ctl+'/api/v1/permits/wait',{'request':native_body('high',True),'wait_seconds':1},keys['batch'],headers={'Idempotency-Key':'native-force-denied'})
  check(denied[0]==403,'native force cannot be enabled by an ordinary application')
  held=request(ctl+'/api/v1/permits',native_body(),keys['batch'],headers={'Idempotency-Key':'native-timeout-held'})
  timed=request(ctl+'/api/v1/permits/wait',{'request':native_body(),'wait_seconds':1},keys['batch'],headers={'Idempotency-Key':'native-timeout'})
  check(timed[0]==429 and timed[1]['code']=='QUEUE_TIMEOUT','native admission deadline does not start computation')
  until(lambda:sql("SELECT count(*) FROM vrs_inference_waiters;")=='0','timed out hint not cleaned')
  request(ctl+'/api/v1/permits/'+held[1]['data']['permit_id']+'/release',{'stopped':True,'evidence':'inference_request_not_started'},keys['batch'])
  check(request(ctl+'/api/v1/permits/wait',{'request':native_body(),'wait_seconds':301},keys['batch'],headers={'Idempotency-Key':'native-invalid-wait'})[0]==422,'native admission wait is bounded')
  private('result.json',{'passed':checks,'fixture_native':True,'installed_data_touched':False})
  print(json.dumps({'passed':len(checks),'evidence':str(run/'result.json')}),flush=True)
  raise SystemExit(0)
 if os.environ.get('SCHEDULER_TEST_MULTI_RESIDENCY')=='1':
  before=request(peerurl+'/stats')[1]
  check(chat('other:latest')[0]==200,'second resident model succeeds')
  check(chat()[0]==200,'first resident model remains callable')
  after=request(peerurl+'/stats')[1]
  check(sorted(after['loaded'])==['chat:latest','other:latest'] and after['unloads']==before['unloads'],'two models stay loaded across role/model switches')
  check(sql("SELECT count(*) FROM vrs_permits WHERE state='active' AND (resources->>'cpu_millicores')::int=0 AND (resources->>'gpu_slots')::int=0;")=='2','idle models retain memory without holding compute capacity')
  before=request(peerurl+'/stats')[1]
  for i in range(27):check(chat('other:latest' if i%2 else 'chat:latest',stream=bool(i%3))[0]==200,'alternating warm call '+str(i))
  after=request(peerurl+'/stats')[1]
  check(after['loads']==before['loads'] and after['unloads']==before['unloads'],'27 interleaved HTTP/SSE calls do not reload either model')
  with concurrent.futures.ThreadPoolExecutor() as pool:
   held=pool.submit(chat,text='hold')
   until(lambda:request(peerurl+'/stats')[1]['active']==1,'native hold not active')
   waiting=pool.submit(chat,'other:latest')
   check(held.result()[0]==200 and waiting.result()[0]==200,'other resident waits for the compute lane')
  after=request(peerurl+'/stats')[1]
  check(after['peak']==1 and after['unloads']==before['unloads'],'shared compute stays exclusive without evicting the waiting resident')
  before=request(peerurl+'/stats')[1]
  with concurrent.futures.ThreadPoolExecutor() as pool:
   held=pool.submit(chat,text='hold')
   until(lambda:request(peerurl+'/stats')[1]['active']==1,'shared-model hold not active')
   same=pool.submit(chat)
   check(held.result()[0]==200 and same.result()[0]==200,'concurrent callers of the same model share the resident weights')
  after=request(peerurl+'/stats')[1]
  check(after['loads']==before['loads'] and after['unloads']==before['unloads'] and sql("SELECT count(*) FROM vrs_permits WHERE state='active';")=='2','same-model requests never reserve another copy of weights or evict the other resident')
  for high_model,low_model in [('other:latest','chat:latest'),('chat:latest','other:latest')]:
   before=request(peerurl+'/stats')[1]
   with concurrent.futures.ThreadPoolExecutor() as pool:
    held=pool.submit(chat,text='hold')
    until(lambda:request(peerurl+'/stats')[1]['active']==1,'priority hold not active')
    low=pool.submit(chat,low_model,False,'hello','low-'+high_model.replace(':','-'),'background')
    until(lambda:len(api('/api/v1/inference-queue')[1]['items'])==1,'low priority call not queued')
    high=pool.submit(chat,high_model,False,'hello','high-'+high_model.replace(':','-'),'high')
    priority_results=[held.result(),high.result(),low.result()]
    private('priority-results.json',priority_results)
    check(all(r[0]==200 for r in priority_results),'task priorities work for shared resident '+high_model+' '+str([(r[0],r[1].get('code') if isinstance(r[1],dict) else None) for r in priority_results]))
   after=request(peerurl+'/stats')[1]
   check([r['model'] for r in after['history'][-2:]]==[high_model,low_model] and after['unloads']==before['unloads'],'priority follows the request, not the model, without reloads')
  check(chat(queue='priority-high',priority='high')[0]==200 and sql("SELECT metadata->'scheduling'->>'priority' FROM vrs_permits WHERE id='"+first_id+"';")=='high','warm lease receives this request priority')
  check(chat(queue='priority-background',priority='background')[0]==200 and sql("SELECT metadata->'scheduling'->>'priority' FROM vrs_permits WHERE id='"+first_id+"';")=='background','next request can change priority without giving the model a permanent rank')
  invalid=request(ctl+'/api/v1/permits/'+first_id+'/park-resident',{'executor_id':'invalid','upstream_url':'http://fixture','model_id':'chat:latest','execution_token':'x'*64,'next_execution_token':'y'*64},keys['batch'])
  check(invalid[0]==403,'third-party identity cannot park an owned model permit')
  available_memory=2047
  until(lambda:sql("SELECT report->>'memory_available_mib' FROM vrs_nodes WHERE id='local';")=='2047','unsafe memory sample not reported')
  calls=request(peerurl+'/stats')[1]['calls']
  rejected=request(gate+'/v1/chat/completions',{'model':'v-scheduler/ollama-qa/chat:latest','messages':[{'role':'user','content':'hello'}]},keys['gateway'],headers={'X-VScheduler-Queue-Wait-Seconds':'0'})
  check(rejected[0]==429 and request(peerurl+'/stats')[1]['calls']==calls,'parked residency never bypasses the memory safety floor')
  available_memory=12288
  api('/api/v1/model-residency',{'idle_seconds':0},'PUT')
  until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','idle test permits not released')
  bindings=[json.loads(docker('exec',app,'cat','/gateway-data/ollama-bindings/'+p.name)) for p in (data/'ollama-bindings').glob('*.json')]
  binding=next(b for b in bindings if 'v-scheduler/ollama-qa/chat:latest' in b.get('models',{}))
  model_id='v-scheduler/ollama-qa/chat:latest';executor=binding['models'][model_id]['executor_id'];proof=secrets.token_hex(32);next_proof=secrets.token_hex(32)
  body={'executor_id':executor,'upstream_url':'http://v-scheduler-ollama-qa:11535/v1','model_id':'chat:latest','execution_token':proof}
  permit=request(ctl+'/api/v1/permits',body,binding['token'],headers={'Idempotency-Key':'park-proof-regression'})
  check(permit[0]==200,'owned permit can be obtained for the native idle proof regression');pid=permit[1]['data']['permit_id']
  active_resources=sql("SELECT resources::text FROM vrs_permits WHERE id='"+pid+"';")
  with concurrent.futures.ThreadPoolExecutor() as pool:
   held=pool.submit(request,peerurl+'/v1/chat/completions',{'model':'chat:latest','messages':[{'role':'user','content':'hold'}],'keep_alive':-1},None,None,{'X-VScheduler-Permit-ID':pid})
   until(lambda:request(peerurl+'/stats')[1]['active']==1,'native proof hold not active')
   parked=request(ctl+'/api/v1/permits/'+pid+'/park-resident',{**body,'next_execution_token':next_proof},binding['token'])
   check(parked[0]==409 and sql("SELECT resources::text FROM vrs_permits WHERE id='"+pid+"';")==active_resources,'active native compute cannot return its resource reservation')
   check(held.result()[0]==200,'native proof regression completes its synthetic computation')
  parked=request(ctl+'/api/v1/permits/'+pid+'/park-resident',{**body,'next_execution_token':next_proof},binding['token'])
  check(parked[0]==200,'native idle evidence permits memory-only reservation')
  verified=request(ctl+'/api/v1/permits/'+pid+'/verify',{**body,'execution_token':next_proof,'execution_id':'parked-proof-regression','claim':True},binding['token'])
  check(verified[0]==409 and verified[1].get('code')=='PERMIT_RESIDENT_IDLE','parked proof cannot start compute without restoring the full budget')
  stopped=request(peerurl+'/api/scheduler/confirm-stopped',{'model':'chat:latest','nonce':'proof-regression'})
  assert stopped[1]['stopped']
  request(ctl+'/api/v1/permits/'+pid+'/release',{'stopped':True,'evidence':'native_proof_regression_stopped'},binding['token'])
  private('result.json',{'passed':checks,'fixture_native':True,'installed_data_touched':False})
  print(json.dumps({'passed':len(checks),'evidence':str(run/'result.json')}),flush=True)
  raise SystemExit(0)
 # Model memory is already reflected in MemAvailable. Preserve the 2048 MiB
 # safety floor while leaving less than the full model budget plus that floor.
 available_memory=2560
 until(lambda:sql("SELECT report->>'memory_available_mib' FROM vrs_nodes WHERE id='local';")=='2560','warm memory sample not reported')
 before=request(peerurl+'/stats')[1]
 for index in range(27):
  response=chat(stream=index%2==0)
  assert response[0]==200,('warm reuse rejected',index,response[0])
  assert api('/api/v1/model-residency')[1]['resident']['permit_id']==first_id,'warm reuse replaced its reservation'
 after=request(peerurl+'/stats')[1]
 check(after['calls']==before['calls']+27 and after['loads']==before['loads'] and after['unloads']==before['unloads'] and after['loaded']==['chat:latest'],'27 low-headroom HTTP/SSE calls reuse one reservation without unloading or reloading')
 available_memory=2047
 until(lambda:sql("SELECT report->>'memory_available_mib' FROM vrs_nodes WHERE id='local';")=='2047','unsafe memory sample not reported')
 rejected=request(gate+'/v1/chat/completions',{'model':'v-scheduler/ollama-qa/chat:latest','messages':[{'role':'user','content':'hello'}]},keys['gateway'],headers={'X-VScheduler-Queue-Wait-Seconds':'0'})
 check(rejected[0]==429 and request(peerurl+'/stats')[1]['calls']==after['calls'],'warm reuse below the safety floor refuses inference before native compute')
 available_memory=12288
 until(lambda:sql("SELECT report->>'memory_available_mib' FROM vrs_nodes WHERE id='local';")=='12288','normal memory sample not restored')
 if os.environ.get('SCHEDULER_TEST_RESIDENCY_ONLY')=='1':
  private('result.json',{'passed':checks,'fixture_native':True,'installed_data_touched':False})
  print(json.dumps({'passed':len(checks),'evidence':str(run/'result.json')}),flush=True)
  raise SystemExit(0)
 with concurrent.futures.ThreadPoolExecutor() as pool:
  held=pool.submit(chat,text='hold')
  until(lambda:request(peerurl+'/stats')[1]['active']==1,'implicit queue hold not active')
  calls=request(peerurl+'/stats')[1]['calls']
  immediate=request(gate+'/v1/chat/completions',{'model':'v-scheduler/ollama-qa/other:latest','messages':[{'role':'user','content':'hello'}]},keys['gateway'],headers={'X-VScheduler-Queue-Wait-Seconds':'0'})
  check(immediate[0]==429 and request(peerurl+'/stats')[1]['calls']==calls,'explicit zero retains immediate refusal while an owned model is active')
  implicit=pool.submit(chat,'other:latest')
  check(held.result()[0]==200 and implicit.result()[0]==200,'resident-mode verified clients wait for owned model handoff without new headers')
 check(request(peerurl+'/stats')[1]['peak']==1,'automatic waiting never overlaps exclusive native execution')

 check(chat('other:latest')[0]==200,'model switch rechecks admission while preserving other idle models when resources fit')
 check(first_id!=api('/api/v1/model-residency')[1]['resident']['permit_id'] and request(peerurl+'/stats')[1]['unloads']>=1,'switch creates a fresh reservation after native stop proof')
 api('/api/v1/model-residency',{'idle_seconds':1},'PUT')
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','idle model was not safely released')
 check(not request(peerurl+'/stats')[1]['loaded'],'idle deadline confirms native unloading and releases reservation')
 api('/api/v1/model-residency',{'idle_seconds':0},'PUT')
 with concurrent.futures.ThreadPoolExecutor() as pool:
  held=pool.submit(chat,text='hold')
  until(lambda:request(peerurl+'/stats')[1]['active']==1,'hold not active')
  background=pool.submit(chat,text='hello',queue='background',priority='background',identity='alice')
  until(lambda:len(api('/api/v1/inference-queue')[1]['items'])==1,'background not queued')
  interactive=pool.submit(chat,queue='interactive',identity='alice')
  cancelled=pool.submit(chat,queue='cancelled',priority='background',identity='alice')
  until(lambda:len(api('/api/v1/inference-queue')[1]['items'])==3,'requests not queued')
  check(api('/api/v1/inference-queue')[1]['items'][0]['request_id']=='interactive','interactive admission precedes waiting background inference')
  check(api('/api/v1/inference-queue',token='bob')[1]['items']==[],'another VOS user cannot see private waiting requests')
  check(api('/api/v1/inference-queue/cancelled',method='DELETE',token='bob')[0]==404,'another user cannot cancel the queued request')
  check(api('/api/v1/inference-queue/cancelled',method='DELETE',token='alice')[0]==200 and cancelled.result()[1]['code']=='QUEUE_CANCELLED','cancelled waiting inference never reaches native execution')
  check(held.result()[0]==200 and interactive.result()[0]==200 and background.result()[0]==200,'busy resource is eventually admitted without provider replay')
 with concurrent.futures.ThreadPoolExecutor() as pool:
  holding=pool.submit(chat,text='hold')
  until(lambda:request(peerurl+'/stats')[1]['active']==1,'capacity hold not active')
  backgrounds=[]
  for index in range(8):
   backgrounds.append(pool.submit(chat,queue='batch-slot-'+str(index),priority='background',identity='bob'))
   until(lambda:len(api('/api/v1/inference-queue')[1]['items'])==index+1,'waiting request consumed an execution slot')
  urgent=pool.submit(chat,queue='interactive-slot',identity='alice')
  until(lambda:len(api('/api/v1/inference-queue')[1]['items'])==9,'interactive request could not join a busy waiting queue')
  check(api('/api/v1/inference-queue')[1]['items'][0]['request_id']=='interactive-slot','waiting background callers do not exhaust execution slots before interaction can join')
  for index in range(8):api('/api/v1/inference-queue/batch-slot-'+str(index),method='DELETE',token='bob')
  check(all(f.result()[1]['code']=='QUEUE_CANCELLED'for f in backgrounds),'eight background waiters cancel without starting native computation')
  check(holding.result()[0]==200 and urgent.result()[0]==200,'interactive call executes after the running request finishes despite a full background queue')
 check(request(peerurl+'/stats')[1]['peak']==1,'native execution remains exclusive under concurrent queued callers')
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','queue leaked permits')
 check(api('/api/v1/inference-queue')[1]['items']==[],'completed requests leave the waiting queue')
 life=api('/api/v1/model-lifecycle')[1]['items']
 ollama=next(m for m in life if m['model_id']=='ollama://chat:latest')
 hf=next(m for m in life if m['model_id']=='hf://fixture/weights')
 check(ollama['deployments'][0]['configured_callable'] and hf['blockers']==['NO_LINKED_GATEWAY_SERVICE'],'lifecycle joins built-in weights and identifies undeployed repository weights')
 check(hf['selected_files']==['weights.gguf'],'selected-file repository scope survives lifecycle inspection')
 # Explicit weight mapping associates served IDs without guessing from names.
 service={'service_id':'external-fixture','name':'External fixture','kind':'openai_compatible','base_url':address(peer,11535),'gateway_url':address(peer,11535),'api_contract':{'health_path':'/api/version','models_path':'/v1/models'},'metadata':{'model_bindings':{'chat:latest':'hf://fixture/weights'}}}
 check(api('/model-hub/api/v1/inference-services/register',service)[0]==200,'external inference service stores an exact repository model binding')
 registration={'service_id':'external-fixture','name':'External fixture','base_url':address(peer,11535)+'/v1','protocol':'chat_completions','model':'chat:latest','model_kind':'chat','endpoints':['chat/completions'],'visibility':'public'}
 check(api('/api/v1/registrations',registration)[0]==200,'linked inference service is registered through the existing gateway contract')
 hf=next(m for m in api('/api/v1/model-lifecycle')[1]['items'] if m['model_id']=='hf://fixture/weights')
 check(hf['deployments'] and 'SCHEDULER_BINDING_PENDING' in hf['deployments'][0]['blockers'],'lifecycle distinguishes routed services from missing resource approval')
 # Native background tasks and interactive requests share priority and aging.
 task_body={'protocol_version':1,'app_task_id':'batch-fixture','executor':{'id':'batch.fixture','contract_version':'1','task_type':'batch.test'},'parameters':{},'inputs':[]}
 task=request(ctl+'/api/v1/tasks',task_body,keys['batch'],headers={'Idempotency-Key':'batch-fixture'})
 assert task[0]==202,task[0]
 task_id=task[1]['data']['task_id']
 binding_path=next(p for p in (data/'ollama-bindings').glob('*.json') if 'v-scheduler/ollama-qa/chat:latest' in json.loads(docker('exec',app,'cat','/gateway-data/ollama-bindings/'+p.name))['models'])
 binding=json.loads(docker('exec',app,'cat','/gateway-data/ollama-bindings/'+binding_path.name))
 executor=binding['models']['v-scheduler/ollama-qa/chat:latest']['executor_id']
 hint={'id':'native-poll-priority','executor_id':executor,'priority':1}
 assert request(ctl+'/api/v1/inference-waiters',hint,binding['token'])[0]==200
 claim=request(ctl+'/api/v1/workers/local/commands/claim',{'session_id':'feature-test'},keys['worker'])
 check(claim[0]==200 and claim[1]['data']['commands']==[],'native worker defers a fresh batch while interactive admission is pending')
 request(ctl+'/api/v1/inference-waiters',{**hint,'remove':True},binding['token'])
 check(chat(queue='ahead-of-fresh-batch')[0]==200,'interactive inference can precede a freshly queued native background task')
 # Only a disposable test database is aged, avoiding a minute of idle waiting.
 sql("UPDATE vrs_tasks SET created_at=now()-interval '61 seconds' WHERE id='"+str(uuid.UUID(task_id))+"';")
 with concurrent.futures.ThreadPoolExecutor() as pool:
  waiting=pool.submit(chat,queue='behind-aged-batch')
  until(lambda:api('/api/v1/inference-queue')[1]['items'],'interactive request not waiting for aged task')
  command=request(ctl+'/api/v1/workers/local/commands/claim',{'session_id':'feature-test'},keys['worker'])
  assert command[0]==200,command[0]
  cmd=command[1]['data']['commands'][0]
  check(cmd['task_id']==task_id,'aged native background task receives the execution lane before new interactive work')
  report=request(ctl+'/api/v1/attempts/'+cmd['attempt_id']+'/report',{'session_id':'feature-test','fence':cmd['fence'],'status':'succeeded','stopped':True,'result':{}},keys['worker'])
  assert report[0]==200,report[0]
  check(waiting.result()[0]==200,'interactive inference resumes after confirmed background completion')
 # Unmodified clients acquire a permit directly through the controller.
 api('/api/v1/model-residency',{'idle_seconds':30},'PUT')
 assert chat()[0]==200 and api('/api/v1/model-residency')[1]['resident']
 before_native=request(peerurl+'/stats')[1]
 direct=request(ctl+'/api/v1/permits',{'executor_id':'batch.infer','upstream_url':'http://fixture:9999/v1','model_id':'direct-fixture','execution_token':secrets.token_hex(32)},keys['batch'],headers={'Idempotency-Key':'direct-after-residency'})
 check(direct[0]==200,'direct permit admission succeeds when the idle resident leaves enough memory')
 check(request(peerurl+'/stats')[1]['unloads']==before_native['unloads'],'direct native admission preserves fitting idle Ollama weights')
 request(ctl+'/api/v1/permits/'+direct[1]['data']['permit_id']+'/release',{'stopped':True,'evidence':'inference_request_not_started'},keys['batch'])
 assert chat()[0]==200
 before_native=request(peerurl+'/stats')[1]
 batch_again=request(ctl+'/api/v1/tasks',{**task_body,'app_task_id':'batch-after-warm'},keys['batch'],headers={'Idempotency-Key':'batch-after-warm'})
 assert batch_again[0]==202
 next_command=until(lambda:(lambda r:r[1].get('data',{}).get('commands',[]))(request(ctl+'/api/v1/workers/local/commands/claim',{'session_id':'feature-test'},keys['worker'])),'native worker did not yield the idle model')[0]
 check(next_command['task_id']==batch_again[1]['data']['task_id'],'native task polling admits work after resource and stop-proof checks')
 check(request(peerurl+'/stats')[1]['unloads']==before_native['unloads'],'native task admission preserves fitting idle Ollama weights')
 request(ctl+'/api/v1/attempts/'+next_command['attempt_id']+'/report',{'session_id':'feature-test','fence':next_command['fence'],'status':'succeeded','stopped':True,'result':{}},keys['worker'])
 # A fabricated/failed native stop receipt must never free the warm reservation.
 api('/api/v1/model-residency',{'idle_seconds':30},'PUT')
 assert chat()[0]==200
 before=request(peerurl+'/stats')[1]['calls']
 request(peerurl+'/proof-mode',{'valid':False})
 available_memory=2047
 until(lambda:sql("SELECT report->>'memory_available_mib' FROM vrs_nodes WHERE id='local';")=='2047','pressure sample not reported')
 rejected=chat('other:latest',queue='unconfirmed-switch')
 check(rejected[0]==429 and request(peerurl+'/stats')[1]['calls']==before,'failed native unload proof blocks replacement inference without replay')
 check(int(sql("SELECT count(*) FROM vrs_permits WHERE state='suspect';"))>=1,'unconfirmed residency keeps its resource commitment for native recovery')
 request(peerurl+'/proof-mode',{'valid':True})
 available_memory=12288
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','native stop recovery did not release the prior model')
 check(chat('other:latest')[0]==200,'verified native recovery permits a subsequent model call')
 request(ctl+'/api/v1/nodes/local/drain',{'draining':True},keys['admin'])
 drained=next(m for m in api('/api/v1/model-lifecycle')[1]['items'] if m['model_id']=='ollama://chat:latest')
 check('EXECUTION_NODE_UNAVAILABLE' in drained['deployments'][0]['blockers'],'lifecycle checks live node eligibility instead of trusting only a binding file')
 request(ctl+'/api/v1/nodes/local/drain',{'draining':False},keys['admin'])
 check(api('/api/v1/model-residency',{'idle_seconds':-1},'PUT')[0]==422 and api('/api/v1/model-residency',token='bob')[0]==403,'residency settings require administrator and bounded values')
 api('/api/v1/model-residency',{'idle_seconds':2},'PUT')
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','idle reservation not released before restart')
 docker('restart',app)
 until(lambda:request(ctl+'/health')[0]==200,'controller not restored after restart')
 until(lambda:api('/api/v1/model-residency')[0]==200,'restart not ready')
 check(api('/api/v1/model-residency')[1]['idle_seconds']==2,'residency policy persists through process restart')
 until(lambda:len(api('/v1/models')[1].get('data',[]))==2,'catalog not restored')
 warm=api('/model-hub/api/v1/inference-services/v-scheduler-ollama-qa/models/start',{'model':'chat:latest'})
 private('warmup-response.json',warm)
 check(warm[0]==200 and warm[1]['managed'] and warm[1]['confirmed'],'model service Start uses the same queue and reserved warmup path')
 cooled=api('/model-hub/api/v1/inference-services/v-scheduler-ollama-qa/models/stop',{'model':'chat:latest'})
 check(cooled[0]==200 and cooled[1]['confirmed'] and sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','model service Stop confirms unloading and releases the resident reservation')
 # Mutations must not race queued/running inference or invalidate warm leases.
 api('/api/v1/model-residency',{'idle_seconds':30},'PUT')
 with concurrent.futures.ThreadPoolExecutor() as pool:
  running=pool.submit(chat,text='hold')
  until(lambda:request(peerurl+'/stats')[1]['active']==1,'mutation test inference not active')
  result=api('/model-hub/api/v1/models?model_id=ollama%3A%2F%2Fchat%3Alatest',method='DELETE')
  check(result[0]==409 and not request(peerurl+'/stats')[1]['mutations'],'weight deletion rejects active inference before native mutation')
  assert running.result()[0]==200
 updating=api('/model-hub/api/v1/models/pull',{'source':'ollama','model_id':'ollama://chat:latest','force':True})
 assert updating[0]==200,updating
 check(chat()[0]==409,'new inference waits for a repository download to complete')
 until(lambda:api('/model-hub/api/v1/tasks/detail/'+updating[1]['task_id'])[1].get('phase')=='READY','updated weights not ready')
 check(all(m['active']==0 and not m['loaded'] for m in request(peerurl+'/stats')[1]['mutations']),'updating warm weights confirms native stop before pulling')
 until(lambda:chat()[0]==200,'updated weights did not receive fresh resource approval')
 check(sql("SELECT count(*) FROM vrs_permits WHERE state='suspect';")=='0','weight update keeps the resource ledger reconcilable')
 deleted=api('/model-hub/api/v1/models?model_id=ollama%3A%2F%2Fchat%3Alatest',method='DELETE')
 check(deleted[0]==200 and sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','deleting warm weights unloads and releases reservations before removing the manifest')
 check(not any(m['id'].endswith('/chat:latest') for m in api('/v1/models')[1]['data']),'deleted model is immediately removed from the owned gateway catalog')
 private('result.json',{'passed':checks,'fixture_native':True,'installed_data_touched':False})
 print(json.dumps({'passed':len(checks),'evidence':str(run/'result.json')}),flush=True)
finally:
 stop.set()
 for name in reversed(owned):
  try:
   if name.endswith(('-app','-pg')):
    logs=subprocess.run(['docker','logs',name],text=True,capture_output=True)
    private('app.log' if name.endswith('-app') else 'postgres.log',logs.stdout+logs.stderr)
   docker('rm','-f',name)
  except subprocess.CalledProcessError:pass
 try:docker('network','rm',network)
 except subprocess.CalledProcessError:pass
