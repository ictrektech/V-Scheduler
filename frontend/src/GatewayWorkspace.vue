<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import markup from './gateway/markup.html?raw'
import css from './gateway/style.css?raw'
import { mountGateway } from './gateway/runtime.js'
import { getVOSBearerToken, clearCachedVOSBearerToken } from './vos-auth'
import {request as weightRequest} from './modelhub/client'
import logo from '../../assets/icon.png'
import {apiBases} from './http-client.js'
const props=defineProps<{tab:string;locale:string;visible:boolean;administrator:boolean}>()
const emit=defineEmits<{select:[tab:string],service:[id:string]}>()
const host=ref<HTMLElement>()
let workspace:ReturnType<typeof mountGateway>|undefined
async function unavailableServices(){
 const services:{service_id:string;name?:string;kind:string}[]=await weightRequest('/api/v1/inference-services')
 const results=await Promise.all(services.filter(item=>item.kind&&item.kind!=='ollama').map(async item=>{
  try{
   const detail=await weightRequest('/api/v1/inference-services/'+encodeURIComponent(item.service_id))
   return detail.healthy===false?{id:item.service_id,name:item.name||item.service_id}:null
  }catch{return null}
 }))
 return results.filter(item=>item!==null)
}
onMounted(async()=>{
 const root=host.value!.attachShadow({mode:'open'})
 root.innerHTML=`<style>${css}</style>${markup.replaceAll('__GATEWAY_ICON__',logo)}`
 const legacyAbout=root.querySelector<HTMLButtonElement>('button[onclick="app.openAbout()"]');if(legacyAbout)legacyAbout.style.display='none'
 const footer=root.querySelector('.side-footer');if(footer)root.querySelector('.top-actions')?.prepend(footer)
 const apiBase=apiBases(location.pathname).gateway
 workspace=mountGateway(root,{apiBase,unavailableServices,openService:(id:string)=>emit('service',id),manageKey:(upstreamId:string,model:string)=>window.dispatchEvent(new CustomEvent('vscheduler:model-key',{detail:upstreamId+'/'+model})),token:getVOSBearerToken,clearToken:clearCachedVOSBearerToken,locale:()=>props.locale,visible:()=>props.visible,administrator:()=>props.administrator,select:(tab:string)=>emit('select',tab)})
 workspace.select(props.tab)
 await workspace.started
})
watch(()=>props.tab,value=>workspace?.select(value))
watch(()=>props.locale,()=>workspace?.updateLocale())
watch(()=>props.administrator,()=>workspace?.updatePrivileges())
onUnmounted(()=>workspace?.dispose())
</script>
<template><div ref="host" class="gateway-workspace"></div></template>
