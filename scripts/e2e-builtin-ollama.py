"""Run on a synced Linux build host. Isolated DB/network; no real models or VOS data."""
import concurrent.futures, datetime, json, os, pathlib, secrets, subprocess, threading, time, urllib.error, urllib.request

build=pathlib.Path.home()/'dev/build/V-Scheduler/model-hub-merge'
run=build/('auto-ollama-'+str(time.time_ns()));run.mkdir(mode=0o700)
network='vs-auto-ollama-'+str(os.getpid());names=[];stop=threading.Event()
admin,worker,password=[secrets.token_hex(32) for _ in range(3)]
def docker(*args,input=None):
 return subprocess.check_output(['docker',*args],input=input,text=True,stderr=subprocess.PIPE).strip()
def file(name,value):
 p=run/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(value if isinstance(value,str) else json.dumps(value));p.chmod(0o600);return p
def launch(suffix,args):
 name=network+'-'+suffix;names.append(name);docker('run','-d','--name',name,'--network',network,*args);return name
def address(name,port):
 info=json.loads(docker('inspect',name))[0];return 'http://'+info['NetworkSettings']['Networks'][network]['IPAddress']+':'+str(port)
def request(url,body=None,token=None,timeout=20,idem=None):
 r=urllib.request.Request(url,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json',**({'Authorization':'Bearer '+token}if token else{}),**({'Idempotency-Key':idem}if idem else{})})
 try:
  with urllib.request.urlopen(r,timeout=timeout)as response:status=response.status;raw=response.read()
 except urllib.error.HTTPError as e:status=e.code;raw=e.read()
 try:return status,json.loads(raw)
 except ValueError:return status,raw.decode()
def until(fn,message,attempts=160):
 for _ in range(attempts):
  try:
   result=fn()
   if result:return result
  except (OSError,ValueError,KeyError,subprocess.CalledProcessError):pass
  time.sleep(.2)
 raise AssertionError(message)
def sql(query):return docker('exec','-i',pg,'psql','-U','postgres','-d','test','-At','-v','ON_ERROR_STOP=1',input=query)
fixture=file('fixture.mjs',r'''
import {createServer} from 'node:http';
let active=0,peak=0,calls=0,stopCalls=0,proofValid=false;
const models=[{name:'chat:latest',size:1048576},{name:'embed:latest',size:1048576},{name:'rank:latest',size:1048576}];
const handler=async(req,res)=>{res.setHeader('Content-Type','application/json');let text='';for await(const c of req)text+=c;const body=text?JSON.parse(text):{};
if(req.url==='/api/tags'){res.end(JSON.stringify({models}));return;}
if(req.url==='/api/show'){res.end(JSON.stringify({capabilities:body.model.startsWith('chat')?['completion']:body.model.startsWith('embed')?['embedding']:['completion','rerank']}));return;}
if(req.url==='/v1/models'){res.end(JSON.stringify({data:models.map(m=>({id:m.name}))}));return;}
if(req.url==='/stats'){res.end(JSON.stringify({active,peak,calls,stopCalls}));return;}
if(req.url==='/proof-mode'){proofValid=body.valid;res.end('{}');return;}
if(req.url==='/api/scheduler/confirm-stopped'){stopCalls++;res.end(JSON.stringify({protocol_version:1,model:body.model,nonce:proofValid?body.nonce:'stale',stopped:true,native_exit_barrier:true}));return;}
if(req.url.startsWith('/v1/')){if(!req.headers['x-vscheduler-permit-id']||body.keep_alive!==0){res.writeHead(500);res.end('{"error":"missing scoped permit or release setting"}');return;}
active++;peak=Math.max(peak,active);calls++;
if(body.messages?.[0]?.content==='hold')await new Promise(r=>setTimeout(r,1800));
let data=req.url.endsWith('/embeddings')?{data:[{index:0,embedding:[1,2,3]}]}:req.url.endsWith('/rerank')?{results:[{index:0,relevance_score:.9}]}:{choices:[{message:{role:'assistant',content:'Hello'}}]};
active--;if(body.stream){res.setHeader('Content-Type','text/event-stream');res.end('data: '+JSON.stringify({choices:[{delta:{content:'Hello'}}]})+'\n\ndata: [DONE]\n\n');}else res.end(JSON.stringify(data));return;}
res.writeHead(404);res.end('{}');};createServer(handler).listen(11434);createServer(handler).listen(11535);
''')
controller=None;gateway=None
try:
 docker('network','create','--internal',network)
 pg=launch('pg',['--env-file',str(file('pg.env',f'POSTGRES_USER=postgres\nPOSTGRES_DB=test\nPOSTGRES_PASSWORD={password}\n')),os.environ.get('SCHEDULER_TEST_PG_IMAGE','postgres:17-bookworm')])
 until(lambda:'accepting connections'in docker('exec',pg,'pg_isready','-U','postgres'),'database not ready')
 launch('fixture',['--network-alias','v-scheduler-ollama-qa','--network-alias','v-scheduler-ollama-embedding','--network-alias','v-scheduler-ollama-rerank','-v',f'{fixture}:/fixture.mjs:ro','--entrypoint','node','v-scheduler-ui-verify:0.0.19','/fixture.mjs'])
 config=file('config/controller.json',{'principals':[{'token':admin,'role':'admin','app_id':'test'},{'token':worker,'role':'worker','node_id':'local','app_id':'test-worker'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':1000,'memory_mib':8192}}],'executors':[],'local_agent':{'node_id':'local','automatic_capacity':False,'executors':[]}})
 data=run/'data';data.mkdir(mode=0o700)
 pgip=json.loads(docker('inspect',pg))[0]['NetworkSettings']['Networks'][network]['IPAddress']
 env=file('controller.env',f'SCHEDULER_DATABASE_URL=postgres://postgres:{password}@{pgip}:5432/test\nSCHEDULER_EMBEDDED_AGENT=false\nVOS_APP_VERSION=automatic-inference-test\n')
 controller=launch('controller',['--user',str(os.getuid())+':'+str(os.getgid()),'--network-alias','v-scheduler','--env-file',str(env),'-v',f'{config.parent}:/config','-v',f'{data}:/gateway-data','-v',f'{build}/target:/target:ro','--entrypoint','/target/debug/v-scheduler','v-scheduler-modelhub-sdk:20261016','controller','--config','/config/controller.json','--listen','0.0.0.0:38120'])
 api=address(controller,38120)
 until(lambda:request(api+'/health')[0]==200,'controller not ready')
 def heartbeat():
  while not stop.is_set():
   now=datetime.datetime.now(datetime.timezone.utc).isoformat()
   try:request(api+'/api/v1/workers/local/heartbeat',{'session_id':'test','report':{'observed_at':now,'cpu_logical_count':4,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':12288,'gpus':[]}},worker)
   except (OSError,ValueError):pass
   stop.wait(1)
 thread=threading.Thread(target=heartbeat,daemon=True);thread.start()
 until(lambda:len(list((data/'ollama-bindings').glob('*.json')))==9,'own models were not automatically registered')
 for p in (data/'ollama-bindings').glob('*.json'):
  assert p.stat().st_mode&0o077==0
  value=json.loads(p.read_text());assert len(value['models'])==1 and len(value['token'])>=32
 count=sql('SELECT count(*) FROM vrs_service_identities;')
 time.sleep(6);assert sql('SELECT count(*) FROM vrs_service_identities;')==count,'polling rotated identities'
 print('PASS automatic model enrollment, private credentials and stable polling',flush=True)
 gatewayenv=file('gateway.env','\n'.join(['V_ROUTER_AUTH_DISABLED=true','V_ROUTER_SCHEDULER_REQUIRED=true','V_ROUTER_STATE_PATH=/data/state.json','V_ROUTER_CONTROLLER_URL=http://v-scheduler:38120','V_ROUTER_DISCOVERY_INTERVAL=1s',*[key+'='for key in ['V_ROUTER_MODEL_HUB_URL','V_ROUTER_IMAGE_STUDIO_URL','V_ROUTER_V_VOICE_URLS','V_ROUTER_V_MUSIC_URLS','V_ROUTER_ML_SERVER_URLS','V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS','V_ROUTER_VIDEO_STRUCTURE_URLS','V_ROUTER_V_SUB_URLS']]])+'\n')
 gateway=launch('gateway',['--env-file',str(gatewayenv),'-v',f'{data}:/data','-v',f'{build}/go:/verify:ro','--entrypoint','/verify/v-router','swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-router-base:golang-1.23-alpine-20260910-amd64'])
 gw=address(gateway,8080)
 models=until(lambda:(lambda result:result[1]['data']if result[0]==200 and len(result[1].get('data',[]))==3 else False)(request(gw+'/v1/models')),'compatible model discovery failed')
 assert {m['id']for m in models}=={'v-scheduler/ollama-qa/chat:latest','v-scheduler/ollama-embedding/embed:latest','v-scheduler/ollama-rerank/rank:latest'}
 print('PASS native capabilities route chat/embedding/rerank to matching backends',flush=True)
 def chat(text='hello',stream=False):
  if text!='hold':until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state='active';")=='0','previous response still releasing')
  return request(gw+'/v1/chat/completions',{'model':'v-scheduler/ollama-qa/chat:latest','messages':[{'role':'user','content':text}],'stream':stream,'max_tokens':4096,'options':{'num_ctx':24576}})
 assert chat()[0]==200
 status,result=chat(stream=True);assert status==200 and '[DONE]'in result and 'Hello'in result
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state='active';")=='0','SSE release pending')
 assert request(gw+'/v1/embeddings',{'model':'v-scheduler/ollama-embedding/embed:latest','input':'hello'})[0]==200
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state='active';")=='0','embedding release pending')
 assert request(gw+'/v1/rerank',{'model':'v-scheduler/ollama-rerank/rank:latest','query':'hello','documents':['hello']})[0]==200
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state='active';")=='0','rerank release pending')
 with concurrent.futures.ThreadPoolExecutor()as pool:
  first=pool.submit(chat,'hold');time.sleep(.3);second=pool.submit(chat,'hold');assert first.result()[0]==200 and second.result()[0]==429
 fixtureurl=address(network+'-fixture',11535);assert request(fixtureurl+'/stats')[1]['peak']==1,'compute overlapped'
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','inference left unreleased reservations')
 print('PASS unified real transport chat/SSE/embedding/rerank, immediate busy response and permit release',flush=True)
 # An interrupted own request must recover only after a fresh native receipt.
 binding=next(json.loads(p.read_text())for p in (data/'ollama-bindings').glob('*.json')if 'v-scheduler/ollama-qa/chat:latest'in json.loads(p.read_text())['models'])
 policy=binding['models']['v-scheduler/ollama-qa/chat:latest'];owner=binding['token']
 status,value=request(api+'/api/v1/permits',{'executor_id':policy['executor_id'],'upstream_url':'http://v-scheduler-ollama-qa:11535/v1','model_id':'chat:latest'},owner,idem=secrets.token_hex(16));assert status==200
 permit=value['data']['permit_id'];calls_before=request(fixtureurl+'/stats')[1]['stopCalls'];time.sleep(6)
 assert request(fixtureurl+'/stats')[1]['stopCalls']==calls_before,'active permit was probed for stopping'
 assert request(api+'/api/v1/permits/'+permit+'/release',{'stopped':False,'evidence':'fixture_interrupted_transport'},owner)[0]==200
 time.sleep(6);assert sql("SELECT state FROM vrs_permits WHERE id='"+permit+"';")=='suspect','stale proof released execution'
 assert chat()[0]==429,'suspect execution did not retain budget'
 sql("UPDATE vrs_permits SET app='fixture-external' WHERE id='"+permit+"';")
 request(fixtureurl+'/proof-mode',{'valid':True});time.sleep(6)
 assert sql("SELECT state FROM vrs_permits WHERE id='"+permit+"';")=='suspect','unrelated application was recovered'
 sql("UPDATE vrs_permits SET app='com.ictrek.v-scheduler.ollama' WHERE id='"+permit+"';")
 until(lambda:sql("SELECT state||':'||evidence FROM vrs_permits WHERE id='"+permit+"';")=='released:builtin_ollama_native_stop_confirmed','confirmed native stop did not recover budget')
 assert chat()[0]==200,'new request was still blocked after verified recovery'
 print('PASS active work preserved, stale receipt rejected, other app untouched, native stop auto-releases and retry works',flush=True)
 before=sql('SELECT count(*) FROM vrs_service_identities;');docker('restart',controller)
 until(lambda:request(api+'/health')[0]==200,'controller restart not ready');time.sleep(1)
 assert chat()[0]==200 and sql('SELECT count(*) FROM vrs_service_identities;')==before
 print('PASS restart retains bindings and inference without per-model approval',flush=True)
 print('BUILTIN_OLLAMA_ACCEPTANCE_PASSED',flush=True)
finally:
 stop.set()
 for name in reversed(names):
  try:
   if name in [controller,gateway]:file(name+'.log',docker('logs',name))
   docker('rm','-f',name)
  except subprocess.CalledProcessError:pass
 try:docker('network','rm',network)
 except subprocess.CalledProcessError:pass
