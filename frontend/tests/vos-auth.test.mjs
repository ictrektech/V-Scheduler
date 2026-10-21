import {test} from 'node:test';
import assert from 'node:assert/strict';
import {readFile} from 'node:fs/promises';
import ts from 'typescript';
import {createHash,webcrypto} from 'node:crypto';

const source = await readFile(new URL('../src/vos-auth.ts', import.meta.url), 'utf8');
let sequence=0;
const jwt=exp=>`header.${Buffer.from(JSON.stringify({exp})).toString('base64url')}.signature`;
async function fixture() {
  const values=new Map();
  globalThis.localStorage={getItem:k=>values.get(k)||null,removeItem:k=>values.delete(k),key:i=>[...values.keys()][i],get length(){return values.size}};
  globalThis.sessionStorage={removeItem(){}};
  let authorizations=0;
  globalThis.window={setInterval,clearInterval,vos_platform:{api:{v1000:{oauth2:{
    async authorize(params){authorizations++;return {code:'test-code',state:params.state}},
    async token(){return {access_token:'renewed-oauth-token',expires_in:3600}},
  }}}}};
  const output=ts.transpileModule(source,{compilerOptions:{target:ts.ScriptTarget.ES2022,module:ts.ModuleKind.ES2022}}).outputText.replace(/(['"])@noble\/hashes\/sha2\.js\1/g, JSON.stringify(import.meta.resolve('@noble/hashes/sha2.js')));
  const auth=await import(`data:text/javascript;base64,${Buffer.from(output).toString('base64')}#${sequence++}`);
  return {auth,values,authorizations:()=>authorizations};
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
