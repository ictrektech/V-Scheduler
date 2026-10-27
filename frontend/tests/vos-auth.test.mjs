import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import ts from 'typescript';
import {createHash,webcrypto} from 'node:crypto';

const source = await readFile(new URL('../src/vos-auth.ts', import.meta.url), 'utf8');
let sequence=0;
const jwt=exp=>`header.${Buffer.from(JSON.stringify({exp})).toString('base64url')}.signature`;
async function fixture() {
  const handlers=new Map();
  const values=new Map();
  globalThis.localStorage={getItem:k=>values.get(k)||null,removeItem:k=>values.delete(k),key:i=>[...values.keys()][i],get length(){return values.size}};
  globalThis.sessionStorage={removeItem(){}};
  let authorizations=0;
  globalThis.window={setInterval,clearInterval,addEventListener:(name,fn)=>handlers.set(name,fn),vos_platform:{api:{v1000:{oauth2:{
    async authorize(params){authorizations++;return {code:'test-code',state:params.state}},
    async token(){return {access_token:'renewed-oauth-token',expires_in:3600}},
  }}}}};
  window.parent=window;
  const output=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText.replace(/(['"])@noble\/hashes\/sha2\.js\1/g, JSON.stringify(import.meta.resolve('@noble/hashes/sha2.js'))).replace(/(['"])\.\/diagnostics\.js\1/g,JSON.stringify(new URL('../src/diagnostics.js',import.meta.url).href));
  const auth=await import(`data:text/javascript;base64,${Buffer.from(output).toString('base64')}#${sequence++}`);
  return {auth,values,authorizations:()=>authorizations,storage:event=>handlers.get("storage")?.(event)};
}
test('polling follows platform token rotation without reloading the iframe',async()=>{
 const {auth,values,authorizations}=await fixture();
 window.__VOS_ACCESS_TOKEN__='old-injected-token';
 values.set('core-access',JSON.stringify({accessToken:'initial-platform-token'}));
 assert.equal(await auth.getVOSBearerToken(),'initial-platform-token');
 values.set('core-access',JSON.stringify({accessToken:'rotated-platform-token'}));
 assert.equal(await auth.getVOSBearerToken(),'rotated-platform-token');
 assert.equal(authorizations(),0);
});
test('expired injected JWT is skipped and concurrent renewal is shared',async()=>{
 const {auth,authorizations}=await fixture();
 window.__VOS_ACCESS_TOKEN__=jwt(Math.floor(Date.now()/1000)-1);
 assert.deepEqual(await Promise.all([auth.getVOSBearerToken(),auth.getVOSBearerToken()]),['renewed-oauth-token','renewed-oauth-token']);
 assert.equal(authorizations(),1);
});
test('a failed old request cannot erase a newly rotated credential',async()=>{
 const {auth,values,authorizations}=await fixture();
 values.set('core-access',JSON.stringify({accessToken:'old'}));
 assert.equal(await auth.getVOSBearerToken(),'old');
 values.set('core-access',JSON.stringify({accessToken:'new'}));
 assert.equal(await auth.getVOSBearerToken(),'new');
 auth.clearCachedVOSBearerToken('old');
 assert.equal(auth.getCachedVOSBearerToken(),'new');
 assert.equal(await auth.getVOSBearerToken(),'new');
 assert.equal(authorizations(),0);
});
test('server-rejected injection is not reused after the former bypass cooldown',async()=>{
 const {auth}=await fixture();
 window.__VOS_ACCESS_TOKEN__='rejected';
 assert.equal(await auth.getVOSBearerToken(),'rejected');
 auth.clearCachedVOSBearerToken('rejected');
 assert.equal(await auth.getVOSBearerToken(),'renewed-oauth-token');
 const now=Date.now;Date.now=()=>now()+61_000;
 try{assert.equal(await auth.getVOSBearerToken(),'renewed-oauth-token')}finally{Date.now=now}
});

test('PKCE remains S256 when the VOS HTTP origin has no Web Crypto subtle API', async()=>{
 const saved=Object.getOwnPropertyDescriptor(globalThis,'crypto');
 Object.defineProperty(globalThis,'crypto',{configurable:true,value:{getRandomValues:webcrypto.getRandomValues.bind(webcrypto)}});
 try {
  const {auth,authorizations}=await fixture();
  assert.equal(await auth.sha256Base64url('RFC7636-verifier'),createHash('sha256').update('RFC7636-verifier').digest('base64url'));
  assert.equal(await auth.getVOSBearerToken(),'renewed-oauth-token');
  assert.equal(authorizations(),1);
 } finally { if(saved)Object.defineProperty(globalThis,'crypto',saved);else delete globalThis.crypto; }
});

test('application refresh token renews an idle session without calling authorize/confirm again',async()=>{
 const {auth,values,authorizations}=await fixture();window.parent={};const grants=[];
 values.set('core-access',JSON.stringify({accessToken:jwt(Math.floor(Date.now()/1000)+3600)}));
 window.vos_platform.api.v1000.oauth2.token=async input=>{grants.push(input.grant_type);return {access_token:'app-'+grants.length,refresh_token:'private-refresh',expires_in:60}};
 await auth.getVOSBearerToken();await new Promise(setImmediate);
 const now=Date.now;Date.now=()=>now()+2*3600_000;
 try {
  assert.deepEqual(await Promise.all([auth.getVOSBearerToken(),auth.getVOSBearerToken()]),['app-2','app-2']);
  assert.deepEqual(grants,['authorization_code','refresh_token']);assert.equal(authorizations(),1);
 }finally{Date.now=now;}
});
test('ExpiredSignature is logged safely and a rotated platform token recovers without an exception',async()=>{
 const {auth,values}=await fixture();const diagnostics=await import('../src/diagnostics.js');diagnostics.clearDiagnostics();
 window.vos_platform.api.v1000.oauth2.authorize=async()=>{setTimeout(()=>values.set('core-access',JSON.stringify({accessToken:'rotated-after-expiry'})),10);throw Error('authorize/confirm code=10001 RS256 JWT ExpiredSignature Bearer private-fixture-token');};
 assert.equal(await auth.getVOSBearerToken(),'rotated-after-expiry');
 const log=diagnostics.diagnosticSnapshot();assert.ok(log.some(row=>row.code==='VOS_JWT_EXPIRED'));assert.ok(!JSON.stringify(log).includes('private-fixture-token'));assert.ok(!JSON.stringify(log).includes('authorize/confirm'));
});
test('platform logout fences a pending application grant and prevents stale-token reuse',async()=>{
 const {auth,values,storage}=await fixture();window.parent={};let finish;
 const pending=new Promise(resolve=>finish=resolve);
 values.set('core-access',JSON.stringify({accessToken:'platform-before-logout'}));
 window.vos_platform.api.v1000.oauth2.token=async()=>{await pending;return {access_token:'late-user-token',refresh_token:'private-refresh'}};
 await auth.getVOSBearerToken();await new Promise(setImmediate);
 values.delete('core-access');storage({key:'core-access',newValue:null});finish();await new Promise(setImmediate);
 assert.equal(auth.getCachedVOSBearerToken(),null);assert.equal(await auth.getVOSBearerToken(),null);
});
