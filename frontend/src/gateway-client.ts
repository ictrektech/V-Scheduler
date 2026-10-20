import {apiBases,authenticatedFetch,jsonResponse} from './http-client.js'
import {getVOSBearerToken,clearCachedVOSBearerToken} from './vos-auth'
export async function gatewayRequest(path:string,body?:unknown,method?:string):Promise<any>{
 const response=await authenticatedFetch(apiBases(location.pathname).gateway+path,{
  method:method||(body===undefined?'GET':'POST'),
  headers:body===undefined?undefined:{'Content-Type':'application/json'},
  body:body===undefined?undefined:JSON.stringify(body)
 },{token:async()=>await getVOSBearerToken()||localStorage.getItem('v-router-key')||null,clearToken:clearCachedVOSBearerToken})
 return jsonResponse(response)
}
