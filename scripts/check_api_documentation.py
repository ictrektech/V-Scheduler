import pathlib,re,json
root=pathlib.Path(__file__).resolve().parents[1]
def routes(source):
 out=set()
 for m in re.finditer(r'\.route\(\s*"([^"]+)"\s*,',source):
  path=m[1];expr=source[m.end():];depth=0;quoted=False;escape=False;i=0
  while i<len(expr):
   c=expr[i]
   if quoted:
    if escape:escape=False
    elif c=='\\':escape=True
    elif c=='"':quoted=False
    i+=1;continue
   if c=='"':quoted=True;i+=1;continue
   if depth==0:
    method=re.match(r'(get|post|put|delete|patch)\s*\(',expr[i:])
    if method and (i==0 or not(expr[i-1].isalnum() or expr[i-1]=='_')):out.add((method[1],path))
   if c=='(':depth+=1
   elif c==')':
    if depth==0:break
    depth-=1
   i+=1
 return out

METHODS={'get','post','put','patch','delete'}
def operations(spec):
 return {(m,p.replace('...}', '}')) for p,ops in spec['paths'].items() for m in ops if m in METHODS}
def load(name):return json.loads((root/name).read_text())
def structural(value):
 if isinstance(value,dict):return {k:structural(v) for k,v in value.items() if k not in {'description','summary','example','examples'}}
 if isinstance(value,list):return [structural(v) for v in value]
 return value
def check():
 gateway={(r['method'].lower(),r['path'].replace('...}', '}')) for r in load('api/router-routes.json')}
 for lang in ['en','zh-CN']:
  spec=load(f'api/router-openapi-{lang}.json');declared=operations(spec)
  assert gateway==declared,(lang,'gateway route mismatch',gateway-declared,declared-gateway)
  docs={(e['method'].lower(),e['path'].replace('...}', '}')) for g in load(f'api/router-documentation-{lang}.json')['groups'] for e in g['endpoints']}
  assert gateway==docs,(lang,'guide route mismatch',gateway-docs,docs-gateway)
 en=load('api/router-openapi-en.json');zh=load('api/router-openapi-zh-CN.json')
 for path,ops in en['paths'].items():
  for method,op in ops.items():
   if method in METHODS:assert structural(op.get('requestBody'))==structural(zh['paths'][path][method].get('requestBody')),('bilingual request mismatch',method,path)
 for path,ops in en['paths'].items():
  for method,op in ops.items():
   if method in METHODS:assert structural(op.get('responses'))==structural(zh['paths'][path][method].get('responses')),('bilingual response mismatch',method,path)
 assert structural(en['components'].get('schemas'))==structural(zh['components'].get('schemas')), 'bilingual component schema mismatch'
 for path,method in [('/v1/models','get'),('/v1/chat/completions','post'),('/v1/messages','post'),('/v1/responses','post'),('/v1/completions','post'),('/v1/embeddings','post'),('/v1/rerank','post'),('/v1/decisions','post'),('/v1/systemone','post')]:
  operation=en['paths'][path][method]
  assert operation['responses']['200']['content']['application/json']['schema'],('missing native protocol response',method,path)
  if method=='post':
   assert operation['requestBody']['required'] and operation['requestBody']['content']['application/json']['schema'].get('$ref'),('missing typed inference request',path)
 assert 'text/event-stream' in en['paths']['/v1/messages']['post']['responses']['200']['content']
 for name in ['router-openapi-en.json','router-openapi-zh-CN.json','openapi-v1.json','model-management.openapi.json']:
  document=load('api/'+name)
  def references(value):
   if isinstance(value,dict):
    if value.get('$ref','').startswith('#/'):
     target=document
     for part in value['$ref'][2:].split('/'):target=target[part.replace('~1','/').replace('~0','~')]
    for child in value.values():references(child)
   elif isinstance(value,list):
    for child in value:references(child)
  references(document)
 management=next(g for g in load('api/router-documentation-en.json')['groups'] if g['id']=='manage')['endpoints']
 for endpoint in management:
  method,path=endpoint['method'].lower(),endpoint['path'];op=en['paths'][path][method]
  success=[(code,response) for code,response in op['responses'].items() if code.startswith('2')]
  assert success,('missing success response',method,path)
  for code,response in success:
   if code=='204':assert not response.get('content'),('204 must not declare a body',method,path)
   else:assert response.get('content',{}).get('application/json',{}).get('schema'),('missing management response schema',method,path)
 for path in ['/api/v1/upstreams/{id}','/api/v1/registrations/{id}']:
  assert '204' in en['paths'][path]['delete']['responses'],('deletion must document 204',path)
 assert '201' in en['paths']['/api/v1/upstreams']['post']['responses'], 'upstream creation must document 201'
 for name,src,spec,excluded in [('controller','backend/src/controller.rs','api/openapi-v1.json',{'/setup','/openapi.json','/health'}),('weights','backend/src/modelhub/mod.rs','api/model-management.openapi.json',{'/health','/openapi.json','/docs'})]:
  actual={(m,p) for m,p in routes((root/src).read_text()) if p not in excluded};declared={(m,p) for m,p in operations(load(spec)) if p not in excluded}
  assert actual==declared,(name,'route mismatch',actual-declared,declared-actual)
 for name in ['openapi-v1.json','model-management.openapi.json']:
  document=load('api/'+name)
  for method,path in operations(document):
   op=document['paths'][path][method]
   if path=='/node-sources/modelinfer/{id}/control':
    assert '501' in op['responses'] and not any(code.startswith('2') for code in op['responses']), 'reserved control must not advertise execution'
   for code,response in op.get('responses',{}).items():
    if not code.startswith('2') or code=='204':continue
    schema=response.get('content',{}).get('application/json',{}).get('schema')
    assert schema and schema!={'$ref':'#/components/schemas/Envelope'},('missing operation-specific response',name,method,path)
 for spec in ['router-openapi-en.json','openapi-v1.json','model-management.openapi.json']:
  for method,path in operations(load('api/'+spec)):
   assert f'| {method.upper()} | `{path}` |' in (root/'docs/api-reference.md').read_text(),('reference index missing',method,path)
 print(f'API documentation: {len(gateway)} gateway operations, controller and weights routes, and bilingual request schemas agree.')
if __name__=='__main__':check()
