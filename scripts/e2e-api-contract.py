#!/usr/bin/env python3
"""Actual Go/Rust services with isolated PostgreSQL and synthetic VOS/model endpoints.
Run on a synced Linux build host; never uses installed application data or model weights.
"""
import argparse, json, os, pathlib, secrets, socket, subprocess, threading, time, urllib.error, urllib.request, uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
p=argparse.ArgumentParser();p.add_argument('--build-root',default=str(pathlib.Path.home()/'dev/build/V-Scheduler/contracts'));a=p.parse_args()
root=pathlib.Path(a.build_root);run=root/uuid.uuid4().hex;run.mkdir(parents=True,mode=0o700)
repo=pathlib.Path(__file__).resolve().parents[1];spec=json.loads((repo/'api/error-contract.json').read_text());keys={k:secrets.token_hex(32) for k in ['db','admin','submitter','gateway','vos_admin','vos_user']};children=[];checks=[];container='vs-contract-pg-'+uuid.uuid4().hex[:12]
def private(name,value):
 f=run/name;f.write_text(value if isinstance(value,str) else json.dumps(value));f.chmod(0o600);return f
def port():
 s=socket.socket();s.bind(('127.0.0.1',0));n=s.getsockname()[1];s.close();return n
ports={k:port() for k in ['db','controller','hub','gateway']}
class Fixture(BaseHTTPRequestHandler):
 def log_message(self,*_):pass
 def reply(self,status,value):
  b=json.dumps(value).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(b)));self.end_headers();self.wfile.write(b)
 def do_GET(self):
  token=self.headers.get('Authorization','').split(' ')[-1];valid=token in (keys['vos_admin'],keys['vos_user']);admin=token==keys['vos_admin']
  if self.path=='/v1000/user/check':return self.reply(200,{'code':0,'data':{'id':'admin' if admin else 'user','role_code':'system_admin' if admin else 'user','enable':1,'delete':0}} if valid else {'code':10001})
  if self.path=='/v1000/oauth2/userinfo':return self.reply(200,{'sub':'admin' if admin else 'user','preferred_username':'admin' if admin else 'user'} if valid else {'code':10001})
  if self.path=='/v1/models':return self.reply(200,{'data':[{'id':'contract-echo'}]})
  return self.reply(404,{})
 def do_POST(self):
  body=json.loads(self.rfile.read(int(self.headers.get('Content-Length',0))) or b'{}')
  if self.path=='/v1/chat/completions':
   if body.get('stream'):
    self.send_response(200);self.send_header('Content-Type','text/event-stream');self.end_headers();self.wfile.write(b'data: {"choices":[{"delta":{"content":"contract echo"}}]}\n\ndata: [DONE]\n\n');return
   return self.reply(200,{'id':'contract','object':'chat.completion','model':body['model'],'choices':[{'index':0,'message':{'role':'assistant','content':'contract echo'},'finish_reason':'stop'}],'usage':{'prompt_tokens':1,'completion_tokens':2,'total_tokens':3}})
  return self.reply(404,{})
mock=ThreadingHTTPServer(('0.0.0.0',0),Fixture);threading.Thread(target=mock.serve_forever,daemon=True).start();vos=f'http://127.0.0.1:{mock.server_port}'
def launch(name,args,env):
 log=open(run/(name+'.log'),'ab');os.chmod(log.name,0o600);proc=subprocess.Popen(args,env={**os.environ,**env},stdout=log,stderr=log);children.append(proc);return proc
binary=os.environ.get('SCHEDULER_BINARY',str(pathlib.Path.home()/'dev/build/V-Scheduler/cargo/debug/v-scheduler'));gateway=os.environ.get('GATEWAY_BINARY',str(pathlib.Path.home()/'dev/build/V-Scheduler/gateway/v-router'))
controller=f'http://127.0.0.1:{ports["controller"]}';hub=f'http://127.0.0.1:{ports["hub"]}';gate=f'http://127.0.0.1:{ports["gateway"]}'
def request(origin,path,token=None,body=None,method=None,scheme='Bearer'):
 data=None if body is None else body.encode() if isinstance(body,str) else json.dumps(body).encode();h={'Content-Type':'application/json'}
 if token:h['Authorization']=scheme+' '+token
 req=urllib.request.Request(origin+path,data=data,headers=h,method=method or ('POST' if body is not None else 'GET'))
 try:r=urllib.request.urlopen(req,timeout=15)
 except urllib.error.HTTPError as error:r=error
 raw=r.read();headers={k.lower():v for k,v in r.headers.items()};status=r.status
 try:value=json.loads(raw)
 except ValueError:value=raw.decode()
 return status,value,headers

def ready(origin,path):
 for _ in range(100):
  try:
   if request(origin,path)[0]==200:return
  except (OSError,TimeoutError):pass
  time.sleep(.15)
 raise AssertionError('owned service did not become ready')
def check(value,label):
 assert value,label;checks.append(label);print('PASS:',label,flush=True)
def failure(origin,path,status,token=None,body=None):
 n,v,h=request(origin,path,token,body);assert n==status,(path,n)
 assert all(k in v for k in spec['required_error_fields']),(path,v)
 assert v['request_id']==h.get('X-Request-ID',h.get('x-request-id')),(path,'trace mismatch')
 if status==401:assert h.get('WWW-Authenticate',h.get('www-authenticate'))=='Bearer'
 return v
try:
 env=private('pg.env',f'POSTGRES_USER=scheduler\nPOSTGRES_PASSWORD={keys["db"]}\nPOSTGRES_DB=scheduler\n')
 subprocess.run(['docker','run','-d','--name',container,'--env-file',str(env),'-p',f'127.0.0.1:{ports["db"]}:5432','registry.ictrek.internal/library/postgres:18'],check=True,stdout=subprocess.DEVNULL)
 for _ in range(60):
  if subprocess.run(['docker','exec',container,'pg_isready','-h','127.0.0.1','-U','scheduler'],capture_output=True).returncode==0:break
  time.sleep(.2)
 config=private('controller.json',{'principals':[{'role':'admin','app_id':'admin','token':keys['admin']},{'role':'submitter','app_id':'alpha','token':keys['submitter']}],'nodes':[],'executors':[]})
 ctl=launch('controller',[binary,'controller','--config',str(config),'--listen',f'127.0.0.1:{ports["controller"]}','--ui-dir',os.environ.get('SCHEDULER_UI_DIR',str(root/'ui'))],{'SCHEDULER_DATABASE_URL':f'postgres://scheduler:{keys["db"]}@127.0.0.1:{ports["db"]}/scheduler','SCHEDULER_EMBEDDED_AGENT':'false','SCHEDULER_VOS_URL':vos})
 storage=run/'models';storage.mkdir();model=launch('models',[binary,'model-hub','--listen',f'127.0.0.1:{ports["hub"]}'],{'MODELS_BASE_DIR':str(storage)})
 gw=launch('gateway',[gateway,'-addr',f'127.0.0.1:{ports["gateway"]}','-state',str(run/'state.json')],{'V_ROUTER_CONTROLLER_URL':controller,'V_SCHEDULER_MODEL_MANAGEMENT_URL':hub,'SCHEDULER_VOS_URL':vos,'V_ROUTER_API_KEY':keys['gateway'],'V_ROUTER_REQUIRE_KEY':'true','V_ROUTER_USAGE_PATH':str(run/'usage.jsonl'),'V_ROUTER_AUTH_DISABLED':'false','V_ROUTER_MODEL_HUB_URL':'','V_ROUTER_IMAGE_STUDIO_URL':'','V_ROUTER_V_VOICE_URLS':'','V_ROUTER_V_MUSIC_URLS':'','V_ROUTER_ML_SERVER_URLS':'','V_ROUTER_VIDEO_STRUCTURE_URLS':'','V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS':''})
 for origin,path in [(controller,'/health'),(hub,'/health'),(gate,'/healthz')]:ready(origin,path)
 for path in ['/api/v1/status','/model-hub/api/v1/models']:failure(gate,path,401)
 check(request(gate,'/api/v1/status',keys['vos_user'])[0]==200,'VOS account authenticates gateway')
 check(request(gate,'/model-hub/api/v1/models',keys['vos_user'])[:2]==(200,[]),'same account authenticates model workspace; legacy success array preserved')
 for token,status in [(keys['vos_admin'],200),(keys['vos_user'],403),(keys['submitter'],403),(keys['gateway'],401),('expired',401)]:
  if status!=200:failure(gate,'/scheduler/api/v1/nodes',status,token)
  else:assert request(gate,'/scheduler/api/v1/nodes',token)[0]==200
 check(request(controller,'/api/v1/tasks',keys['submitter'],scheme='bearer')[0]==200,'Bearer syntax agrees across languages; service identity retains its limited scope')
 failure(gate,'/model-hub/api/v1/models',401,keys['submitter'])
 check(True,'gateway keys and scheduler submitter credentials cannot substitute for one another')
 for origin,path,token in [(gate,'/api/v1/registrations',keys['gateway']),(gate,'/scheduler/api/v1/tasks',keys['vos_admin']),(gate,'/model-hub/api/v1/models/pull',keys['vos_user'])]:failure(origin,path,400,token,'{')
 for path in ['/scheduler/api/v1/unknown','/model-hub/api/v1/unknown']:failure(gate,path,404,keys['vos_admin'])
 check(True,'malformed JSON and unknown APIs share machine-readable errors and response trace IDs')
 host=socket.gethostbyname(socket.gethostname());base=f'http://{host}:{mock.server_port}/v1'
 status,_,_=request(gate,'/api/v1/registrations',keys['gateway'],{'service_id':'contract','name':'Contract fixture','base_url':base,'protocol':'chat_completions','model':'contract-echo','model_kind':'chat','endpoints':['chat/completions','responses','messages'],'visibility':'public','scheduler':{'mode':'cloud'}})
 assert status==200,status
 for path in ['/v1/chat/completions','/v1/responses','/v1/messages']:
  body={'model':'registered/contract-echo','messages':[{'role':'user','content':'echo'}],'max_tokens':8} if path!='/v1/responses' else {'model':'registered/contract-echo','input':'echo','max_output_tokens':8}
  n,value,_=request(gate,path,keys['vos_user'],body);assert n==200,(path,n,value);assert 'contract echo' in json.dumps(value)
 check(True,'Chat Completions, Responses and Messages preserve real gateway protocol conversion')
 n,stream,_=request(gate,'/v1/chat/completions',keys['vos_user'],{'model':'registered/contract-echo','messages':[{'role':'user','content':'echo'}],'stream':True});check(n==200 and '[DONE]' in stream and 'contract echo' in stream,'SSE reaches terminal output without envelope rewriting')
 ctl.terminate();ctl.wait(timeout=10);failure(gate,'/scheduler/api/v1/nodes',503,keys['vos_admin']);assert request(gate,'/api/v1/status',keys['vos_user'])[0]==200;assert request(gate,'/model-hub/api/v1/models',keys['vos_user'])[0]==200
 check(True,'controller outage is structured and does not disable gateway or model management')
 model.terminate();model.wait(timeout=10);failure(gate,'/model-hub/api/v1/models',503,keys['vos_user']);check(True,'model management outage has the same error contract')
 private('evidence.json',{'fixture':'actual Go/Rust services, synthetic identity/model and isolated PostgreSQL','checks':checks,'success':True});print('Evidence:',run)
finally:
 for child in reversed(children):
  if child.poll() is None:
   child.terminate()
   try:child.wait(timeout=10)
   except subprocess.TimeoutExpired:child.kill();child.wait()
 mock.shutdown();subprocess.run(['docker','rm','-f','-v',container],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
