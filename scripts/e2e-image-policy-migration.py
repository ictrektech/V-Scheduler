"""Isolated protocol fixtures, NOT production model calibration. Run on a synced build host."""
import datetime,json,os,pathlib,secrets,subprocess,threading,time,urllib.request,urllib.error

build=pathlib.Path.home()/'dev/build/V-Scheduler'
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
fixture=file('fixture.mjs',"import {createServer} from 'node:http';\nlet binding=null,restart=false,threads=1;\nconst origin='http://image-studio-backend:5000';\ncreateServer(async(req,res)=>{let raw='';for await(const c of req)raw+=c;const b=raw?JSON.parse(raw):{};let value={};\nif(req.url==='/v1000/oauth2/userinfo')value={sub:'com.ictrek.v-scheduler',enable:1};\nelse if(req.url.startsWith('/v1000/app/detail?'))value={code:0,data:{state:'installed',configs_current:{VOS_APP_PROFILES:'amd-cuda'}}};\nelse if(req.url==='/api/v1/v-router/registration')value={scheduler:{protocol_version:1,app_id:'com.ictrek.image-studio',executor_base_url:origin,upstream_url:origin,binding_endpoint:'/api/v1/scheduler/binding',execution_settings:{provider:'fixture',threads},async_models:[{id:'fixture-image',task_type:'image.generate'}],...(restart?{preemption_modes:['restart']}:{})}};\nelse if(req.url==='/api/v1/scheduler/binding'){binding=b;value={configured:true};}\nelse if(req.url==='/mode'){restart=!!b.restart;threads=b.threads;value={ok:true};}\nelse if(req.url==='/stats')value={configured:!!binding,revision:binding?.binding_revision};\nres.writeHead(200,{'content-type':'application/json'});res.end(JSON.stringify(value));}).listen(5000);")

try:
 docker('network','create','--internal',network)
 pg=launch('pg',['--env-file',str(file('pg.env',f'POSTGRES_DB=test\nPOSTGRES_PASSWORD={password}\n')),os.environ.get('SCHEDULER_TEST_PG_IMAGE','postgres:18')])
 until(lambda:'accepting connections'in docker('exec',pg,'pg_isready','-U','postgres'),'database readiness')
 peer=launch('fixture',['--network-alias','fixture','--network-alias','image-studio-backend','-v',f'{fixture}:/fixture.mjs:ro','--entrypoint','node',os.environ.get('SCHEDULER_TEST_NODE_IMAGE','node:24-bookworm-slim'),'/fixture.mjs'])
 peerurl=address(peer,5000)
 config=file('config/controller.json',{'principals':[{'token':admin,'role':'admin','app_id':'test'},{'token':worker,'role':'worker','node_id':'local','app_id':'test-worker'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':1000,'memory_mib':8192}}],'executors':[],'local_agent':{'node_id':'local','automatic_capacity':False,'executors':[]}})
 file('config/local-agent.json',{'controller':'http://v-scheduler:38120/api/v1','node_id':'local','token':worker,'journal':'/config/journal.json','executors':[]})
 approval={'app_id':'com.ictrek.image-studio','architecture':'x86_64','profile':'amd-cuda','hardware':[],'execution_settings':{'provider':'fixture','threads':1},'model_id':'fixture-image','task_type':'image.generate','resources':{'cpu_millicores':1000,'memory_mib':512},'evidence':{'id':'protocol-fixture-not-production-calibration','peak_memory_mib':256,'confirmed_stopped':True},'max_input_bytes':8192,'model_limits':{'width':512},'model_parameters':{'backend':'fixture'}}
 file('config/app-approvals.json',{'protocol_version':1,'credential_epoch':1,'approvals':[approval]})
 data=run/'data';data.mkdir(mode=0o700);context=file('context/access-token','fixture-context');pgip=json.loads(docker('inspect',pg))[0]['NetworkSettings']['Networks'][network]['IPAddress']
 env=file('controller.env',f'SCHEDULER_DATABASE_URL=postgres://postgres:{password}@{pgip}:5432/test\nSCHEDULER_EMBEDDED_AGENT=false\nVOS_APP_VERSION=protocol-fixture\nSCHEDULER_VOS_URL=http://fixture:5000\n')
 ctl=launch('controller',['--user',str(os.getuid())+':'+str(os.getgid()),'--network-alias','v-scheduler','--env-file',str(env),'-v',f'{config.parent}:/config','-v',f'{context.parent}:/var/lib/vos-context:ro','-v',f'{data}:/gateway-data','-v',f'{build}/target:/target:ro','--entrypoint','/target/debug/v-scheduler',os.environ.get('SCHEDULER_TEST_SDK','v-scheduler-p0-test-sdk:20261019'),'controller','--config','/config/controller.json','--listen','0.0.0.0:38120'])
 api=address(ctl,38120);until(lambda:request(api+'/health')[0]==200,'controller ready')
 def heartbeat():
  while not stop.is_set():
   try:request(api+'/api/v1/workers/local/heartbeat',{'session_id':'test','report':{'observed_at':datetime.datetime.now(datetime.timezone.utc).isoformat(),'cpu_logical_count':4,'cpu_used_percent':0,'memory_total_mib':16384,'memory_available_mib':12288,'gpus':[]}},worker)
   except OSError:pass
   stop.wait(1)
 threading.Thread(target=heartbeat,daemon=True).start()
 until(lambda:request(peerurl+'/stats')[1]['configured'],'legacy binding')
 path=run/'config/app-bindings/image-studio.json';old=json.loads(path.read_text());oldid=old['binding']['models']['fixture-image'];assert old['executors'][0]['retry_mode']=='none'
 managed=request(api+'/api/v1/management',token=admin)[1]['data'];row=next(x for x in managed['executors']if x['id']==oldid)
 assert request(api+'/api/v1/executors/'+oldid,{'version':row['version'],'enabled':False,'definition':row['definition']},admin)[0]==200
 request(peerurl+'/mode',{'restart':False,'threads':2})
 until(lambda:json.loads(path.read_text())['binding']['models']=={},'transient incompatible discovery did not retire')
 request(peerurl+'/mode',{'restart':True,'threads':1});time.sleep(12)
 assert sql("SELECT count(*) FROM vrs_executor_settings WHERE state='enabled'")=='0','restart upgrade bypassed disabled legacy identity after empty discovery'
 print('PASS restart policy preserves explicit administrator disable after empty discovery',flush=True)
 managed=request(api+'/api/v1/management',token=admin)[1]['data'];row=next(x for x in managed['executors']if x['id']==oldid)
 assert request(api+'/api/v1/executors/'+oldid,{'version':row['version'],'enabled':True,'definition':row['definition']},admin)[0]==200
 until(lambda:(s if (s:=json.loads(path.read_text()))['binding']['models'].get('fixture-image') not in (None,oldid) else None),'new restart identity not delivered')
 new=json.loads(path.read_text());definition=next(x for x in new['executors']if x['id']==new['binding']['models']['fixture-image'])
 assert definition['retry_mode']=='idempotent'and definition['max_attempts']==3 and definition['resources']==old['executors'][0]['resources']
 assert new['binding']['token']==old['binding']['token'] and new['binding']['binding_revision']>old['binding']['binding_revision']
 assert sql("SELECT state FROM vrs_executor_settings WHERE id='"+oldid+"'")=='disabled'
 print('PASS legacy policy migrates with fresh identity, unchanged budget and credentials after explicit restore',flush=True)
 print('IMAGE_RESTART_POLICY_MIGRATION_ACCEPTANCE_PASSED',flush=True)
finally:
 stop.set()
 for name in reversed(names):
  try:docker('rm','-f',name)
  except subprocess.CalledProcessError:pass
 try:docker('network','rm',network)
 except subprocess.CalledProcessError:pass
