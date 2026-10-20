<script setup lang="ts">
import {onMounted,onUnmounted,ref} from 'vue'
import {gatewayRequest} from './gateway-client'
const props=defineProps<{t:(text:string)=>string,enabled:boolean}>()
const items=ref<any[]>([]),error=ref(''),busy=ref(false)
let timer:ReturnType<typeof setInterval>,disposed=false
async function refresh(){
 if(!props.enabled||busy.value)return
 busy.value=true
 try{const result=await gatewayRequest('/api/v1/inference-queue');if(!disposed){items.value=result.items;error.value=''}}
 catch(e){if(!disposed)error.value=e instanceof Error?e.message:String(e)}
 finally{busy.value=false}
}
async function cancel(id:string){try{await gatewayRequest('/api/v1/inference-queue/'+encodeURIComponent(id),undefined,'DELETE');await refresh()}catch(e){error.value=e instanceof Error?e.message:String(e)}}
onMounted(()=>{void refresh();timer=setInterval(refresh,2500)})
onUnmounted(()=>{disposed=true;clearInterval(timer)})
</script>
<template>
 <section class="section panel">
  <div class="section-head"><h2>{{t('推理等待队列')}}</h2><button class="quiet" :disabled="busy||!enabled" @click="refresh">{{t('刷新')}}</button></div>
  <p>{{t('交互请求优先；后台请求随等待时间提升优先级。仅显示当前身份可见的请求。')}}</p>
  <p v-if="error" class="error" role="alert">{{error}}</p>
  <div class="table-wrap"><table><thead><tr><th>{{t('位置')}}</th><th>{{t('模型 / 应用')}}</th><th>{{t('优先级')}}</th><th>{{t('等待')}}</th><th>{{t('原因')}}</th><th>{{t('操作')}}</th></tr></thead>
  <tbody><tr v-for="item in items" :key="item.request_id"><td>{{item.position}}</td><td>{{item.model_id}}<small>{{item.app_id}}</small></td><td>{{t(item.priority==='interactive'?'交互':'后台')}}</td><td>{{(item.wait_ms/1000).toFixed(1)}}s</td><td>{{item.reason}}</td><td><button class="quiet" @click="cancel(item.request_id)">{{t('取消')}}</button></td></tr><tr v-if="!items.length"><td colspan="6">{{t('没有等待中的推理请求')}}</td></tr></tbody></table></div>
 </section>
</template>
<style scoped>p,small{font-size:12px;color:#64748b}small{display:block}td{overflow-wrap:anywhere}</style>
