<script setup lang="ts">
import { onMounted, onUnmounted, ref, watch } from 'vue'
import markup from './gateway/markup.html?raw'
import css from './gateway/style.css?raw'
import { mountGateway } from './gateway/runtime.js'
import { getVOSBearerToken, clearCachedVOSBearerToken } from './vos-auth'
import logo from '../../assets/icon.png'
const props=defineProps<{tab:string;locale:string;visible:boolean}>()
const emit=defineEmits<{select:[tab:string]}>()
const host=ref<HTMLElement>()
let workspace:ReturnType<typeof mountGateway>|undefined
onMounted(async()=>{
 const root=host.value!.attachShadow({mode:'open'})
 root.innerHTML=`<style>${css}</style>${markup.replaceAll('__GATEWAY_ICON__',logo)}`
 const footer=root.querySelector('.side-footer');if(footer)root.querySelector('.top-actions')?.prepend(footer)
 const apiBase=location.pathname.startsWith('/app/')?'/api/com.ictrek.v-scheduler':''
 workspace=mountGateway(root,{apiBase,token:getVOSBearerToken,clearToken:clearCachedVOSBearerToken,locale:()=>props.locale,visible:()=>props.visible,select:(tab:string)=>emit('select',tab)})
 workspace.select(props.tab)
 await workspace.started
})
watch(()=>props.tab,value=>workspace?.select(value))
watch(()=>props.locale,()=>workspace?.updateLocale())
onUnmounted(()=>workspace?.dispose())
</script>
<template><div ref="host" class="gateway-workspace"></div></template>
