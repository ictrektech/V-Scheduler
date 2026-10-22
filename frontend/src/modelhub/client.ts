import {apiBases,authenticatedFetch,jsonResponse} from '../http-client.js'
import {getVOSBearerToken,clearCachedVOSBearerToken} from '../vos-auth'
export const base=apiBases(location.pathname).models
export async function request(path:string,body?:unknown,method?:string):Promise<any>{
 const form=body instanceof FormData
 const response=await authenticatedFetch(base+path,{method:method||(body===undefined?'GET':'POST'),headers:body===undefined||form?undefined:{'Content-Type':'application/json'},body:body===undefined?undefined:form?body:JSON.stringify(body)},{token:async()=>await getVOSBearerToken()||localStorage.getItem('v-router-key')||null,clearToken:clearCachedVOSBearerToken})
 return jsonResponse(response)
}
export const encode=encodeURIComponent
export async function schedulerRequest(path:string,body?:unknown,method?:string):Promise<any>{
 const response=await authenticatedFetch(apiBases(location.pathname).scheduler+path,{method:method||(body===undefined?'GET':'POST'),headers:body===undefined?undefined:{'Content-Type':'application/json'},body:body===undefined?undefined:JSON.stringify(body)},{token:async()=>await getVOSBearerToken()||localStorage.getItem('v-router-key')||null,clearToken:clearCachedVOSBearerToken})
 return (await jsonResponse(response))?.data
}
export function fileSize(value:number|null|undefined):string {
 if(value===undefined||value===null)return '—'
 const units=['B','KiB','MiB','GiB','TiB'];let i=0;while(value>=1024&&i<units.length-1){value/=1024;i++}return `${value.toFixed(i?1:0)} ${units[i]}`
}
