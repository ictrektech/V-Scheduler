"""Opt-in live SDK acceptance. Requires reachable chat and embedding models.

V_ROUTER_TEST_BASE excludes /v1. V_ROUTER_TEST_KEY is never printed.
Dependencies: openai, anthropic. This script generates real inference requests.
Run separately from load tests when assessing automatic route distribution.
"""
import json,os,math,urllib.request,openai,anthropic
base=os.environ['V_ROUTER_TEST_BASE'].rstrip('/');key=os.environ['V_ROUTER_TEST_KEY']
def api(method,path,data=None):
 req=urllib.request.Request(base+path,data=json.dumps(data).encode() if data is not None else None,method=method,headers={'Authorization':'Bearer '+key,'Content-Type':'application/json'})
 with urllib.request.urlopen(req,timeout=120) as r:return json.load(r) if r.status!=204 else None
oa=openai.OpenAI(base_url=base+'/v1',api_key=key,timeout=180,max_retries=0,_strict_response_validation=True)
an=anthropic.Anthropic(base_url=base,api_key=key,timeout=180,max_retries=0,_strict_response_validation=True)
doc=api('GET','/api/v1/openapi.json')
for p in ['/v1/chat/completions','/v1/responses','/v1/messages','/v1/embeddings','/api/v1/registrations']:
 assert p in doc['paths']
models=[m.id for m in oa.models.list().data]
chat_model=os.environ.get('V_ROUTER_TEST_CHAT_MODEL','local/qwen3:0.6b')
embedding_model=os.environ.get('V_ROUTER_TEST_EMBED_MODEL','local/nomic-embed-text:latest')
assert chat_model in models and embedding_model in models,models
messages=[{'role':'user','content':'请只回复一个字：好'}];model=chat_model
r=oa.chat.completions.create(model=model,messages=messages,max_tokens=512);assert r.choices[0].message.content.strip();print('PASS chat nonstream',flush=True)
chunks=list(oa.chat.completions.create(model=model,messages=messages,max_tokens=512,stream=True));assert ''.join(x.choices[0].delta.content or '' for x in chunks if x.choices).strip();assert any(x.choices and x.choices[0].finish_reason for x in chunks);print('PASS chat SSE',flush=True)
r=oa.responses.create(model=model,input=messages,store=False,max_output_tokens=512);assert r.output_text.strip();print('PASS Responses conversion nonstream',flush=True)
with oa.responses.stream(model=model,input=messages,store=False,max_output_tokens=512) as stream:
 final=stream.get_final_response();assert final.output_text.strip();assert final.status=='completed'
print('PASS Responses conversion SSE + SDK final',flush=True)
r=an.messages.create(model=model,messages=messages,max_tokens=512);assert any(getattr(x,'text','').strip() for x in r.content);print('PASS Anthropic conversion nonstream',flush=True)
with an.messages.stream(model=model,messages=messages,max_tokens=512) as stream:
 text=''.join(stream.text_stream);final=stream.get_final_message();assert text.strip();assert final.stop_reason=='end_turn'
print('PASS Anthropic conversion SSE + SDK final',flush=True)
r=oa.embeddings.create(model=embedding_model,input=['hello world','different sentence'],encoding_format='float');assert len(r.data)==2
for row in r.data:assert len(row.embedding)>0 and all(math.isfinite(x) for x in row.embedding)
assert r.data[0].embedding!=r.data[1].embedding
print('PASS embeddings',len(r.data[0].embedding),'dimensions',flush=True)
oa.close();an.close()
print('ALL REAL MODEL SDK CHECKS PASSED',flush=True)
