<script setup lang="ts">
import {onMounted,onUnmounted,ref} from 'vue'
import {isSessionError,recordDiagnostic} from './diagnostics.js'
import {gatewayRequest} from './gateway-client'
const props=defineProps<{t:(text:string)=>string,enabled:boolean}>()
const items=ref<any[]>([]),error=ref(''),busy=ref(false),loaded=ref(false)
let timer:ReturnType<typeof setInterval>,disposed=false
async function refresh(){
 if(!props.enabled||busy.value)return
 busy.value=true
 try{const result=await gatewayRequest('/api/v1/inference-queue');if(!disposed){items.value=result.items;loaded.value=true;error.value=''}}
 catch(e){if(!disposed){if(isSessionError(e))recordDiagnostic('AUTH_REQUEST_REJECTED','inference_queue');else error.value=props.t('暂时无法刷新推理等待队列。')}}
 finally{busy.value=false}
}
async function cancel(id:string){try{await gatewayRequest('/api/v1/inference-queue/'+encodeURIComponent(id),undefined,'DELETE');await refresh()}catch(e){if(isSessionError(e)){recordDiagnostic('AUTH_REQUEST_REJECTED','inference_queue');error.value=props.t('连接恢复后可重试，输入已保留。')}else error.value=props.t('取消推理请求失败，请刷新队列后重试。')}}
onMounted(()=>{void refresh();timer=setInterval(refresh,2500)})
onUnmounted(()=>{disposed=true;clearInterval(timer)})
</script>
<template>
 <section class="section panel">
  <div class="section-head"><h2>{{t('推理等待队列')}}</h2><button class="quiet" :disabled="busy||!enabled" @click="refresh">{{t('刷新')}}</button></div>
  <p>{{t('交互请求优先；后台请求随等待时间提升优先级。仅显示当前身份可见的请求。')}}</p>
  <p v-if="error" role="status">{{error}}</p>
  <div class="table-wrap"><table><thead><tr><th>{{t('位置')}}</th><th>{{t('模型 / 应用')}}</th><th>{{t('优先级')}}</th><th>{{t('等待')}}</th><th>{{t('原因')}}</th><th>{{t('操作')}}</th></tr></thead>
  <tbody><tr v-for="item in items" :key="item.request_id"><td>{{item.position}}</td><td>{{item.model_id}}<small>{{item.app_id}}</small></td><td>{{t(item.priority==='high'?'高优先级':item.priority==='interactive'?'交互':'后台')}}</td><td>{{(item.wait_ms/1000).toFixed(1)}}s</td><td>{{item.reason}}</td><td><button class="quiet" @click="cancel(item.request_id)">{{t('取消')}}</button></td></tr><tr v-if="!items.length"><td colspan="6">{{t(loaded?'没有等待中的推理请求':'正在读取推理等待队列…')}}</td></tr></tbody></table></div>
 </section>
</template>
<style scoped>p,small{font-size:12px;color:#64748b}small{display:block}td{overflow-wrap:anywhere}</style>
