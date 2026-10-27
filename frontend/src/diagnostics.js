// Deliberately structured: never accept raw error messages, tokens or URLs.
const key='v-scheduler:diagnostics',limit=200,listeners=new Set();
let entries=[];
try {const saved=JSON.parse(globalThis.sessionStorage?.getItem(key)||'[]');if(Array.isArray(saved))entries=saved.filter(valid).slice(-limit).map(({code,stage,level,at,count})=>({code,stage,level,at,count}));}catch{}
function valid(row){return row&&/^[A-Z0-9_]{1,64}$/.test(row.code)&&/^[a-z0-9_.-]{1,64}$/.test(row.stage)&&['info','warning'].includes(row.level)&&typeof row.at==='string'&&Number.isFinite(Date.parse(row.at))&&Number.isInteger(row.count)&&row.count>0;}
export function diagnosticSnapshot(){return entries.map(e=>({...e}));}
function publish(){try{globalThis.sessionStorage?.setItem(key,JSON.stringify(entries));}catch{}for(const listener of listeners)listener(diagnosticSnapshot());}
export function recordDiagnostic(code,stage,level='warning'){
 const row={code,stage,level,at:new Date().toISOString(),count:1};if(!valid(row))return;
 const previous=entries.at(-1);
 if(previous&&previous.code===code&&previous.stage===stage&&Date.now()-Date.parse(previous.at)<5000){previous.count++;previous.at=row.at;}else entries.push(row);
 entries=entries.slice(-limit);publish();
}
export function subscribeDiagnostics(listener){listeners.add(listener);listener(diagnosticSnapshot());return()=>listeners.delete(listener);}
export function clearDiagnostics(){entries=[];publish();}
export function authDiagnosticCode(error){const value=String(error?.message||error?.msg||error?.error?.message||error||'');return /ExpiredSignature|JWT.*(?:expired|过期)/i.test(value)?'VOS_JWT_EXPIRED':/state mismatch/i.test(value)?'VOS_OIDC_STATE_MISMATCH':'VOS_AUTH_RECOVERY_FAILED';}
export function isSessionError(error){return error?.sessionError!==false&&(error?.status===401&&!String(error?.code||'').startsWith('PROVIDER_')||/ExpiredSignature|authorize\/confirm.*(?:10001|JWT)/i.test(String(error?.message||error?.msg||error?.error?.message||error||'')));}
export function userErrorMessage(error){if(isSessionError(error)){recordDiagnostic(authDiagnosticCode(error),'request_auth');return '';}return error instanceof Error?error.message:String(error);}
