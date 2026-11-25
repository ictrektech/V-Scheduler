import {recordDiagnostic} from './diagnostics.js';
// Shared transport for the unified UI. Successful service payloads are not reshaped.
export function apiBases(pathname) {
 const gateway=pathname.startsWith('/app/')||pathname.startsWith('/api/com.ictrek.v-scheduler')?'/api/com.ictrek.v-scheduler':'';
 return {gateway,scheduler:gateway+'/scheduler/api/v1/',models:gateway+'/model-hub'};
}
export function lanBaseURL(bases = {}, hostname = window.location.hostname) {
 return (bases?.lan || 'http://<vos-host>:22888/v1')
  .replace('<vos-host>', hostname).replace('<host-port>', '22888');
}
export function apiError(payload,status) {
 const candidate=typeof payload?.code==='string'?payload.code:payload?.error?.code;
 const code=typeof candidate==='string'?candidate:'';
 const reason=payload?.error?.message||payload?.detail||payload?.msg||`HTTP ${status}`;
 const id=typeof payload?.request_id==='string'?payload.request_id:'';
 const message=typeof reason==='string'?reason:`HTTP ${status}`;
 const error=new Error(message+(code&&code!==message?` [${code}]`:'')+(id?` · ${id}`:''));
 Object.assign(error,{status,code:code||'',requestId:id,gatewayRouteMissing:status===404&&!payload,sessionError:status===401&&isBrowserIdentityPayload(payload)});return error;
}
export async function jsonResponse(response) {
 const text=await response.text();let payload=null;
 try {payload=text?JSON.parse(text):null} catch {
  if(response.ok)throw apiError({code:'INVALID_RESPONSE',msg:'Service returned invalid JSON'},response.status);
 }
 if(!response.ok||(payload?.category==='v-scheduler'&&typeof payload.code==='string'&&payload.code!=='OK'))throw apiError(payload,response.status);
 return payload;
}
const transientStatuses=new Set([408,502,503,504]);
const wait=milliseconds=>new Promise(resolve=>setTimeout(resolve,milliseconds));
export function isTemporaryReadFailure(error) {
 return transientStatuses.has(error?.status)||error?.gatewayRouteMissing===true||error instanceof TypeError||error?.code==='INVALID_RESPONSE';
}
export async function retryReadFetch(url,init={},fetcher=fetch) {
 const method=(init.method||'GET').toUpperCase();
 if(method!=='GET'&&method!=='HEAD')return fetcher(url,init);
 for(const delay of [0,250,850]){
  if(delay)await wait(delay);
  if(init.signal?.aborted)throw init.signal.reason;
  const timeout=new AbortController();
  const timer=setTimeout(()=>timeout.abort(new Error('Read request timed out')),15_000);
  const signal=init.signal?AbortSignal.any([init.signal,timeout.signal]):timeout.signal;
  try {
   const response=await fetcher(url,{...init,signal});
   const routeMissing=response.status===404&&!response.headers.get('content-type')?.includes('json');
   if((!transientStatuses.has(response.status)&&!routeMissing)||delay===850)return response;
   await response.body?.cancel();
  }catch(error){
   if(init.signal?.aborted||delay===850)throw error;
  }finally{
   clearTimeout(timer);
  }
 }
}
export async function authenticatedFetch(url,init={},auth={}) {
 const send=token=>{
  const headers=new Headers(init.headers);
  if(token)headers.set('Authorization',`Bearer ${token}`);
  return retryReadFetch(url,{...init,headers},auth.fetch||fetch);
 };
 const isRead=['GET','HEAD'].includes((init.method||'GET').toUpperCase());
 let token=await auth.token?.()||'';
 let response=await send(token);
 const rejected=async()=>response.status===401&&auth.clearToken&&!init.signal?.aborted&&await browserIdentityFailure(response);
 if(!await rejected())return response;
 recordDiagnostic('AUTH_REQUEST_RETRY','http_401_retry','info');
 if(token)auth.clearToken(token);
 for(const delay of isRead?[0,300,900,1800]:[0]){
  if(delay)await wait(delay);
  if(init.signal?.aborted)break;
  const renewed=await auth.token?.()||'';
  if(!renewed||renewed===token)continue;
  await response.body?.cancel();
  token=renewed;
  response=await send(token);
  if(!await rejected())return response;
  auth.clearToken(token);
 }
 return response;
}

function isBrowserIdentityPayload(payload) {
 const code=payload?.error?.code||payload?.code;
 if(String(code||'').startsWith('PROVIDER_')||String(payload?.error?.message||payload?.msg||'').startsWith('API key owner is'))return false;
 if(payload?.category==='v-scheduler'||['UNAUTHENTICATED','UNAUTHORIZED'].includes(code))return true;
 return !['invalid_api_key','authentication_error'].includes(payload?.error?.type);
}
export async function browserIdentityFailure(response){
 try{return isBrowserIdentityPayload(await response.clone().json());}catch{return true;}
}
