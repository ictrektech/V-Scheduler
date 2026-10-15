<script setup lang="ts">
import {computed,onMounted,onUnmounted,ref,watch} from 'vue'
import Icon from './Icon.vue'
import {duration,occupancyMs,permitBudget,type Permit,type Resources,type Statistics,type Timing} from './activity'
const props=defineProps<{request:(path:string)=>Promise<any>,connected:boolean,demo:boolean,t:(text:string)=>string,nodeName:(id:string)=>string,locale:string}>()
const statistics=ref<Statistics|null>(null),permits=ref<Permit[]>([]),state=ref(''),cursor=ref('0'),nextCursor=ref<string|null>(null)
const historyBusy=ref(false),statsBusy=ref(false),historyError=ref(false),statsError=ref(false),sampleAt=ref(Date.now())
let lastStats=0,generation=0
const mockTiming:Timing={samples:24,p50_ms:2800,p95_ms:12000}
const demoStats:Statistics={window:{hours:24,from:new Date(Date.now()-86400000).toISOString(),until:new Date().toISOString()},min_p95_samples:20,first_queue_wait:mockTiming,attempts:{succeeded:20,failed:2,cancelled:2,failure_rate:2/22,occupancy:{samples:24,p50_ms:18000,p95_ms:42000}},permits:{occupancy:{samples:8,p50_ms:4500,p95_ms:null}}}
const demoPermits:Permit[]=[{permit_id:'demo-permit-active',app_id:'v-router',executor_id:'demo.infer',node_id:'demo-gpu-01',state:'active',model_id:'sample-chat',resources:{cpu_millicores:1000,memory_mib:1024,gpu_slots:1,gpu_memory_mib:2048},created_at:new Date(Date.now()-14000).toISOString()},{permit_id:'demo-permit-released',app_id:'v-router',executor_id:'demo.infer',node_id:'demo-gpu-02',state:'released',model_id:'sample-embedding',resources:{cpu_millicores:1000,memory_mib:1024,gpu_slots:1,gpu_memory_mib:1024},evidence:'synchronous_response_completed',created_at:new Date(Date.now()-60000).toISOString(),released_at:new Date(Date.now()-55000).toISOString()}]
const displayedStats=computed(()=>props.demo?demoStats:statistics.value)
const displayedPermits=computed(()=>props.demo?demoPermits.filter(p=>!state.value||p.state===state.value):permits.value)
const canRead=computed(()=>props.connected&&!props.demo)
async function loadHistory(){
 if(!canRead.value||historyBusy.value)return
 const current=generation;historyBusy.value=true
 try{
  const query=new URLSearchParams({limit:'50',cursor:cursor.value});if(state.value)query.set('state',state.value)
  const data=await props.request('permits?'+query)
  if(current===generation){permits.value=data.items;nextCursor.value=data.next_cursor;sampleAt.value=Date.now();historyError.value=false}
 }catch{if(current===generation)historyError.value=true}
 finally{if(current===generation)historyBusy.value=false}
}
async function loadStats(){
 if(!canRead.value||statsBusy.value)return
 const current=generation;statsBusy.value=true
 try{const data=await props.request('statistics');if(current===generation){statistics.value=data;statsError.value=false;lastStats=Date.now()}}
 catch{if(current===generation){statsError.value=true;lastStats=Date.now()}}
 finally{if(current===generation)statsBusy.value=false}
}
function refresh(){void loadHistory();void loadStats()}
function move(value:string){cursor.value=value;permits.value=[];nextCursor.value=null;void loadHistory()}
watch(state,()=>{cursor.value='0';permits.value=[];nextCursor.value=null;void loadHistory()})
watch(()=>[props.connected,props.demo],()=>{generation++;statistics.value=null;permits.value=[];cursor.value='0';nextCursor.value=null;historyBusy.value=false;statsBusy.value=false;historyError.value=false;statsError.value=false;lastStats=0;refresh()},{immediate:true})
let timer:ReturnType<typeof setInterval>
onMounted(()=>{timer=setInterval(()=>{sampleAt.value=Date.now();void loadHistory();if(Date.now()-lastStats>=30000)void loadStats()},2500)})
onUnmounted(()=>{generation++;clearInterval(timer)})
function pretty(value:string){const ms=Date.parse(value);return Number.isFinite(ms)?new Intl.DateTimeFormat(props.locale,{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit'}).format(ms):'—'}
function timingLabel(value?:Timing){return `P50 ${duration(value?.p50_ms)} · P95 ${duration(value?.p95_ms)}`}
function stateLabel(p:Permit){return props.t(p.state==='active'?'许可占用中':p.state==='suspect'?'停止待核验':p.evidence?.startsWith('delegated_task:')?'已移交任务':'已释放')}
function evidence(p:Permit){if(p.state==='active')return props.t('等待执行完成');if(p.state==='suspect')return props.t('尚未确认停止');if(p.evidence?.startsWith('delegated_task:'))return props.t('资源已移交至持久任务');if(p.evidence==='synchronous_response_completed')return props.t('同步响应结束');if(p.evidence==='async_terminal_state_confirmed')return props.t('异步执行已停止');return props.t('已确认停止')}
const budget=permitBudget
</script>
<template>
 <section class="section panel activity-statistics" :aria-busy="statsBusy">
  <div class="section-head"><div><h2>{{t('负载与可靠性')}}</h2><p>{{t('最近 24 小时；每 30 秒更新。')}}</p></div><small v-if="displayedStats">{{t('统计时间')}} {{pretty(displayedStats.window.until)}}</small></div>
  <p v-if="statsError" class="activity-error" role="alert">{{t('统计暂不可用；上次数据可能已过期。')}}</p>
  <div class="activity-metrics">
   <article><span>{{t('首次排队等待')}}</span><strong>{{timingLabel(displayedStats?.first_queue_wait)}}</strong><small>{{displayedStats?.first_queue_wait.samples??'—'}} {{t('个已开始任务')}}</small></article>
   <article><span>{{t('任务执行占用')}}</span><strong>{{timingLabel(displayedStats?.attempts.occupancy)}}</strong><small>{{displayedStats?.attempts.occupancy.samples??'—'}} {{t('次已停止执行')}}</small></article>
   <article><span>{{t('任务执行失败率')}}</span><strong>{{displayedStats?.attempts.failure_rate==null?'—':(displayedStats.attempts.failure_rate*100).toFixed(1)+'%'}}</strong><small>{{t('成功')}} {{displayedStats?.attempts.succeeded??'—'}} · {{t('失败')}} {{displayedStats?.attempts.failed??'—'}} · {{t('取消不计入失败率')}}</small></article>
   <article><span>{{t('模型许可占用')}}</span><strong>{{timingLabel(displayedStats?.permits.occupancy)}}</strong><small>{{displayedStats?.permits.occupancy.samples??'—'}} {{t('条带释放时间的许可')}}</small></article>
  </div>
  <p class="activity-note">{{t('占用时长包含分配与停止核验；首次等待只统计任务第一次分配。P95 至少需要 20 个样本。')}}</p>
 </section>
 <section class="section panel permit-history" :aria-busy="historyBusy">
  <div class="section-head"><div><h2>{{t('模型调用许可')}}</h2><p>{{t('展示模型调用的资源预约与释放记录。释放或移交不代表推理成功。')}}</p></div><button class="quiet" :disabled="historyBusy||statsBusy||demo" @click="refresh"><Icon name="refresh"/>{{t('刷新')}}</button></div>
  <div class="table-toolbar"><select v-model="state" :disabled="historyBusy" :aria-label="t('筛选许可状态')"><option value="">{{t('全部状态')}}</option><option value="active">{{t('许可占用中')}}</option><option value="suspect">{{t('停止待核验')}}</option><option value="released">{{t('已释放')}}</option></select><small>{{t('每页 50 条；每 2.5 秒刷新。')}}</small></div>
  <p v-if="historyError" class="activity-error" role="alert">{{t('许可记录暂不可用；上次数据可能已过期。')}}</p>
  <div class="table-wrap"><table><thead><tr><th>{{t('模型 / 应用')}}</th><th>{{t('执行节点')}}</th><th>{{t('状态')}}</th><th>{{t('预约资源')}}</th><th>{{t('创建时间')}}</th><th>{{t('占用时长')}}</th><th>{{t('释放依据')}}</th></tr></thead><tbody>
   <tr v-for="permit in displayedPermits" :key="permit.permit_id"><td><strong>{{permit.model_id||t('未记录模型')}}</strong><small>{{permit.app_id}}</small><small :title="permit.permit_id">{{permit.permit_id.slice(0,8)}}</small></td><td>{{nodeName(permit.node_id)}}<small v-for="part in permit.placements||[]" :key="part.node_id">+ {{nodeName(part.node_id)}}</small></td><td><span class="status" :class="permit.state">{{stateLabel(permit)}}</span></td><td class="permit-budget"><span>{{(budget(permit).cpu_millicores||0)/1000}} vCPU · {{((budget(permit).memory_mib||0)/1024).toFixed(1)}} GiB RAM</span><small>{{budget(permit).gpu_slots||0}} GPU · {{((budget(permit).gpu_memory_mib||0)/1024).toFixed(1)}} GiB {{t('GPU 内存预算')}}</small><small v-if="permit.placements?.length">{{t('跨节点合计，按各节点独立核算')}}</small><small v-if="permit.parent_task_id">{{t('共享父任务预约')}}</small></td><td><time :datetime="permit.created_at">{{pretty(permit.created_at)}}</time></td><td :title="permit.state==='released'&&!permit.released_at?t('旧记录未保存释放时间'):''">{{duration(occupancyMs(permit,sampleAt))}}</td><td>{{evidence(permit)}}</td></tr>
   <tr v-if="!displayedPermits.length"><td colspan="7" class="empty">{{t(historyBusy?'正在加载许可记录':'没有匹配的许可记录')}}</td></tr>
  </tbody></table></div>
  <div class="permit-pages"><button class="quiet" :disabled="historyBusy||cursor==='0'||demo" @click="move(String(Math.max(0,Number(cursor)-50)))">{{t('上一页')}}</button><span>{{t('第 {page} 页').replace('{page}',String(Math.floor(Number(cursor)/50)+1))}}</span><button class="quiet" :disabled="historyBusy||!nextCursor||demo" @click="move(nextCursor||'0')">{{t('下一页')}}</button></div>
 </section>
</template>
<style scoped>
.section-head p{margin:8px 0 0;font-size:12px;color:#73869a;line-height:1.6}.section-head>small{font-size:11px;color:#73869a}.activity-metrics{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:14px}.activity-metrics article{border:1px solid #e7edf3;background:#f8fafc;border-radius:9px;padding:16px;min-width:0}.activity-metrics article>span{font-size:12px;color:#6b7e91;display:block}.activity-metrics strong{font-size:15px;line-height:1.5;display:block;margin:12px 0;color:#193a50;font-variant-numeric:tabular-nums}.activity-metrics small{font-size:11px;line-height:1.6;color:#73869a}.activity-note{font-size:11px;color:#73869a;line-height:1.7;margin:14px 0 0}.activity-error{color:#a44545;background:#fff5f3;border:1px solid #efd7d3;padding:12px;border-radius:7px;font-size:12px;margin:0 0 15px}.table-toolbar{display:flex;justify-content:space-between;gap:14px;align-items:center}.table-toolbar small{color:#73869a;font-size:11px}.permit-history td{font-size:12px;vertical-align:top;line-height:1.6}.permit-history td>strong{font-size:12px;overflow-wrap:anywhere}.permit-history td>small{display:block;font-size:11px;color:#73869a}.permit-budget{min-width:170px}.permit-budget span{white-space:nowrap}.permit-history time{font-size:11px;white-space:nowrap;font-variant-numeric:tabular-nums}.status.active{color:#147b70;background:#edf9f5}.status.suspect{color:#966618;background:#fff7e7}.status.released{color:#617789;background:#f1f5f8}.permit-pages{display:flex;gap:16px;justify-content:flex-end;align-items:center;margin-top:18px}.permit-pages span{font-size:12px;color:#73869a}.quiet:disabled{opacity:.45;cursor:not-allowed}@media(max-width:1050px){.activity-metrics{grid-template-columns:repeat(2,minmax(0,1fr))}}@media(max-width:540px){.activity-metrics{grid-template-columns:minmax(0,1fr)}.section-head{flex-wrap:wrap;gap:12px}.table-toolbar{align-items:flex-start;flex-direction:column}}
</style>
