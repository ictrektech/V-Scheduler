import {recordDiagnostic} from './diagnostics.js';
// Shared transport for the unified UI. Successful service payloads are not reshaped.
export function apiBases(pathname) {
 const gateway=pathname.startsWith('/app/')||pathname.startsWith('/api/com.ictrek.v-scheduler')?'/api/com.ictrek.v-scheduler':'';
 return {gateway,scheduler:gateway+'/scheduler/api/v1/',models:gateway+'/model-hub'};
}
export function apiError(payload,status) {
 const candidate=typeof payload?.code==='string'?payload.code:payload?.error?.code;
 const code=typeof candidate==='string'?candidate:'';
 const reason=payload?.error?.message||payload?.detail||payload?.msg||`HTTP ${status}`;
 const id=typeof payload?.request_id==='string'?payload.request_id:'';
 const message=typeof reason==='string'?reason:`HTTP ${status}`;
 const error=new Error(message+(code&&code!==message?` [${code}]`:'')+(id?` · ${id}`:''));
 Object.assign(error,{status,code:code||'',requestId:id,sessionError:status===401&&!String(code||'').startsWith('PROVIDER_')&&(payload?.category==='v-scheduler'||['UNAUTHENTICATED','UNAUTHORIZED'].includes(code))});return error;
}
export async function jsonResponse(response) {
 const text=await response.text();let payload=null;
 try {payload=text?JSON.parse(text):null} catch {
  if(response.ok)throw apiError({code:'INVALID_RESPONSE',msg:'Service returned invalid JSON'},response.status);
 }
 if(!response.ok||(payload?.category==='v-scheduler'&&typeof payload.code==='string'&&payload.code!=='OK'))throw apiError(payload,response.status);
 return payload;
}
export async function authenticatedFetch(url,init={},auth={}) {
 const send=async()=>{
  const token=await auth.token?.()||'';const headers=new Headers(init.headers);
  if(token)headers.set('Authorization',`Bearer ${token}`);
  return {token,response:await (auth.fetch||fetch)(url,{...init,headers})};
 };
 let result=await send();
 if(result.response.status===401&&auth.clearToken&&!init.signal?.aborted&&await browserIdentityFailure(result.response)){
  recordDiagnostic('AUTH_REQUEST_RETRY','http_401_retry','info');
  auth.clearToken(result.token);
  await result.response.body?.cancel();
  result=await send();
 }
 return result.response;
}

export async function browserIdentityFailure(response){
 try{const payload=await response.clone().json();const code=payload?.error?.code||payload?.code;if(String(code||'').startsWith('PROVIDER_'))return false;if(payload?.category==='v-scheduler'||['UNAUTHENTICATED','UNAUTHORIZED'].includes(code))return true;return !String(code||'').startsWith('PROVIDER_')&&!['invalid_api_key','authentication_error'].includes(payload?.error?.type);}catch{return true;}
}
