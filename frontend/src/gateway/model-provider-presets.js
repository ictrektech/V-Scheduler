import {NATIVE_PROVIDERS} from './native-providers.js';
import {MODELS_DEV_PROVIDERS} from './models-dev-providers.js';
// These are identity aliases, not new provider implementations. Product/region
// URLs remain separate. SDK defaults preserve its own URL construction and auth.
const SDK_ALIASES = {
 'fireworks-ai':'fireworks',togetherai:'together',vercel:'vercel-ai-gateway',
 'alibaba-token-plan':'qwen-token-plan','alibaba-token-plan-cn':'qwen-token-plan-cn',
 'kimi-code-plan-global':'kimi-coding','zai-coding-plan':'zai','zhipuai-coding-plan':'zai-coding-cn',
};
function presets(providers){
 const used=new Set(),result=[];
 for(const p of providers){
  const nativeId=SDK_ALIASES[p.id]||(p.id==='zai'?'':p.id);
  const native=NATIVE_PROVIDERS.find(n=>n.id===nativeId);
  if(native)used.add(native.id);
  const protocol=native?'pi':p.id==='ollama-cloud'?'ollama':p.registrationProtocol;
  result.push({...p,catalogId:p.id,base:native?native.base:p.registrationBase||'',protocol:protocol||'openai',nativeOnly:!!native,dshIds:native?[native.id]:[],requiresGateway:!protocol,
    hint:!protocol?'已授权的兼容网关地址':undefined});
 }
 for(const native of NATIVE_PROVIDERS){
  if(used.has(native.id)||['typesafe','qwen-token-plan-individual'].includes(native.id)||native.id==='ant-ling'&&providers.some(p=>p.id==='bailing'))continue;
  result.push({...native,catalogId:'',protocol:'pi',nativeOnly:true,dshIds:[native.id]});
 }
 result.push({id:'typesafe',name:'TypeSafe / Jev',base:'https://api.typesafe.ai/v1',protocol:'typesafe',model:'jev-latest'});
 // Models.dev already owns the Ollama Cloud identity when present.
 return result.sort((a,b)=>a.name.localeCompare(b.name));
}
export const PROVIDER_PRESETS=presets(MODELS_DEV_PROVIDERS);
export function replaceProviderCatalog(providers){
 if(!Array.isArray(providers)||!providers.length)return;
 const unique=new Map(providers.filter(p=>typeof p.id==='string'&&typeof p.name==='string').map(p=>[p.id,p]));
 PROVIDER_PRESETS.splice(0,PROVIDER_PRESETS.length,...presets([...unique.values()]));
}
