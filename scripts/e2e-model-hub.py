#!/usr/bin/env python3
"""Isolated API acceptance. Run on a build host; no production apps or model downloads.

python3 scripts/e2e-model-hub.py --build-root ~/dev/build/V-Scheduler/model-hub-merge
Uses the Rust debug binary and SDK image produced by verification; tiny local HTTP fixtures only.
"""
import argparse, hashlib, io, json, os, pathlib, shutil, socket, subprocess, tarfile, tempfile, threading, time, urllib.parse, urllib.request, urllib.error, uuid, zipfile
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

parser=argparse.ArgumentParser();parser.add_argument('--build-root',required=True);parser.add_argument('--runtime-image');parser.add_argument('--binary');args=parser.parse_args()
build=pathlib.Path(args.build_root).expanduser().resolve();data=build/('acceptance-'+uuid.uuid4().hex);data.mkdir(parents=True)
files={'config.json':b'{"architecture":"fixture"}', 'weights/Q4.gguf':b'Q4weight'*100,'weights/Q8.gguf':b'Q8weight'*150,'weights/slow.gguf':b'slow-weights'*65536}
seen=[];version=[1000];fail=[False];slow=[False];running=set()
class Fixture(BaseHTTPRequestHandler):
 def log_message(self,*_):pass
 def reply(self,value,status=200):
  body=json.dumps(value).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body)
 def do_GET(self):
  url=urllib.parse.urlsplit(self.path);path=url.path;query=urllib.parse.parse_qs(url.query);seen.append((path,self.headers.get('Range')))
  if path.endswith('/revisions'):return self.reply({'Data':{'RevisionMap':{'Branches':[{'Revision':'master','CreatedAt':version[0]}]}}})
  if path.startswith('/api/models/'):
   return self.reply({'sha':'fixture-'+str(version[0]),'siblings':[{'rfilename':k,'size':len(v),'lfs':{'size':len(v),'sha256':hashlib.sha256(v).hexdigest()}} for k,v in files.items()]})
  if path.endswith('/repo/files'):return self.reply({'Data':{'Files':[{'Path':k,'Size':len(v),'Sha256':hashlib.sha256(v).hexdigest(),'Type':'blob'} for k,v in files.items()]}})
  if path.endswith('/repo') or '/resolve/' in path:
   name=query.get('FilePath',[''])[0] if path.endswith('/repo') else path.split('/resolve/',1)[1].split('/',1)[1]
   if fail[0]:return self.reply({'error':'fixture failure'},503)
   body=files[name];offset=int((self.headers.get('Range') or 'bytes=0-').split('=')[1].split('-')[0]);self.send_response(206 if offset else 200);self.send_header('Content-Length',str(len(body)-offset))
   if offset:self.send_header('Content-Range',f'bytes {offset}-{len(body)-1}/{len(body)}')
   self.end_headers()
   try:
    for start in range(offset,len(body),4096):
     self.wfile.write(body[start:start+4096]);self.wfile.flush()
     if 'slow' in name and slow[0]:time.sleep(.03)
   except (BrokenPipeError,ConnectionResetError):pass
   return
  if path.endswith('/api/version'):return self.reply({'version':'fixture-ollama'})
  if path.endswith('/api/tags'):return self.reply({'models':[{'name':'qwen:latest','size':4096,'digest':'sha256:fixture','capabilities':['completion']},{'name':'embed:latest','capabilities':['embedding']},{'name':'rank:latest','capabilities':['rerank']}]})
  if path.endswith('/api/ps'):return self.reply({'models':[{'name':m,'size_vram':2048,'context_length':8192} for m in running]})
  if path.endswith('/v1/models'):return self.reply({'data':[{'id':'qwen:latest'},{'id':'embed:latest'},{'id':'rank:latest'}]})
  if path.endswith('/metrics'):return self.reply({'active_slots':1,'total_slots':8,'model_metrics':{'qwen:latest':{'active_slots':1,'phase':'decode','tokens_per_second':12.5}}})
  return self.reply({'detail':'not found'},404)
 def do_POST(self):
  value=json.loads(self.rfile.read(int(self.headers.get('Content-Length','0'))) or b'{}');path=urllib.parse.urlsplit(self.path).path
  if path.endswith('/api/pull'):
   body=b'{"status":"pulling","digest":"fixture","completed":10,"total":10}\n{"status":"success"}\n';self.send_response(200);self.send_header('Content-Length',str(len(body)));self.end_headers();self.wfile.write(body);return
  if path.endswith(('/api/generate','/api/embed','/v1/rerank')):
   if value.get('keep_alive')==0:running.discard(value['model'])
   else:running.add(value['model'])
   return self.reply({'done':True})
  return self.reply({'detail':'not found'},404)
 def do_DELETE(self):return self.reply({'deleted':True})
fixture=ThreadingHTTPServer(('127.0.0.1',0),Fixture);threading.Thread(target=fixture.serve_forever,daemon=True).start();upstream=f'http://127.0.0.1:{fixture.server_port}'
listener=socket.socket();listener.bind(('127.0.0.1',0));port=listener.getsockname()[1];listener.close();base=f'http://127.0.0.1:{port}'
old=data/'export/hf/legacy/model/snapshots/old';old.mkdir(parents=True);(old/'weights.gguf').write_bytes(b'legacy weights');(old.parent.parent/'current').symlink_to('snapshots/old');inode=(old/'weights.gguf').stat().st_ino
registry={'hf://legacy/model':{'model_id':'hf://legacy/model','name':'Legacy','source':'hf','status':'ready','size':14,'path':'/data/export/hf/legacy/model','version':'old','files':['weights.gguf'],'created_at':'2026-01-01T00:00:00Z','sha256':None,'legacy_field':'keep'}}
(data/'registry').mkdir();(data/'registry/models.json').write_text(json.dumps(registry));(data/'registry/aliases.json').write_text(json.dumps({'legacy':'hf://legacy/model'}));(data/'registry/containers.json').write_text(json.dumps({'legacy-container':{'container_id':'legacy-container','model_id':'hf://legacy/model','ref_count':1,'mounted_at':'2026-01-01T00:00:00Z'}}));(data/'shared/tasks').mkdir(parents=True);(data/'shared/tasks/legacy-task.json').write_text(json.dumps({'task_id':'legacy-task','model_id':'hf://legacy/model','phase':'READY','progress':1,'speed':None,'eta':None,'error_msg':None,'files':['weights.gguf'],'append':False}))
name='vs-modelhub-accept-'+uuid.uuid4().hex[:8];log=open(data/'runtime.log','w')
command=['docker','run','--rm','--name',name,'--network','host','-v',f'{data}:/data','-v',f'{build}/target:/target:ro','-e','MODELS_BASE_DIR=/data','-e',f'MODELSCOPE_ENDPOINT={upstream}','-e','HF_MIRROR_ENABLED=true','-e',f'HF_ENDPOINT={upstream}']
for role in ['QA','EMBEDDING','RERANK']:command+=['-e',f'OLLAMA_{role}_API_URL={upstream}','-e',f'OLLAMA_{role}_GATEWAY_URL={upstream}']
command+=['-e','VOS_APP_VERSION=acceptance-fixture','-e','MODEL_HUB_PROFILE=amd','-e','V_SCHEDULER_IMAGE=fixture/scheduler:amd_fixture','-e','V_ROUTER_BACKEND_IMAGE=fixture/router:amd_fixture','-e','OLLAMA_SERVER_IMAGE=fixture/ollama:amd_fixture']
command+=([args.runtime_image] if args.runtime_image else ['--entrypoint','/target/debug/v-scheduler','v-scheduler-modelhub-sdk:20261016'])+['model-hub','--listen',f'127.0.0.1:{port}']
environment=None
if args.binary:
 environment={**os.environ,'MODELS_BASE_DIR':str(data),'MODELSCOPE_ENDPOINT':upstream,'HF_MIRROR_ENABLED':'true','HF_ENDPOINT':upstream,'VOS_APP_VERSION':'acceptance-fixture','MODEL_HUB_PROFILE':'amd','V_SCHEDULER_IMAGE':'fixture/scheduler:amd_fixture','V_ROUTER_BACKEND_IMAGE':'fixture/router:amd_fixture','OLLAMA_SERVER_IMAGE':'fixture/ollama:amd_fixture'}
 for role in ['QA','EMBEDDING','RERANK']:
  for kind in ['API','GATEWAY']:environment[f'OLLAMA_{role}_{kind}_URL']=upstream
 command=[args.binary,'model-hub','--listen',f'127.0.0.1:{port}']
process=subprocess.Popen(command,env=environment,stdout=log,stderr=log)
checks=[]
def check(condition,label):
 if not condition:raise AssertionError(label)
 checks.append(label)
def request(path,body=None,method=None,status=200,headers=None):
 payload=None if body is None else json.dumps(body).encode();req=urllib.request.Request(base+path,data=payload,method=method or ('GET' if body is None else 'POST'),headers=headers or {'Content-Type':'application/json'})
 try:
  with urllib.request.urlopen(req,timeout=30) as response:code=response.status;data=json.load(response)
 except urllib.error.HTTPError as error:code=error.code;data=json.load(error)
 assert code==status,(path,code,data)
 return data
def task_wait(task_id,phase='READY'):
 deadline=time.monotonic()+20
 while time.monotonic()<deadline:
  value=request('/api/v1/tasks/detail/'+task_id)
  if value['phase'] in ['READY','FAILED']:
   check(value['phase']==phase,f'task reaches {phase}');return value
  time.sleep(.05)
 raise AssertionError('task timeout')
def pull(source='ms',repo='org/repo',selected=None,**extra):return request('/api/v1/models/pull',{'source':source,'model_id':f'{source}://{repo}',**({'files':selected} if selected is not None else {}),**extra})['task_id']
try:
 for _ in range(150):
  try:request('/health');break
  except Exception:
   if process.poll() is not None:raise RuntimeError((data/'runtime.log').read_text())
   time.sleep(.1)
 else:raise RuntimeError('service startup timeout')
 check(request('/api/v1/models')[0]['name']=='Legacy','adopts old model registry')
 check(request('/api/v1/aliases')[0]['alias']=='legacy','adopts aliases')
 check(request('/api/v1/containers')[0]['ref_count']==1,'adopts container references')
 check(request('/api/v1/tasks')[0]['task_id']=='legacy-task','adopts task history')
 check((old/'weights.gguf').stat().st_ino==inode,'weights retain inode and bytes on adoption')
 request('/api/v1/models?model_id=legacy',method='DELETE',status=409)
 metadata=request('/api/v1/models/repository-files?source=ms&model_id=org/repo');check(len(metadata['files'])==4,'remote metadata lists exact repository files')
 check(not any(p.endswith('/repo') for p,_ in seen),'metadata listing does not download weights')
 task_wait(pull(selected=['config.json','weights/Q4.gguf']))
 local=request('/api/v1/models/files?model_id=ms%3A%2F%2Forg%2Frepo');check({v['path'] for v in local['files']}=={'config.json','weights/Q4.gguf'},'only selected files are downloaded')
 metadata=request('/api/v1/models/repository-files?source=ms&model_id=org/repo');check(sum(f['downloaded'] for f in metadata['files'])==2,'remote listing recognizes downloaded paths with bare repository ID')
 current=data/'export/ms/org/repo/current';before=(current/'weights/Q4.gguf').stat().st_ino
 task_wait(pull(selected=['weights/Q8.gguf'],append=True));check((current/'weights/Q4.gguf').stat().st_ino==before,'append preserves existing weight inode');check((current/'weights/Q8.gguf').read_bytes()==files['weights/Q8.gguf'],'append publishes additional quantization')
 check(request('/api/v1/models/check-update?model_id=ms%3A%2F%2Forg%2Frepo',{})['has_update'] is False,'unchanged ModelScope master timestamp has no update');version[0]+=1;check(request('/api/v1/models/check-update?model_id=ms%3A%2F%2Forg%2Frepo',{})['has_update'],'ModelScope detects master timestamp change')
 link=os.readlink(current);fail[0]=True;failed=pull(selected=['weights/Q4.gguf'],force=True);task_wait(failed,'FAILED');check(os.readlink(current)==link,'failed update retains old current snapshot');check(next(m for m in request('/api/v1/models') if m['model_id']=='ms://org/repo')['status']=='ready','failed update restores READY registry state')
 fail[0]=False;task_wait(request('/api/v1/tasks/'+failed+'/retry',{})['task_id'])
 for path in ['../escape','/etc/passwd','weights/*.gguf','weights\\escape']:
  request('/api/v1/models/pull',{'source':'ms','model_id':'org/bad','files':[path]},status=400)
 check(not (data/'escape').exists(),'selected paths cannot escape shared storage')
 task_wait(pull(source='hf',repo='org/hf',selected=['config.json','weights/Q4.gguf']));check((data/'export/hf/org/hf/current/weights/Q4.gguf').read_bytes()==files['weights/Q4.gguf'],'HuggingFace native HTTP download and checksums')
 task_wait(pull(repo='org/append-new',selected=['config.json'],append=True));check(True,'legacy selected-file append request works for first install')
 slow[0]=True;task_id=pull(repo='org/slow',selected=['weights/slow.gguf']);time.sleep(.4);request('/api/v1/tasks/'+task_id+'/cancel',{});task_wait(task_id,'FAILED');slow[0]=False;task_wait(request('/api/v1/tasks/'+task_id+'/retry',{})['task_id']);check(any(header and header!='bytes=0-' for _,header in seen),'retry resumes partial data with HTTP Range')
 task_wait(pull(source='ollama',repo='qwen:latest'));check(True,'Ollama streaming pull task reaches READY')
 runtime=request('/api/v1/ollama/runtime');check(len(runtime['runtimes'])==3,'three builtin runtime cards are inspectable');check(runtime['runtimes'][0]['models'][0]['tokens_per_second']==12.5,'runtime metrics retain model token speed')
 action=request('/api/v1/ollama/runtime/start',{'runtime':'qa','model':'qwen:latest','confirm_timeout_seconds':1});check(action['confirmed'],'model start is confirmed by actual running state');action=request('/api/v1/ollama/runtime/stop',{'runtime':'qa','model':'qwen:latest','confirm_timeout_seconds':1});check(action['confirmed'],'model stop is confirmed')
 request('/api/v1/inference-services/register',{'service_id':'external','name':'External Ollama','kind':'ollama','base_url':upstream,'gateway_url':upstream,'role':'generate'});check(any(s['service_id']=='external' for s in request('/api/v1/inference-services')),'external service registration and listing');check(request('/api/v1/inference-services/external')['healthy'],'external service health and cards')
 boundary='fixture'+uuid.uuid4().hex
 archive=io.BytesIO()
 with zipfile.ZipFile(archive,'w') as z:z.writestr('wrapper/model.safetensors',b'imported weight');z.writestr('wrapper/config.json',b'{}')
 body=(f'--{boundary}\r\nContent-Disposition: form-data; name="source"\r\n\r\nhf\r\n--{boundary}\r\nContent-Disposition: form-data; name="model_id"\r\n\r\nlocal/imported\r\n--{boundary}\r\nContent-Disposition: form-data; name="archive"; filename="fixture.zip"\r\nContent-Type: application/zip\r\n\r\n'.encode()+archive.getvalue()+f'\r\n--{boundary}--\r\n'.encode())
 req=urllib.request.Request(base+'/api/v1/models/import-archive',data=body,headers={'Content-Type':f'multipart/form-data; boundary={boundary}'})
 with urllib.request.urlopen(req,timeout=30) as response:imported=json.load(response)
 check(imported['status']=='ready' and (data/'export/hf/local/imported/current/model.safetensors').read_bytes()==b'imported weight','archive import strips wrapper and publishes valid weights')
 if args.runtime_image:
  for extension,mode in [('tar','w'),('tar.gz','w:gz'),('tar.bz2','w:bz2'),('tar.xz','w:xz')]:
   archive=io.BytesIO();content=b'{}'
   with tarfile.open(fileobj=archive,mode=mode) as output:
    member=tarfile.TarInfo('wrapper/config.json');member.size=len(content);output.addfile(member,io.BytesIO(content))
   boundary='archive'+uuid.uuid4().hex;model='flat-'+extension.replace('.','-')
   body=(f'--{boundary}\r\nContent-Disposition: form-data; name="source"\r\n\r\nhf\r\n--{boundary}\r\nContent-Disposition: form-data; name="model_id"\r\n\r\n{model}\r\n--{boundary}\r\nContent-Disposition: form-data; name="archive"; filename="fixture.{extension}"\r\n\r\n'.encode()+archive.getvalue()+f'\r\n--{boundary}--\r\n'.encode())
   req=urllib.request.Request(base+'/api/v1/models/import-archive',data=body,headers={'Content-Type':f'multipart/form-data; boundary={boundary}'})
   with urllib.request.urlopen(req,timeout=30) as response:imported=json.load(response)
   check(imported['status']=='ready' and (data/f'export/hf/{model}/current/config.json').read_bytes()==content,f'{extension} supports flat IDs and config-only archives')
  archive=io.BytesIO()
  with zipfile.ZipFile(archive,'w') as output:output.writestr('model.gguf',b'x'*(3*1024*1024))
  boundary='large'+uuid.uuid4().hex
  body=(f'--{boundary}\r\nContent-Disposition: form-data; name="source"\r\n\r\nms\r\n--{boundary}\r\nContent-Disposition: form-data; name="model_id"\r\n\r\nlarge-upload\r\n--{boundary}\r\nContent-Disposition: form-data; name="archive"; filename="large.zip"\r\n\r\n'.encode()+archive.getvalue()+f'\r\n--{boundary}--\r\n'.encode())
  req=urllib.request.Request(base+'/api/v1/models/import-archive',data=body,headers={'Content-Type':f'multipart/form-data; boundary={boundary}'})
  with urllib.request.urlopen(req,timeout=30) as response:imported=json.load(response)
  check(imported['status']=='ready' and (data/'export/ms/large-upload/current/model.gguf').stat().st_size==3*1024*1024,'archive upload streams past default 2 MiB body limit')
 request('/api/v1/aliases',{'alias':'new-model','target_model_id':'hf://local/imported'});request('/api/v1/containers/bind',{'container_id':'consumer','model_id':'hf://local/imported','ref_count':2});request('/api/v1/models?model_id=new-model',method='DELETE',status=409);request('/api/v1/containers/unbind',{'container_id':'consumer'},method='DELETE');request('/api/v1/models?model_id=new-model',method='DELETE');check(not any(a['alias']=='new-model' for a in request('/api/v1/aliases')),'safe deletion respects references and removes target aliases')
 spec=request('/openapi.json');check('/api/v1/models/pull' in spec['paths'] and '/api/v1/ollama/chat' not in spec['paths'],'OpenAPI lists management API and avoids duplicate chat')
 check(next(e for e in request('/api/v1/integration')['endpoints'] if e['name']=='Model management')['host_port']=='36005','integration docs expose management host mapping')
 installed=request('/api/v1/version');check(installed['version']=='acceptance-fixture' and installed['profile']=='amd' and len(installed['images'])==6 and all(i['tag']=='amd_fixture' and '/' not in i['image'] for i in installed['images']),'About API reports actual app/profile and every image tag')
 check((old/'weights.gguf').stat().st_ino==inode and (old/'weights.gguf').read_bytes()==b'legacy weights','legacy model stays unchanged after full lifecycle tests')
 result={'checks':len(checks),'passed':checks,'fixture_data':str(data)};(build/'model-hub-acceptance.json').write_text(json.dumps(result,indent=2));print(json.dumps({'checks':len(checks),'status':'PASS','evidence':str(build/'model-hub-acceptance.json')}))
finally:
 if args.binary:process.terminate()
 else:subprocess.run(['docker','stop',name],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
 process.wait(timeout=15);log.close();fixture.shutdown()
