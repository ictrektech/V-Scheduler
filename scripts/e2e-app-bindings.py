"""Isolated protocol fixtures, NOT production model calibration. Run on a synced build host."""
import datetime,json,os,pathlib,secrets,subprocess,threading,time,urllib.request,urllib.error

build=pathlib.Path.home()/'dev/build/V-Scheduler/model-hub-merge'
run=build/('app-binding-'+str(time.time_ns()));run.mkdir(mode=0o700)
network='vs-app-binding-'+str(os.getpid());names=[];stop=threading.Event()
admin,worker,password=[secrets.token_hex(32)for _ in range(3)]
def docker(*args,input=None):return subprocess.check_output(['docker',*args],input=input,text=True,stderr=subprocess.PIPE).strip()
def file(name,value):
 p=run/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(value if isinstance(value,str)else json.dumps(value));p.chmod(0o600);return p
def launch(suffix,args):
 name=network+'-'+suffix;names.append(name);docker('run','-d','--name',name,'--network',network,*args);return name
def address(name,port):return 'http://'+json.loads(docker('inspect',name))[0]['NetworkSettings']['Networks'][network]['IPAddress']+':'+str(port)
def request(url,body=None,token=None):
 req=urllib.request.Request(url,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json','Idempotency-Key':secrets.token_hex(16),**({'Authorization':'Bearer '+token}if token else{})})
 try:
  with urllib.request.urlopen(req,timeout=20)as response:status=response.status;raw=response.read()
 except urllib.error.HTTPError as e:status=e.code;raw=e.read()
 try:return status,json.loads(raw)
 except ValueError:return status,raw.decode()
def until(fn,message,seconds=45):
 deadline=time.monotonic()+seconds
 while time.monotonic()<deadline:
  try:
   value=fn()
   if value:return value
  except (OSError,ValueError,KeyError,subprocess.CalledProcessError):pass
  time.sleep(.25)
 raise AssertionError(message)
def sql(query):return docker('exec','-i',pg,'psql','-U','postgres','-d','test','-At','-v','ON_ERROR_STOP=1',input=query)
fixture=file('fixture.mjs',r'''
import {createServer} from 'node:http';import {randomUUID} from 'node:crypto';
const origin='http://v-music-backend:7080',upstream=origin+'/v1',controller='http://v-scheduler:38120/api/v1';
let binding=null,calls=0,claims=0;let settings={provider:'fixture',threads:1};
const send=(res,code,value)=>{res.writeHead(code,{'content-type':'application/json'});res.end(JSON.stringify(value));};
async function api(path,body,token){const res=await fetch(controller+path,{method:body===undefined?'GET':'POST',headers:{'content-type':'application/json',authorization:'Bearer '+token,'Idempotency-Key':randomUUID()},body:body===undefined?undefined:JSON.stringify(body)});return {status:res.status,data:await res.json()};}
createServer(async(req,res)=>{let raw='';for await(const chunk of req)raw+=chunk;const body=raw?JSON.parse(raw):{};
if(req.url==='/v1000/oauth2/userinfo'){send(res,req.headers.authorization==='Bearer fixture-context'?200:401,{sub:'com.ictrek.v-scheduler',enable:1});return;}
if(req.url.startsWith('/v1000/app/detail?app_id=')){send(res,200,{code:0,data:{state:'installed',configs_current:{VOS_APP_PROFILES:'amd'}}});return;}
if(req.url==='/health'){send(res,200,{ok:true});return;}
if(req.url==='/stats'){send(res,200,{calls,claims,configured:binding!==null,revision:binding?.binding_revision,settings});return;}
if(req.url==='/settings'&&req.method==='POST'){settings=body;send(res,200,{ok:true});return;}
if(req.url==='/api/v1/discovery'){send(res,200,{service_id:'v-music',name:'Fixture music',visibility:'public',models:[{id:'fixture-music',kind:'music',endpoints:['audio/music/generations']}],scheduler:{protocol_version:1,app_id:'com.ictrek.v-music',executor_base_url:origin,upstream_url:upstream,binding_endpoint:'/api/v1/scheduler/binding',execution_settings:settings,sync_models:[{id:'fixture-music',task_type:'model.infer'}]}});return;}
if(req.url==='/api/v1/scheduler/binding'){
 if(req.headers.authorization!=='Bearer fixture-context'){send(res,403,{});return;}
 if(JSON.stringify(body.execution_settings)!==JSON.stringify(settings)||!body.model_limits?.['fixture-music']){send(res,422,{});return;}
 const found=await api('/executors',undefined,body.token);const rows=found.data.data?.executors||found.data.data;
 const ids=new Set(Array.isArray(rows)?rows.map(row=>row.id):[]);if(found.status!==200||Object.values(body.models).some(id=>!ids.has(id))){send(res,422,{});return;}
 if(binding&&body.binding_revision<binding.binding_revision){send(res,409,{});return;}binding=body;send(res,200,{configured:true});return;
}
if(req.url==='/v1/audio/music/generations'||req.url==='/direct'){
 if(!binding||JSON.stringify(binding.execution_settings)!==JSON.stringify(settings)||!binding.models[body.model]){send(res,503,{error:'no measured binding'});return;}
 if(body.duration>binding.model_limits[body.model].duration_seconds){send(res,422,{error:'input outside measured range'});return;}
 let permit=req.headers['x-vscheduler-permit-id'];const via=Boolean(permit);
 if(via){const result=await api('/permits/'+permit+'/verify',{executor_id:binding.gateway_executors[body.model],upstream_url:upstream,model_id:body.model,execution_token:req.headers['x-vscheduler-execution-token'],execution_id:randomUUID(),claim:true},binding.token);if(result.status!==200){send(res,403,{});return;}claims++;}
 else{const result=await api('/permits',{executor_id:binding.models[body.model],upstream_url:upstream,model_id:body.model},binding.token);if(result.status!==200){send(res,result.status,{});return;}permit=result.data.data.permit_id;}
 calls++;if(!via)await api('/permits/'+permit+'/release',{stopped:true,evidence:'fixture_computation_stopped'},binding.token);
 send(res,200,{audio:'fixture-only',model:body.model});return;
}
send(res,404,{});
}).listen(7080);
''')
controller=None;gateway=None
try:
 docker('network','create','--internal',network)
 pg=launch('pg',['--env-file',str(file('pg.env',f'POSTGRES_DB=test\nPOSTGRES_PASSWORD={password}\n')),os.environ.get('SCHEDULER_TEST_PG_IMAGE','postgres:17-bookworm')])
 until(lambda:'accepting connections'in docker('exec',pg,'pg_isready','-U','postgres'),'database not ready')
 fixture_name=launch('fixture',['--network-alias','fixture','--network-alias','v-music-backend','-v',f'{fixture}:/fixture.mjs:ro','--entrypoint','node','v-scheduler-ui-verify:0.0.19','/fixture.mjs'])
 fixture_url=address(fixture_name,7080)
 config=file('config/controller.json',{'principals':[{'token':admin,'role':'admin','app_id':'test'},{'token':worker,'role':'worker','node_id':'local','app_id':'test-worker'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':1000,'memory_mib':8192}}],'executors':[],'local_agent':{'node_id':'local','automatic_capacity':False,'executors':[]}})
 file('config/local-agent.json',{'controller':'http://v-scheduler:38120/api/v1','node_id':'local','token':worker,'journal':'/config/journal.json','executors':[]})
 row={'app_id':'com.ictrek.v-music','architecture':'x86_64','profile':'amd','hardware':[],'execution_settings':{'provider':'fixture','threads':2},'model_id':'fixture-music','task_type':'model.infer','resources':{'cpu_millicores':1000,'memory_mib':512},'evidence':{'id':'isolated-protocol-fixture-only','peak_memory_mib':256,'confirmed_stopped':True},'max_input_bytes':8192,'model_limits':{'duration_seconds':10,'prompt_characters':100,'negative_prompt_characters':100}}
 row['limits']={'duration':10}
 approvals={'protocol_version':1,'credential_epoch':1,'approvals':[row]};approval_path=file('config/app-approvals.json',approvals)
 data=run/'data';data.mkdir(mode=0o700);context=file('context/access-token','fixture-context')
 pgip=json.loads(docker('inspect',pg))[0]['NetworkSettings']['Networks'][network]['IPAddress']
 env=file('controller.env',f'SCHEDULER_DATABASE_URL=postgres://postgres:{password}@{pgip}:5432/test\nSCHEDULER_EMBEDDED_AGENT=false\nVOS_APP_VERSION=protocol-fixture\nSCHEDULER_VOS_URL=http://fixture:7080\n')
 controller=launch('controller',['--user',str(os.getuid())+':'+str(os.getgid()),'--network-alias','v-scheduler','--env-file',str(env),'-v',f'{config.parent}:/config','-v',f'{context.parent}:/var/lib/vos-context:ro','-v',f'{data}:/gateway-data','-v',f'{build}/target:/target:ro','--entrypoint','/target/debug/v-scheduler','v-scheduler-modelhub-sdk:20261016','controller','--config','/config/controller.json','--listen','0.0.0.0:38120'])
 api=address(controller,38120);until(lambda:request(api+'/health')[0]==200,'controller not ready')
 def heartbeat():
  while not stop.is_set():
   try:request(api+'/api/v1/workers/local/heartbeat',{'session_id':'test','report':{'observed_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'cpu_logical_count':4,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':12288,'gpus':[]}},worker)
   except OSError:pass
   stop.wait(1)
 threading.Thread(target=heartbeat,daemon=True).start()
 time.sleep(2);assert request(fixture_url+'/stats')[1]['configured']is False
 assert sql('SELECT count(*) FROM vrs_executor_settings;')=='0'
 print('PASS settings mismatch cannot authorize discovered models',flush=True)
 row['execution_settings']['threads']=1;file('config/app-approvals.json',approvals)
 until(lambda:request(fixture_url+'/stats')[1]['configured'],'calibrated binding not delivered')
 state=json.loads((run/'config/app-bindings/v-music.json').read_text());assert (run/'config/app-bindings/v-music.json').stat().st_mode&0o077==0
 gatewayenv=file('gateway.env','\n'.join(['V_ROUTER_AUTH_DISABLED=true','V_ROUTER_SCHEDULER_REQUIRED=true','V_ROUTER_STATE_PATH=/data/state.json','V_ROUTER_CONTROLLER_URL=http://v-scheduler:38120','V_ROUTER_DISCOVERY_INTERVAL=1s','V_ROUTER_V_MUSIC_URLS=http://v-music-backend:7080',*[key+'='for key in ['V_ROUTER_MODEL_HUB_URL','V_ROUTER_IMAGE_STUDIO_URL','V_ROUTER_V_VOICE_URLS','V_ROUTER_ML_SERVER_URLS','V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS','V_ROUTER_VIDEO_STRUCTURE_URLS','V_ROUTER_V_SUB_URLS']]])+'\n')
 gateway=launch('gateway',['--network-alias','v-router','--env-file',str(gatewayenv),'-v',f'{data}:/data','-v',f'{build}/go:/verify:ro','--entrypoint','/verify/v-router','swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-router-base:golang-1.23-alpine-20260910-amd64'])
 gw=address(gateway,8080);until(lambda:request(gw+'/v1/models')[0]==200,'gateway not ready')
 until(lambda:any(m['id']=='v-music/fixture-music'for m in request(gw+'/v1/models')[1].get('data',[])),'music not discovered')
 before=int(sql('SELECT count(*) FROM vrs_permits;'))
 result=request(gw+'/v1/audio/music/generations',{'model':'v-music/fixture-music','prompt':'fixture','duration':10})
 if result[0]!=200:
  proof=request(api+'/api/v1/permits',{'executor_id':state['binding']['gateway_executors']['fixture-music'],'upstream_url':'http://v-music-backend:7080/v1','model_id':'fixture-music'},state['gateway']['token'])
  if proof[0]==200:request(api+'/api/v1/permits/'+proof[1]['data']['permit_id']+'/release',{'stopped':True,'evidence':'diagnostic_only_no_backend_execution'},state['gateway']['token'])
  raise AssertionError((result,proof))
 until(lambda:sql("SELECT count(*) FROM vrs_permits WHERE state<>'released';")=='0','gateway permit not released')
 assert int(sql('SELECT count(*) FROM vrs_permits;'))==before+1
 assert request(fixture_url+'/stats')[1]['claims']==1
 assert request(fixture_url+'/direct',{'model':'fixture-music','duration':10})[0]==200
 assert int(sql('SELECT count(*) FROM vrs_permits;'))==before+2
 assert request(fixture_url+'/direct',{'model':'unknown','duration':1})[0]==503
 assert request(fixture_url+'/direct',{'model':'fixture-music','duration':11})[0]==422
 assert request(gw+'/v1/audio/music/generations',{'model':'v-music/fixture-music','duration':11})[0]==422
 print('PASS scoped binding, gateway claim without duplicate budget, direct permits and input bounds',flush=True)
 old_token=state['binding']['token'];old_revision=state['binding']['binding_revision']
 held=request(api+'/api/v1/permits',{'executor_id':state['binding']['models']['fixture-music'],'upstream_url':'http://v-music-backend:7080/v1','model_id':'fixture-music'},old_token)
 assert held[0]==200,held
 approvals['credential_epoch']=2;file('config/app-approvals.json',approvals)
 time.sleep(11)
 assert request(fixture_url+'/stats')[1]['revision']==old_revision,'rotation invalidated an active permit'
 assert request(api+'/api/v1/permits/'+held[1]['data']['permit_id']+'/release',{'stopped':True,'evidence':'fixture_admission_held_without_starting_execution'},old_token)[0]==200
 until(lambda:request(fixture_url+'/stats')[1].get('revision',0)>old_revision,'credential rotation not delivered')
 assert request(api+'/api/v1/executors',token=old_token)[0]in [401,403]
 fresh=json.loads((run/'config/app-bindings/v-music.json').read_text());assert fresh['binding']['token']!=old_token
 docker('restart',controller);until(lambda:request(api+'/health')[0]==200,'restart failed')
 until(lambda:request(fixture_url+'/direct',{'model':'fixture-music','duration':1})[0]==200,'restart lost scoped identity')
 assert json.loads((run/'config/app-bindings/v-music.json').read_text())['binding']['binding_revision']==fresh['binding']['binding_revision']
 request(fixture_url+'/settings',{'provider':'fixture','threads':2})
 until(lambda:sql("SELECT count(*) FROM vrs_executor_settings WHERE state='enabled';")=='0','runtime drift retained execution grants')
 assert request(fixture_url+'/direct',{'model':'fixture-music','duration':1})[0]==503
 assert request(gw+'/v1/audio/music/generations',{'model':'v-music/fixture-music','duration':1})[0]==503
 print('PASS credential revocation, persistent revisions and runtime drift fail closed',flush=True)
 print('APPLICATION_BINDING_PROTOCOL_ACCEPTANCE_PASSED',flush=True)
finally:
 stop.set()
 for name in reversed(names):
  try:
   if name in [controller,gateway]:file(name+'.log',docker('logs',name))
   docker('rm','-f',name)
  except subprocess.CalledProcessError:pass
 try:docker('network','rm',network)
 except subprocess.CalledProcessError:pass
