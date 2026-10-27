// Private one-request child, stdin carries secrets; stdout carries framed HTTP.
// No listening port, inherited provider credentials, or public management API.
import fs from 'node:fs/promises';
import path from 'node:path';
import presets from './presets.json' with {type:'json'};
import {once} from 'node:events';
import {builtinProviders} from '@earendil-works/pi-ai/providers/all';
import {createModels} from '@earendil-works/pi-ai/models';
import {contextFromChat,streamOptions,completion,chunk,usage,finishReason,InputError} from './bridge.mjs';
let headersSent=false;
async function write(v){if(!process.stdout.write(v))await once(process.stdout,'drain');}
async function head(status,type='application/json'){if(headersSent)return;headersSent=true;await write(JSON.stringify({status,content_type:type})+'\n');}
const failure=(status,message)=>({status,message});
const signal=AbortSignal.timeout(120000);
try {
  process.stdin.setEncoding('utf8');
  let raw='';for await(const part of process.stdin){raw+=part;if(raw.length>18*1024*1024)throw new InputError('provider request too large');}
  const input=JSON.parse(raw);
  const providers=builtinProviders();
  let provider=providers.find(p=>p.id===input.provider);
  if(provider?.id==='qwen-token-plan') {
    const base=provider, alias=providers.find(p=>p.id==='qwen-token-plan-individual');
    provider={...base,getModels:()=>[...new Map([...base.getModels(),...(alias?.getModels()||[])].map(m=>[m.id,{...m,provider:base.id}])).values()]};
  }
  if(!provider)throw new InputError('unknown native provider');
  const credentialPath=input.credential_path;
  let credential=input.credential?.type==='oauth'?input.credential:{...input.credential,type:'api_key',key:input.credential?.key??input.api_key??'',env:input.credential?.env||{}};
  const env={...credential.env,AWS_MAX_ATTEMPTS:'1'};
  // An isolated process gives SDKs which still read process.env a scoped env.
  for(const [k,v] of Object.entries(env)) {
    if(!/^(AWS_|AZURE_|GOOGLE_|GCLOUD_|CLOUDFLARE_|OPENAI_|GITHUB_|COPILOT_|RADIUS_)[A-Z0-9_]+$/.test(k)||/FILE|PROFILE|PROXY|ENDPOINT|SKIP_AUTH|CREDENTIAL_PROCESS/.test(k)||typeof v!=='string')throw new InputError('unsupported provider environment key');
    process.env[k]=v;
  }
  if(credentialPath){try {credential=JSON.parse(await fs.readFile(credentialPath,'utf8'));}catch(e){if(e.code!=='ENOENT')throw failure(503,'provider credential store unavailable');}}
  const store={read:async()=>credential,list:async()=>[],delete:async()=>{},modify:async(_,fn)=>{
    credential=(await fn(credential))??credential;
    if(credentialPath){await fs.mkdir(path.dirname(credentialPath),{recursive:true,mode:0o700});const tmp=credentialPath+'.'+process.pid;await fs.writeFile(tmp,JSON.stringify(credential),{mode:0o600});await fs.rename(tmp,credentialPath);}
    return credential;
  }};
  const models=createModels({credentials:store,authContext:{env:async k=>env[k],fileExists:async()=>false}});
  models.setProvider(provider);
  // Resolve and persist refresh while Rust holds the per-credential auth lock.
  const auth=await models.getAuth(provider.id,{signal});
  if(!auth)throw failure(401,'native provider credentials are missing or incomplete');
  await write('{"ready":true}\n');
  if(input.operation==='models') {
    const result=await models.refresh({providers:[provider.id],allowNetwork:true,force:true,signal});
    if(result.errors.size)throw failure(502,'native provider model discovery failed');
    const rows=models.getModels(provider.id).map(m=>({id:m.id,kind:'chat',capabilities:['chat','completion','tools',...(m.input.includes('image')?['vision']:[]),...(m.reasoning?['thinking']:[])],endpoints:['chat_completions']}));
    await head(200);await write(JSON.stringify({models:rows,catalog:true}));
  } else {
    const body=input.body;
    let model=models.getModel(provider.id,body.model);
    if(!model) {await models.refresh({providers:[provider.id],allowNetwork:true,signal});model=models.getModel(provider.id,body.model);}
    if(!model)throw failure(404,'model is absent from native provider catalog; use a transparent upstream for custom model IDs');
    const defaultBase=presets.find(p=>p.id===provider.id)?.base;
    model={...model,...(input.base_url && input.base_url!==defaultBase?{baseUrl:input.base_url}:{})};
    const context=contextFromChat(body,model), options={...streamOptions(body,model),env,signal};
    let upstreamStatus=502;
    options.onResponse=r=>{upstreamStatus=r.status;};
    if(['openai-completions','openai-responses','azure-openai-responses','openai-codex-responses','anthropic-messages'].includes(model.api)) {
      options.fetch=async(...args)=>{const response=await fetch(...args);upstreamStatus=response.status;return response;};
    }
    const stream=models.streamSimple(model,context,options);
    const id='chatcmpl-'+crypto.randomUUID();const toolIndices=new Map();
    let started=false,terminal=false;
    async function delta(v,reason=null,extra={}){await head(200,'text/event-stream');if(!started){started=true;await write('data: '+JSON.stringify(chunk(id,body.model,{role:'assistant',content:''}))+'\n\n');}await write('data: '+JSON.stringify(chunk(id,body.model,v,reason,extra))+'\n\n');}
    for await(const event of stream) {
      if(event.type==='error') {
        const recorded=event.error.diagnostics?.find(d=>Number.isInteger(d.details?.status));
        if(recorded)upstreamStatus=recorded.details.status;
        if(signal.aborted)upstreamStatus=504;
        const status=Number.isInteger(upstreamStatus)&&upstreamStatus>=400&&upstreamStatus<=599?upstreamStatus:502;
        throw failure(status,'native provider request failed; check credentials, model access and provider status');
      }
      if(!body.stream){if(event.type==='done'){terminal=true;await head(200);await write(JSON.stringify(completion(event.message,id)));}continue;}
      if(event.type==='text_delta')await delta({content:event.delta});
      if(event.type==='thinking_delta')await delta({reasoning_content:event.delta});
      if(event.type==='toolcall_start'){
        const call=event.partial.content[event.contentIndex],index=toolIndices.size;toolIndices.set(event.contentIndex,index);
        await delta({tool_calls:[{index,id:call.id,type:'function',function:{name:call.name,arguments:''}}]});
      }
      if(event.type==='toolcall_delta')await delta({tool_calls:[{index:toolIndices.get(event.contentIndex),function:{arguments:event.delta}}]});
      if(event.type==='done'){terminal=true;await delta({},finishReason(event.reason));if(body.stream_options?.include_usage)await write('data: '+JSON.stringify({...chunk(id,body.model,{}),choices:[],usage:usage(event.message.usage)})+'\n\n');await write('data: [DONE]\n\n');}
    }
    if(!terminal)throw failure(502,'native provider stream ended without completion');
  }
} catch(error) {
  const status=error.status|| (error.code==='oauth'||error.code==='auth'?401:502);
  const message=error instanceof InputError||error.status?error.message:'native provider operation failed; verify provider configuration';
  const value=JSON.stringify({error:{code:status===401?'PROVIDER_AUTH_REQUIRED':'PROVIDER_ERROR',type:status===400?'invalid_request_error':'api_error',message}});
  if(headersSent)await write('data: '+value+'\n\n');else {await head(status);await write(value);}
}
