<script setup lang="ts">
import {onMounted,onUnmounted,ref,watch} from 'vue'
import Icon from './Icon.vue'
import {duration} from './activity'
import {mergeEvents,waitLabel,waitReasons,type Task,type Attempt,type TaskEvent} from './task-experience'
const props=defineProps<{task:Task,actionBusy:boolean,request:(path:string)=>Promise<any>,demo:boolean,labels:Record<string,string>,t:(text:string)=>string,nodeName:(id:string)=>string,locale:string}>()
const emit=defineEmits<{close:[],cancel:[task:Task],retry:[task:Task],delete:[task:Task]}>()
const detail=ref(props.task),attempts=ref<Attempt[]>([]),events=ref<TaskEvent[]>([]),tab=ref('overview'),loading=ref(false),failed=ref(false),nextCursor=ref<string|null>(null),olderBusy=ref(false),olderFailed=ref(false),now=ref(Date.now())
let alive=true,timer:ReturnType<typeof setInterval>,loadedHistory=false
async function refresh(){
 if(props.demo||loading.value)return
 loading.value=true
 try{
  const [task,history,timeline]=await Promise.all([props.request(`tasks/${props.task.task_id}`),props.request(`tasks/${props.task.task_id}/attempts`),props.request(`tasks/${props.task.task_id}/events?limit=50`)])
  if(!alive)return
  if(task.version<detail.value.version)return
  detail.value=task;attempts.value=history
  // If an entire page arrived between polls, restart from the latest page so
  // the older-page cursor can still reach every intervening event.
  const gap=events.value.length>0&&timeline.items.length>0&&!timeline.items.some((item:TaskEvent)=>events.value.some(old=>old.cursor===item.cursor))
  events.value=gap?timeline.items:mergeEvents(events.value,timeline.items)
  if(!loadedHistory||gap){nextCursor.value=timeline.next_cursor;loadedHistory=true}
  failed.value=false
 }catch{if(alive)failed.value=true}
 finally{if(alive)loading.value=false}
}
async function older(){
 if(!nextCursor.value||olderBusy.value)return
 olderBusy.value=true
 try{const data=await props.request(`tasks/${props.task.task_id}/events?limit=50&cursor=${encodeURIComponent(nextCursor.value)}`);if(alive){events.value=mergeEvents(events.value,data.items);nextCursor.value=data.next_cursor;olderFailed.value=false}}
 catch{if(alive)olderFailed.value=true}finally{if(alive)olderBusy.value=false}
}
function time(value:string){const ms=Date.parse(value);return Number.isFinite(ms)?new Intl.DateTimeFormat(props.locale,{month:'2-digit',day:'2-digit',hour:'2-digit',minute:'2-digit',second:'2-digit'}).format(ms):'—'}
function elapsed(start:string,end?:string|null){const from=Date.parse(start),to=end?Date.parse(end):now.value;return duration(Number.isFinite(from)&&Number.isFinite(to)&&to>=from?to-from:null)}
function occupancy(attempt:Attempt){return attempt.stopped_at?elapsed(attempt.created_at,attempt.stopped_at):['assigned','running','cancelling','recovering','unknown'].includes(attempt.status)?elapsed(attempt.created_at):'—'}
function onKey(event:KeyboardEvent){if(event.key==='Escape')emit('close')}
watch(()=>props.task.version,()=>{if(props.demo||props.task.version>detail.value.version)detail.value=props.task;if(!props.demo)void refresh()})
onMounted(()=>{void refresh();timer=setInterval(()=>{now.value=Date.now();void refresh()},2500);window.addEventListener('keydown',onKey)})
onUnmounted(()=>{alive=false;clearInterval(timer);window.removeEventListener('keydown',onKey)})
</script>
<template>
 <div class="drawer-backdrop" @click.self="emit('close')"><section class="drawer task-details" role="dialog" aria-modal="true" :aria-label="t('任务详情')">
  <div class="drawer-title"><h2>{{detail.app_task_id||detail.task_id.slice(0,8)}}</h2><span class="status" :class="detail.status">{{labels[detail.status]||detail.status}}</span><button class="icon-button" :aria-label="t('关闭')" @click="emit('close')"><Icon name="close"/></button></div>
  <p>{{detail.app_id}} <span class="detail-refresh">{{t('每 2.5 秒更新详情')}}</span></p>
  <p v-if="failed" class="detail-error" role="alert">{{t('详情暂不可用；保留上次数据。')}} <button class="text-button" :disabled="loading" @click="refresh">{{t('重试')}}</button></p>
  <div class="detail-tabs"><button v-for="item in [{key:'overview',label:'概览'},{key:'timeline',label:'状态时间线'},{key:'attempts',label:'执行历史'},{key:'json',label:'请求与结果'}]" :class="{active:tab===item.key}" @click="tab=item.key">{{t(item.label)}}</button></div>
  <div v-if="tab==='overview'">
   <div v-if="['queued','retry_wait'].includes(detail.status)" class="wait-guidance"><h3>{{t('为什么还在等待')}}</h3><article v-for="reason in detail.wait_reasons?.length?detail.wait_reasons:['WAIT_RESOURCE_OR_NODE']" :key="reason"><strong>{{waitLabel(reason,t)}}</strong><p>{{t(waitReasons[reason]?.[1]||'等待调度条件满足；可检查节点状态和未释放预约。')}}</p></article><small>{{t('等待时间不等于预计开始时间。')}}</small></div>
   <dl><dt>{{t('业务任务 ID')}}</dt><dd class="mono">{{detail.app_task_id||'—'}}</dd><dt>{{t('任务 ID')}}</dt><dd class="mono">{{detail.task_id}}</dd><dt>{{t('应用')}}</dt><dd>{{detail.app_id}}</dd><dt>{{t('执行器')}}</dt><dd>{{detail.executor_id||'—'}}</dd><dt>{{t('执行节点')}}</dt><dd>{{detail.node_id?nodeName(detail.node_id):t('待分配')}}</dd><dt>{{t('创建时间')}}</dt><dd>{{time(detail.created_at)}}</dd><dt v-if="['queued','retry_wait'].includes(detail.status)">{{t('提交至今')}}</dt><dd v-if="['queued','retry_wait'].includes(detail.status)">{{elapsed(detail.created_at)}}</dd><dt v-if="attempts.length">{{t('首次排队等待')}}</dt><dd v-if="attempts.length">{{elapsed(detail.created_at,attempts[0].created_at)}}</dd><dt>{{t('执行代次')}}</dt><dd>{{detail.attempt_count}}</dd><dt v-if="detail.progress?.percent!=null">{{t('进度')}}</dt><dd v-if="detail.progress?.percent!=null">{{Math.round(detail.progress.percent*100)}}%</dd><dt v-if="detail.progress?.stage">{{t('阶段')}}</dt><dd v-if="detail.progress?.stage">{{detail.progress.stage}}</dd><dt v-if="detail.progress?.message">{{t('状态说明')}}</dt><dd v-if="detail.progress?.message">{{detail.progress.message}}</dd></dl>
   <div v-if="detail.status==='failed'" class="wait-guidance"><h3>{{t('任务执行失败')}}</h3><p>{{t(detail.can_retry?'可重试；新执行会保留在同一任务的执行历史中。':'此任务当前不可重试；请查看执行历史中的结果和停止状态。')}}</p><pre>{{JSON.stringify(detail.result,null,2)}}</pre></div>
  </div>
  <div v-else-if="tab==='timeline'" class="task-timeline"><p class="detail-note">{{t('按最新事件在前展示已保存的状态与进度快照。')}}</p><article v-for="event in events" :key="event.cursor"><time :datetime="event.created_at">{{time(event.created_at)}}</time><span class="status" :class="event.status">{{labels[event.status]||event.status}}</span><small>{{t('状态版本')}} {{event.version}}</small><p v-if="event.progress?.percent!=null">{{t('进度')}} {{Math.round(event.progress.percent*100)}}%</p><p v-if="event.progress?.stage||event.progress?.message">{{event.progress.stage}} {{event.progress.message}}</p></article><p v-if="!events.length" class="empty">{{t(loading?'正在加载任务详情':'尚无状态事件。')}}</p><p v-if="olderFailed" role="alert" class="detail-error">{{t('较早记录加载失败，请重试。')}}</p><button v-if="nextCursor" class="quiet" :disabled="olderBusy" @click="older">{{t(olderBusy?'正在加载任务详情':'加载更早事件')}}</button></div>
  <div v-else-if="tab==='attempts'" class="attempt-list"><article v-for="attempt in attempts" :key="attempt.attempt_id"><span class="status" :class="attempt.status">{{labels[attempt.status]||attempt.status}}</span><h3>{{t('执行代次')}} {{attempt.fence}}</h3><p class="mono">{{attempt.attempt_id}}</p><dl><dt>{{t('执行节点')}}</dt><dd>{{nodeName(attempt.node_id)}}</dd><dt>{{t('分配时间')}}</dt><dd>{{time(attempt.created_at)}}</dd><dt>{{t('停止时间')}}</dt><dd>{{attempt.stopped_at?time(attempt.stopped_at):t('尚未确认停止')}}</dd><dt>{{t('任务执行占用')}}</dt><dd>{{occupancy(attempt)}}</dd></dl><pre v-if="attempt.result!=null">{{JSON.stringify(attempt.result,null,2)}}</pre></article><p v-if="!attempts.length" class="empty">{{t(loading?'正在加载任务详情':'没有执行记录。')}}</p></div>
  <pre v-else class="code-block">{{JSON.stringify(detail,null,2)}}</pre>
  <div class="drawer-actions"><button v-if="['queued','retry_wait','assigned','running','recovering'].includes(detail.status)" class="quiet" :disabled="actionBusy" @click="emit('cancel',detail)">{{t('取消')}}</button><button v-if="detail.can_retry" :disabled="actionBusy" @click="emit('retry',detail)">{{t('重试')}}</button><button v-if="['succeeded','failed','cancelled'].includes(detail.status)" class="quiet" :disabled="actionBusy" @click="emit('delete',detail)">{{t('删除')}}</button></div>
 </section></div>
</template>
<style scoped>
.task-details{width:520px}
.task-details .drawer-title h2{overflow-wrap:anywhere;min-width:0;flex:1}.detail-refresh{display:block;margin-top:6px;font-size:11px;color:#73869a}.detail-tabs{flex-wrap:wrap}.wait-guidance{background:#f7fafc;border:1px solid #e4ebf1;border-radius:8px;padding:16px;margin:18px 0}.wait-guidance h3{font-size:14px;margin:0 0 12px}.wait-guidance article+article{margin-top:14px}.wait-guidance strong{font-size:13px}.wait-guidance p,.detail-note{font-size:12px;color:#62798d;line-height:1.7}.wait-guidance small{font-size:11px;color:#73869a}.task-details dd{overflow-wrap:anywhere}.task-timeline article{border-left:2px solid #dce8ed;padding:4px 0 18px 18px;margin-left:4px}.task-timeline time{display:block;font-size:11px;color:#73869a;margin-bottom:8px}.task-timeline small{font-size:11px;color:#73869a;margin-left:10px}.task-timeline p{font-size:12px;line-height:1.7;overflow-wrap:anywhere}.task-details pre{overflow:auto;max-height:260px;font-size:11px}.detail-error{color:#a44545;font-size:12px;line-height:1.7;background:#fff5f3;padding:12px;border-radius:7px}.task-details .drawer-actions{position:sticky;bottom:0;background:#fff;padding:16px 0 4px;border-top:1px solid #e4ebf1}
</style>
