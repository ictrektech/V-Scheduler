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
 for name,src,spec,excluded in [('controller','backend/src/controller.rs','api/openapi-v1.json',{'/setup','/openapi.json','/health'}),('weights','backend/src/modelhub/mod.rs','api/model-management.openapi.json',{'/health','/openapi.json','/docs'})]:
  actual={(m,p) for m,p in routes((root/src).read_text()) if p not in excluded};declared={(m,p) for m,p in operations(load(spec)) if p not in excluded}
  assert actual==declared,(name,'route mismatch',actual-declared,declared-actual)
 for spec in ['router-openapi-en.json','openapi-v1.json','model-management.openapi.json']:
  for method,path in operations(load('api/'+spec)):
   assert f'| {method.upper()} | `{path}` |' in (root/'docs/api-reference.md').read_text(),('reference index missing',method,path)
 print(f'API documentation: {len(gateway)} gateway operations, controller and weights routes, and bilingual request schemas agree.')
if __name__=='__main__':check()
