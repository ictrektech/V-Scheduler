<script setup lang="ts">
import {ref,onMounted,onUnmounted} from 'vue'
const props=defineProps<{request:(path:string,body?:unknown,method?:string)=>Promise<any>,t:(value:string)=>string}>()
const items=ref<any[]>([]),error=ref(''),busy=ref(false)
let disposed=false,timer:ReturnType<typeof setInterval>|undefined
async function refresh(){try{const value=await props.request('integrations');if(!disposed){items.value=value.items;error.value=''}}catch(e){if(!disposed)error.value=e instanceof Error?e.message:String(e)}}
async function cancelTask(id:string){if(busy.value)return;busy.value=true;try{await props.request('tasks/'+encodeURIComponent(id)+'/cancel',{});await refresh()}catch(e){error.value=e instanceof Error?e.message:String(e)}finally{busy.value=false}}
async function stop(id:string){if(busy.value)return;busy.value=true;try{await props.request('permits/'+encodeURIComponent(id)+'/stop',{});await refresh()}catch(e){error.value=e instanceof Error?e.message:String(e)}finally{busy.value=false}}
onMounted(()=>{void refresh();timer=setInterval(()=>void refresh(),5000)})
onUnmounted(()=>{disposed=true;if(timer)clearInterval(timer)})
</script>
<template>
 <section class="panel integrations">
  <div class="section-head"><h2>{{t('应用统一接入')}}</h2><button class="quiet" @click="refresh">{{t('刷新')}}</button></div>
  <p>{{t('节点遥测由 VOS App 内置组件自动上报；应用登记已批准的执行器并定时心跳，停止请求经原生退出及权重卸载核验后释放预约。')}}</p>
  <p v-if="error" role="alert">{{error}}</p>
  <p v-else-if="!items.length">{{t('尚无应用启用统一生命周期协议；已有应用继续使用原自动绑定契约。')}}</p>
  <article v-for="item in items" :key="item.app_id">
   <h3>{{item.app_id}} · {{t(item.fresh?'心跳新鲜':'心跳已过期')}} · r{{item.revision}}</h3>
   <p>{{t('应用心跳')}}：{{item.heartbeat}}</p>
   <div class="table-scroll"><table><thead><tr><th>{{t('执行器')}}</th><th>{{t('节点')}}</th><th>{{t('批准预算')}}</th><th>{{t('状态')}}</th></tr></thead><tbody><tr v-for="e in item.executors" :key="e.definition.id"><td>{{e.definition.id}}</td><td>{{e.definition.node_id}}</td><td>{{e.definition.resources.cpu_millicores}} mCPU / {{e.definition.resources.memory_mib}} MiB / {{e.definition.resources.gpu_slots}} GPU</td><td>{{t(e.approval_current&&e.node?.fresh&&!e.node?.draining&&e.node?.management_enabled?'配置就绪':'暂不可执行')}}</td></tr></tbody></table></div>
   <div v-for="execution in item.executions" :key="execution.permit_id" class="section-head"><code>{{execution.model_id}} · {{execution.state}}</code><button class="quiet" :disabled="busy||execution.stop_requested" @click="stop(execution.permit_id)">{{t(execution.stop_requested?'等待原生停止核验':'停止并卸载')}}</button></div>
   <div v-for="task in item.tasks||[]" :key="task.task_id" class="section-head"><code>{{task.executor_id}} · {{task.status}}</code><button class="quiet" :disabled="busy||task.status==='cancelling'" @click="cancelTask(task.task_id)">{{t(task.status==='cancelling'?'等待原生停止核验':'取消')}}</button></div>
  </article>
 </section>
</template>
<style scoped>.integrations{padding:24px;margin-top:20px}.integrations article{border-top:1px solid #dce4ec;margin-top:18px}.table-scroll{overflow:auto}.integrations code{overflow-wrap:anywhere}</style>
