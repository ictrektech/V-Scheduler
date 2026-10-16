"""Isolated V-Sub namespace protocol fixtures, NOT production model calibration."""
import datetime,json,os,pathlib,secrets,subprocess,threading,time,urllib.request,urllib.error
build=pathlib.Path.home()/'dev/build/V-Scheduler/model-hub-merge';run=build/('vsub-binding-'+str(time.time_ns()));run.mkdir(mode=0o700)
network='vs-vsub-binding-'+secrets.token_hex(5);names=[];stop=threading.Event();admin,worker,password=[secrets.token_hex(32)for _ in range(3)]
def docker(*args,input=None):return subprocess.check_output(['docker',*args],input=input,text=True,stderr=subprocess.PIPE).strip()
def private(name,value):
 p=run/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(value if isinstance(value,str)else json.dumps(value));p.chmod(0o600);return p
def launch(suffix,args):
 name=network+'-'+suffix;names.append(name);docker('run','-d','--name',name,'--network',network,*args);return name
def address(name,port):return 'http://'+json.loads(docker('inspect',name))[0]['NetworkSettings']['Networks'][network]['IPAddress']+':'+str(port)
def request(url,body=None,token=None):
 req=urllib.request.Request(url,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json','Idempotency-Key':secrets.token_hex(16),**({'Authorization':'Bearer '+token}if token else{})})
 try:
  with urllib.request.urlopen(req,timeout=15)as response:status=response.status;raw=response.read()
 except urllib.error.HTTPError as e:status=e.code;raw=e.read()
 try:return status,json.loads(raw)
 except ValueError:return status,None
def until(fn,message,seconds=60):
 deadline=time.monotonic()+seconds
 while time.monotonic()<deadline:
  try:
   result=fn()
   if result:return result
  except (OSError,ValueError,KeyError,TypeError,subprocess.CalledProcessError):pass
  time.sleep(.25)
 raise AssertionError(message)
def sql(query):return docker('exec','-i',pg,'psql','-U','postgres','-d','test','-At','-v','ON_ERROR_STOP=1',input=query)
fixture=private('fixture.mjs',r'''
import {createServer}from'node:http';import{createHash,randomUUID}from'node:crypto';
const app='com.ictrek.v-sub',owner='user-123',namespace=createHash('sha256').update('v-sub\0'+owner).digest('hex'),id='v-sub-'+namespace.slice(0,12)+'/builtin/tiny-fixture',origin='http://v-sub-backend:7090/inference/'+namespace+'/v1';
let binding=null,generation='a'.repeat(32),present=true,calls=0,claims=0;
const send=(res,status,value)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(value));};
async function api(path,body,token){const res=await fetch('http://v-scheduler:38120/api/v1'+path,{method:body===undefined?'GET':'POST',headers:{'content-type':'application/json',authorization:'Bearer '+token,'Idempotency-Key':randomUUID()},body:body===undefined?undefined:JSON.stringify(body)});return {status:res.status,data:await res.json()};}
createServer(async(req,res)=>{let raw='';for await(const c of req)raw+=c;const body=raw?JSON.parse(raw):{};
if(req.url==='/v1000/oauth2/userinfo'){const token=req.headers.authorization;send(res,['Bearer fixture-context','Bearer alice-token','Bearer bob-token'].includes(token)?200:401,token==='Bearer fixture-context'?{sub:'com.ictrek.v-scheduler',enable:1}:{sub:token==='Bearer alice-token'?owner:'user-456',preferred_username:token==='Bearer alice-token'?'alice':'bob'});return;}
if(req.url.startsWith('/v1000/app/detail?app_id=')){send(res,200,req.url.endsWith(app)?{code:0,data:{state:'installed',configs_current:{VOS_APP_PROFILES:'l4t'}}}:{code:10067});return;}
if(req.url==='/api/v1/scheduler/discovery'){
if(req.headers.authorization!=='Bearer fixture-context'){send(res,403,{});return;}
send(res,200,{protocol_version:1,app_id:app,capabilities:['namespace-inference-v1'],binding_endpoint:'/api/v1/scheduler/binding',models:present?[{id,native_id:'builtin/tiny-fixture',namespace,generation,task_type:'model.infer',upstream_url:origin}]:[]});return;}
if(req.url==='/api/v1/scheduler/binding'){
if(req.headers.authorization!=='Bearer fixture-context'||body.namespaces?.[id]!==namespace||body.model_generations?.[id]!==generation||body.upstreams?.[id]!==origin){send(res,403,{});return;}
const result=await api('/executors',undefined,body.token);if(result.status!==200){send(res,422,{});return;}binding=body;send(res,200,{configured:true,binding_revision:body.binding_revision});return;}
if(req.url==='/api/v1/discovery'){
if(req.headers.authorization!=='Bearer alice-token'){send(res,200,{owner:'user-456',models:[]});return;}
send(res,200,{owner,base_path:'/inference/'+namespace+'/v1',api_key:'fixture-inference-token',models:present?[{id,kind:'asr',endpoints:['audio/transcriptions'],capabilities:['asr']}]:[]});return;}
if(req.url==='/stats'){send(res,200,{configured:!!binding,revision:binding?.binding_revision,id,namespace,generation,calls,claims});return;}
if(req.url==='/state'){generation=body.generation??generation;present=body.present??present;send(res,200,{ok:true});return;}
if(req.url==='/direct'||req.url==='/inference/'+namespace+'/v1/audio/transcriptions'){
if(!present||!binding||binding.model_generations[id]!==generation){send(res,503,{});return;}
const inherited=req.url!=='/direct';if(inherited&&req.headers.authorization!=='Bearer fixture-inference-token'){send(res,401,{});return;}
let permit,proof,executor;if(inherited){permit=req.headers['x-vscheduler-permit-id'];proof=req.headers['x-vscheduler-execution-token'];executor=req.headers['x-vscheduler-executor-id'];if(executor!==binding.gateway_executors[id]){send(res,403,{});return;}}else{executor=binding.models[id];proof=randomUUID()+randomUUID();const result=await api('/permits',{executor_id:executor,upstream_url:origin,model_id:id,execution_token:proof},binding.token);if(result.status!==200){send(res,result.status,{});return;}permit=result.data.data.permit_id;}
const claim=await api('/permits/'+permit+'/verify',{executor_id:executor,upstream_url:origin,model_id:id,execution_token:proof,execution_id:randomUUID(),claim:true},binding.token);if(claim.status!==200){send(res,claim.status,{});return;}calls++;if(inherited)claims++;else await api('/permits/'+permit+'/release',{stopped:true,evidence:'isolated_fixture_no_model_process'},binding.token);send(res,200,{text:'fixture transcript'});return;}
send(res,404,{});}).listen(7090,'0.0.0.0');
''')
try:
 docker('network','create','--internal',network)
 pg=launch('pg',['--env-file',str(private('pg.env',f'POSTGRES_DB=test\nPOSTGRES_PASSWORD={password}\n')),'postgres:17-bookworm']);until(lambda:'accepting connections'in docker('exec',pg,'pg_isready','-U','postgres'),'database unavailable')
 fixture_name=launch('fixture',['--network-alias','fixture','--network-alias','v-sub-backend','-v',f'{fixture}:/fixture.mjs:ro','--entrypoint','node','v-scheduler-ui-verify:0.0.19','/fixture.mjs']);fx=address(fixture_name,7090)
 config=private('config/controller.json',{'principals':[{'token':admin,'role':'admin','app_id':'test'},{'token':worker,'role':'worker','node_id':'local','app_id':'test-worker'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':4500,'memory_mib':8192}}],'executors':[],'local_agent':{'node_id':'local','automatic_capacity':False,'executors':[]}})
 private('config/app-approvals.json',{'protocol_version':1,'credential_epoch':1,'approvals':[]})
 approval={'architecture':'x86_64','profile':'l4t','hardware':[],'native_model':'builtin/tiny-fixture','execution_settings':{'profile':'l4t','runtime':'isolated-fixture'},'resources':{'cpu_millicores':4500,'memory_mib':2048},'max_input_bytes':8192,'model_limits':{'input_bytes':8192,'duration_seconds':20,'prompt_characters':0},'limits':{},'evidence':{'id':'isolated-fixture-only','confirmed_stopped':True,'peak_memory_mib':800,'observed_peak_cpu_millicores':4021}}
 private('config/vsub-approvals.json',{'protocol_version':1,'credential_epoch':1,'approvals':[approval]})
 context=private('context/access-token','fixture-context');data=run/'data';data.mkdir(mode=0o700)
 pgip=json.loads(docker('inspect',pg))[0]['NetworkSettings']['Networks'][network]['IPAddress'];env=private('controller.env',f'SCHEDULER_DATABASE_URL=postgres://postgres:{password}@{pgip}:5432/test\nSCHEDULER_EMBEDDED_AGENT=false\nSCHEDULER_VOS_URL=http://fixture:7090\nVOS_APP_VERSION=isolated-vsub-test\n')
 controller=launch('controller',['--network-alias','v-scheduler','--env-file',str(env),'-v',f'{config.parent}:/config','-v',f'{context.parent}:/var/lib/vos-context:ro','-v',f'{data}:/gateway-data','-v',f'{build}/target:/target:ro','--entrypoint','/target/debug/v-scheduler','v-scheduler-modelhub-sdk:20261016','controller','--config','/config/controller.json','--listen','0.0.0.0:38120']);api=address(controller,38120)
 until(lambda:request(api+'/health')[0]==200,'controller unavailable')
 def heartbeat():
  while not stop.is_set():
   try:request(api+'/api/v1/workers/local/heartbeat',{'session_id':'fixture','report':{'observed_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'cpu_logical_count':8,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':12288,'gpus':[]}},worker)
   except OSError:pass
   stop.wait(1)
 threading.Thread(target=heartbeat,daemon=True).start()
 until(lambda:request(fx+'/stats')[1].get('configured'),'root broker binding not delivered')
 state=json.loads(docker('exec',controller,'cat','/config/app-bindings/v-sub.json'));stats=request(fx+'/stats')[1]
 gatewayenv=private('gateway.env','\n'.join(['V_ROUTER_STATE_PATH=/data/state.json','V_ROUTER_SCHEDULER_REQUIRED=true','V_ROUTER_CONTROLLER_URL=http://v-scheduler:38120','VOS_OIDC_USERINFO_URL=http://v-sub-backend:7090/v1000/oauth2/userinfo','V_ROUTER_V_SUB_URLS=http://v-sub-backend:7090',*[k+'='for k in ['V_ROUTER_MODEL_HUB_URL','V_ROUTER_IMAGE_STUDIO_URL','V_ROUTER_V_VOICE_URLS','V_ROUTER_V_MUSIC_URLS','V_ROUTER_ML_SERVER_URLS','V_ROUTER_VIDEO_STRUCTURE_URLS','V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS']]])+'\n')
 gateway=launch('gateway',['--network-alias','v-router','--env-file',str(gatewayenv),'-v',f'{data}:/data','-v',f'{build}/go:/verify:ro','--entrypoint','/verify/v-router','swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-router-base:golang-1.23-alpine-20260910-amd64']);gw=address(gateway,8080)
 qualified='v-sub/'+stats['id'];until(lambda:any(m['id']==qualified for m in request(gw+'/v1/models',token='alice-token')[1].get('data',[])),'private model not discovered')
 assert not any(m['id']==qualified for m in request(gw+'/v1/models',token='bob-token')[1].get('data',[]))
 before=int(sql('SELECT count(*) FROM vrs_permits;'))
 result=request(gw+'/v1/audio/transcriptions',{'model':qualified,'input':'fixture'},'alice-token');assert result[0]==200,result
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','gateway permit not released')
 assert int(sql('SELECT count(*) FROM vrs_permits;'))==before+1
 assert request(fx+'/stats')[1]['claims']==1
 assert request(fx+'/direct',{'input':'fixture'})[0]==200
 assert int(sql('SELECT count(*) FROM vrs_permits;'))==before+2
 print('PASS private discovery isolation and exactly one claimed gateway/direct permit',flush=True)
 held=request(api+'/api/v1/permits',{'executor_id':state['binding']['models'][stats['id']],'upstream_url':state['binding']['upstreams'][stats['id']],'model_id':stats['id']},state['binding']['token']);assert held[0]==200,held
 old_revision=stats['revision'];request(fx+'/state',{'generation':'b'*32});time.sleep(11)
 assert request(fx+'/stats')[1]['revision']==old_revision
 assert request(fx+'/direct',{})[0]==503
 assert request(api+'/api/v1/permits/'+held[1]['data']['permit_id']+'/release',{'stopped':True,'evidence':'fixture_held_admission_without_compute'},state['binding']['token'])[0]==200
 until(lambda:request(fx+'/stats')[1]['revision']>old_revision,'new worker generation not rebound')
 fresh=json.loads(docker('exec',controller,'cat','/config/app-bindings/v-sub.json'));assert fresh['binding']['models'][stats['id']]!=state['binding']['models'][stats['id']]
 assert request(fx+'/direct',{})[0]==200
 print('PASS live generation change waits for prior permit and uses new executor identity',flush=True)
 print('VSUB_BINDING_PROTOCOL_ACCEPTANCE_PASSED',flush=True)
finally:
 stop.set()
 for name in reversed(names):subprocess.run(['docker','rm','-f',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 subprocess.run(['docker','network','rm',network],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
