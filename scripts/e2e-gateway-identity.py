"""Isolated identity protocol fixtures; no production credentials/model inference."""
import json,os,pathlib,secrets,subprocess,time,urllib.request,urllib.error
build=pathlib.Path.home()/'dev/build/V-Scheduler/model-hub-merge';run=build/('gateway-identity-'+str(time.time_ns()));run.mkdir(mode=0o700)
network='vs-gateway-identity-'+secrets.token_hex(5);names=[];admin,password=[secrets.token_hex(32)for _ in range(2)]
def docker(*args,input=None):return subprocess.check_output(['docker',*args],input=input,text=True,stderr=subprocess.PIPE).strip()
def private(name,value):
 p=run/name;p.parent.mkdir(parents=True,exist_ok=True);p.write_text(value if isinstance(value,str)else json.dumps(value));p.chmod(0o600);return p
def launch(suffix,args):
 name=network+'-'+suffix;names.append(name);docker('run','-d','--name',name,'--network',network,*args);return name
def address(name,port):return 'http://'+json.loads(docker('inspect',name))[0]['NetworkSettings']['Networks'][network]['IPAddress']+':'+str(port)
def request(url,body=None,token=None):
 req=urllib.request.Request(url,data=None if body is None else json.dumps(body).encode(),headers={'Content-Type':'application/json',**({'Authorization':'Bearer '+token}if token else{})})
 try:
  with urllib.request.urlopen(req,timeout=15)as response:status=response.status;data=response.read()
 except urllib.error.HTTPError as e:status=e.code;data=e.read()
 try:return status,json.loads(data)
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
fixture=private('fixture.mjs',r'''
import {createServer}from'node:http';let installed=true,backend='scheduler',binding=null;
const send=(res,status,value)=>{res.writeHead(status,{'content-type':'application/json'});res.end(JSON.stringify(value));};
const handler=async(req,res)=>{let raw='';for await(const c of req)raw+=c;const body=raw?JSON.parse(raw):{};
if(req.url==='/v1000/oauth2/userinfo'){send(res,req.headers.authorization==='Bearer fixture-context'?200:401,{sub:'com.ictrek.v-scheduler',enable:1});return;}
if(req.url.startsWith('/v1000/app/detail?app_id=')){send(res,200,req.url.endsWith('com.ictrek.hybrag')&&installed?{code:0,data:{state:'installed'}}:{code:10067});return;}
if(req.url==='/api/v1/scheduler/gateway-binding'&&req.method==='GET'){send(res,200,{protocol_version:1,app_id:'com.ictrek.hybrag',capabilities:['gateway-inference-v1'],binding_endpoint:'/api/v1/scheduler/gateway-binding',backend,configured:!!binding});return;}
if(req.url==='/api/v1/scheduler/gateway-binding'&&req.method==='POST'){
if(req.headers.authorization!=='Bearer fixture-context'||body.app_id!=='com.ictrek.hybrag'||body.url!=='http://v-scheduler-gateway:8080/v1'||body.scope!=='gateway-inference-v1'){send(res,403,{});return;}
binding=body;send(res,200,{configured:true,binding_revision:binding.binding_revision});return;}
if(req.url==='/fixture-state'){if(req.method==='POST'){installed=body.installed??installed;backend=body.backend??backend;}send(res,200,{installed,backend,binding});return;}
send(res,404,{});};createServer(handler).listen(7080,'0.0.0.0');createServer(handler).listen(8080,'0.0.0.0');
''')
try:
 docker('network','create','--internal',network)
 pg=launch('pg',['--env-file',str(private('pg.env',f'POSTGRES_DB=test\nPOSTGRES_PASSWORD={password}\n')),'postgres:17-bookworm'])
 until(lambda:'accepting connections'in docker('exec',pg,'pg_isready','-U','postgres'),'database unavailable')
 fixture_name=launch('fixture',['--network-alias','fixture','--network-alias','hybrag-app','-v',f'{fixture}:/fixture.mjs:ro','--entrypoint','node','v-scheduler-ui-verify:0.0.19','/fixture.mjs']);fx=address(fixture_name,7080)
 config=private('config/controller.json',{'principals':[{'token':admin,'role':'admin','app_id':'test'}],'nodes':[{'id':'local','capacities':{'cpu_millicores':1000,'memory_mib':8192}}],'executors':[]})
 private('config/app-approvals.json',{'protocol_version':1,'credential_epoch':1,'approvals':[]})
 context=private('context/access-token','fixture-context');data=run/'data';data.mkdir(mode=0o700)
 pgip=json.loads(docker('inspect',pg))[0]['NetworkSettings']['Networks'][network]['IPAddress']
 env=private('controller.env',f'SCHEDULER_DATABASE_URL=postgres://postgres:{password}@{pgip}:5432/test\nSCHEDULER_EMBEDDED_AGENT=false\nSCHEDULER_VOS_URL=http://fixture:7080\nVOS_APP_VERSION=isolated-identity-test\n')
 controller=launch('controller',['--env-file',str(env),'-v',f'{config.parent}:/config','-v',f'{context.parent}:/var/lib/vos-context:ro','-v',f'{data}:/gateway-data','-v',f'{build}/target:/target:ro','--entrypoint','/target/debug/v-scheduler','v-scheduler-modelhub-sdk:20261016','controller','--config','/config/controller.json','--listen','0.0.0.0:38120'])
 api=address(controller,38120);until(lambda:request(api+'/health')[0]==200,'controller unavailable')
 state=until(lambda:request(fx+'/fixture-state')[1].get('binding'),'identity not delivered')
 private_state=json.loads(docker('exec',controller,'cat','/config/gateway-bindings/hybrag.json'));assert private_state['binding']==state
 gatewayenv=private('gateway.env','\n'.join(['V_ROUTER_STATE_PATH=/data/state.json','V_ROUTER_REQUIRE_KEY=true','V_ROUTER_API_KEY=',*[k+'='for k in ['V_ROUTER_MODEL_HUB_URL','V_ROUTER_IMAGE_STUDIO_URL','V_ROUTER_V_VOICE_URLS','V_ROUTER_V_MUSIC_URLS','V_ROUTER_V_SUB_URLS','V_ROUTER_ML_SERVER_URLS','V_ROUTER_VIDEO_STRUCTURE_URLS','V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS']]])+'\n')
 gateway=launch('gateway',['--env-file',str(gatewayenv),'-v',f'{data}:/data','-v',f'{build}/go:/verify:ro','--entrypoint','/verify/v-router','swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-router-base:golang-1.23-alpine-20260910-amd64']);gw=address(gateway,8080)
 until(lambda:request(gw+'/v1/models',token=state['token'])[0]==200,'inference identity rejected')
 assert request(gw+'/api/v1/status',token=state['token'])[0]==403
 assert request(gw+'/v1/images/generations',{'model':'unknown'},state['token'])[0]==403
 assert request(gw+'/v1/models',token='vs-app-'+secrets.token_hex(32))[0]==401
 digest=json.loads(docker('exec',gateway,'cat','/data/app-identities/hybrag.json'));assert 'token'not in digest
 docker('restart',controller);until(lambda:request(api+'/health')[0]==200,'controller restart failed')
 until(lambda:request(fx+'/fixture-state')[1].get('binding')==state,'restart changed credentials')
 request(fx+'/fixture-state',{'installed':False})
 until(lambda:request(gw+'/v1/models',token=state['token'])[0]==401,'uninstall did not revoke gateway identity')
 request(fx+'/fixture-state',{'installed':True})
 next_state=until(lambda:(x if(x:=request(fx+'/fixture-state')[1].get('binding'))and x['binding_revision']>state['binding_revision']else None),'reinstall did not rotate credentials')
 assert next_state['token']!=state['token']
 until(lambda:request(gw+'/v1/models',token=next_state['token'])[0]==200,'new identity rejected')
 assert request(gw+'/v1/models',token=state['token'])[0]==401
 request(fx+'/fixture-state',{'backend':'model-hub'})
 until(lambda:request(gw+'/v1/models',token=next_state['token'])[0]==401,'legacy backend did not deactivate identity')
 print('PASS fixed trusted callback, inference-only scope, restart persistence, uninstall/reinstall rotation and legacy-mode deactivation',flush=True)
 print('GATEWAY_IDENTITY_PROTOCOL_ACCEPTANCE_PASSED',flush=True)
finally:
 for name in reversed(names):subprocess.run(['docker','rm','-f',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 subprocess.run(['docker','network','rm',network],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
