export const base = location.pathname.startsWith('/app/') ? '/app/com.ictrek.v-scheduler/model-hub' : '/model-hub'
export async function request(path: string, body?: unknown, method?: string): Promise<any> {
 const form=body instanceof FormData
 const response=await fetch(base+path,{method:method||(body===undefined?'GET':'POST'),headers:body===undefined||form?undefined:{'Content-Type':'application/json'},body:body===undefined?undefined:form?body:JSON.stringify(body)})
 const payload=await response.json().catch(()=>({detail:`HTTP ${response.status}`}))
 if(!response.ok)throw new Error(payload.detail||`HTTP ${response.status}`)
 return payload
}
export const encode=encodeURIComponent
export function fileSize(value:number|null|undefined):string {
 if(value===undefined||value===null)return '—'
 const units=['B','KiB','MiB','GiB','TiB'];let i=0;while(value>=1024&&i<units.length-1){value/=1024;i++}return `${value.toFixed(i?1:0)} ${units[i]}`
}
