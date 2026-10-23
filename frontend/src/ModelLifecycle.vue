<script setup lang="ts">
import {onMounted,onUnmounted,ref} from 'vue'
import {gatewayRequest} from './gateway-client'
defineProps<{t:(text:string)=>string}>()
const items=ref<any[]>([]),policy=ref<any>(null),seconds=ref(60),error=ref(''),busy=ref(false)
let timer:ReturnType<typeof setInterval>,disposed=false,editing=false
async function refresh(){
 if(busy.value)return
 busy.value=true
 try{
  const [lifecycle,residency]=await Promise.all([gatewayRequest('/api/v1/model-lifecycle'),gatewayRequest('/api/v1/model-residency')])
  if(!disposed){items.value=lifecycle.items;policy.value=residency;if(!editing)seconds.value=residency.idle_seconds;error.value=''}
 }catch(e){if(!disposed)error.value=e instanceof Error?e.message:String(e)}finally{busy.value=false}
}
async function save(){if(busy.value)return;busy.value=true;try{await gatewayRequest('/api/v1/model-residency',{idle_seconds:seconds.value},'PUT');editing=false}catch(e){error.value=e instanceof Error?e.message:String(e);return}finally{busy.value=false}await refresh()}
const reasons:Record<string,string>={EXECUTION_NODE_UNAVAILABLE:'执行节点尚未就绪',GATEWAY_MODEL_NOT_DISCOVERED:'网关尚未发现该服务模型',NO_LINKED_GATEWAY_SERVICE:'未关联可发现的推理服务',UPSTREAM_UNAVAILABLE:'推理后端不可达',ROUTING_PAUSED:'路由已暂停',WEIGHTS_NOT_READY:'权重尚未就绪',SCHEDULER_BINDING_PENDING:'调度绑定尚未就绪'}
onMounted(()=>{void refresh();timer=setInterval(refresh,5000)})
onUnmounted(()=>{disposed=true;clearInterval(timer)})
</script>
<template>
 <section class="panel lifecycle"><div class="section-head"><h2>{{t('模型接入状态')}}</h2><button class="quiet" :disabled="busy" @click="refresh">{{t('刷新')}}</button></div>
 <p>{{t('关联权重、推理服务、网关模型与调度绑定；实际资源在调用时检查。')}}</p><p v-if="error" class="error" role="alert">{{error}}</p>
 <article v-for="item in items" :key="item.model_id"><h3>{{item.name||item.model_id}}</h3><code>{{item.model_id}}</code><p>{{t('权重状态')}}: {{item.weight_state}}</p><p v-if="item.selected_files?.length">{{item.selected_files.length}} {{t('个文件')}} · {{t('READY 仅表示所选范围完成')}}</p>
  <p v-for="reason in item.blockers" :key="reason">{{t(reasons[reason]||reason)}}</p>
  <p v-if="item.blockers?.includes('NO_LINKED_GATEWAY_SERVICE')">{{t('在推理服务中关联仓库模型，并将该服务注册到统一网关。下载完成不会自动部署外部推理后端。')}}</p>
  <div v-for="deployment in item.deployments" :key="deployment.service_id+'/'+deployment.native_model"><strong>{{deployment.service_id}}</strong><p><code>{{deployment.gateway_model}}</code></p><p>{{t(deployment.configured_callable?'接入就绪':'接入未就绪')}} · {{deployment.scheduler_state}} · {{deployment.route_state}}</p><p v-for="reason in deployment.blockers" :key="reason">{{t(reasons[reason]||reason)}}</p></div>
 </article><p v-if="!items.length&&!busy">{{t('暂无本地模型')}}</p></section>
 <section class="panel lifecycle"><h2>{{t('模型驻留与卸载')}}</h2><p>{{t('QA、Embedding、Rerank 可同时驻留。空闲时保留已确认的模型内存，调用前重新申请完整计算预算；资源不足时按空闲先后卸载，到期仍确认卸载。')}}</p>
  <form @submit.prevent="save"><label>{{t('空闲保留秒数（0 为每次卸载）')}} <input v-model.number="seconds" type="number" min="0" max="600" required @input="editing=true"></label><button :disabled="busy">{{t('保存')}}</button></form>
  <p v-for="resident in (policy?.residents || (policy?.resident ? [policy.resident] : []))" :key="resident.permit_id">{{t('驻留模型')}}: {{resident.upstream_id}} / {{resident.model_id}} · {{resident.idle_seconds}}s</p><p v-if="!policy?.resident && !policy?.residents?.length">{{t('当前没有空闲驻留模型')}}</p>
 </section>
</template>
<style scoped>.lifecycle{padding:22px;margin-bottom:18px}.lifecycle article{padding:15px 0;border-bottom:1px solid #e2e8f0}.lifecycle p{font-size:13px;color:#64748b;line-height:1.7}.lifecycle code{overflow-wrap:anywhere}.lifecycle form{display:flex;align-items:center;gap:12px;flex-wrap:wrap}.lifecycle input{width:90px;padding:8px;border:1px solid #d6e0eb;border-radius:8px}</style>
