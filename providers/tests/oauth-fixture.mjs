// Test-only preload: never included by the production worker entry point.
const original=globalThis.fetch;
globalThis.fetch=async(input,init)=>{
 const url=String(input instanceof Request?input.url:input);
 if(url==='https://platform.claude.com/v1/oauth/token') {
  const body=JSON.parse(init.body);
  if(body.refresh_token!=='fixture-refresh')throw Error('unexpected refresh credential');
  return new Response(JSON.stringify({access_token:'fixture-renewed-access',refresh_token:'fixture-rotated-refresh',expires_in:3600}),{status:200,headers:{'content-type':'application/json'}});
 }
 if(!url.startsWith('http://127.0.0.1:'))throw Error('unexpected network request in OAuth fixture');
 return original(input,init);
};
