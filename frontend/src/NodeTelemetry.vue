<script setup lang="ts">
import {computed,ref,watch} from 'vue'
const props=defineProps<{node:any,demo:boolean,cpu?:number|null,t:(value:string)=>string}>()
const coresExpanded=ref(false)
const history=ref<{cpu:number|null,gpu:number|null}[]>([])
const online=computed(()=>props.demo||!!props.node.heartbeat&&Date.now()-Date.parse(props.node.heartbeat)<12000)
const report=computed(()=>online.value?props.node.report||{}:{})
const gpu=computed<any>(()=>report.value.gpus?.[0])
const cpu=computed(()=>typeof props.cpu==='number'&&Number.isFinite(props.cpu)?props.cpu:null)
const total=computed(()=>report.value.memory_total_mib||props.node.vos_metrics?.data.memory_total_mib||0)
const available=computed(()=>report.value.memory_available_mib)
const used=computed(()=>available.value!=null&&total.value?Math.max(0,total.value-available.value):props.node.vos_metrics?.fresh?props.node.vos_metrics.data.memory_used_mib:null)
const cores=computed(()=>props.node.vos_metrics?.fresh?props.node.vos_metrics.data.cpu_logical_count||report.value.cpu_logical_count:report.value.cpu_logical_count)
const coreUsage=computed<any[]>(()=>Array.isArray(report.value.cpu_cores)?report.value.cpu_cores:[])
const gpuPercent=computed<number|null>(()=>gpu.value?.utilization_percent??null)
const dedicated=computed(()=>gpu.value&&gpu.value.memory_type==='dedicated'&&Number.isFinite(gpu.value.total_mib)&&Number.isFinite(gpu.value.free_mib))
watch(()=>[props.node.report?.observed_at,props.node.vos_metrics?.observed_at],()=>{
 if(!online.value&&!props.node.vos_metrics?.fresh)return
 history.value=[...history.value.slice(-23),{cpu:cpu.value,gpu:gpuPercent.value}]
},{immediate:true})
function percent(value:number|null,totalValue:number){return value==null||!totalValue?null:Math.min(100,Math.max(0,value/totalValue*100))}
function ring(value:number|null){return {'--value':`${value??0}%`} as Record<string,string>}
function points(key:'cpu'|'gpu'){
 return history.value.map((item,index)=>item[key]==null?null:`${index*100/23},${36-(item[key]??0)*.32}`).filter(Boolean).join(' ')
}
function gib(value:number){return (value/1024).toFixed(1)}
function sampleTime(value?:string|null){return value?new Date(value).toLocaleString():'—'}
</script>
<template>
 <div class="resource-observability">
  <div class="resource-gauges">
   <div class="gauge-card"><div class="donut" :style="ring(cpu)"><strong>{{cpu==null?'—':`${cpu.toFixed(0)}%`}}</strong></div><div><span>{{t('CPU 使用率')}}</span><button type="button" class="core-toggle" :aria-expanded="coresExpanded" @click="coresExpanded=!coresExpanded">{{cores||'—'}} {{t('逻辑处理器')}} <span aria-hidden="true">{{coresExpanded?'⌃':'⌄'}}</span></button></div></div>
   <div class="gauge-card"><div class="donut memory" :style="ring(percent(used,total))"><strong>{{percent(used,total)==null?'—':`${percent(used,total)?.toFixed(0)}%`}}</strong></div><div><span>{{t('内存使用率')}}</span><strong>{{used==null?'—':gib(used)}} / {{total?gib(total):'—'}} GiB</strong></div></div>
   <div v-if="gpu" class="gauge-card"><div class="donut gpu" :style="ring(gpuPercent)"><strong>{{gpuPercent==null?'—':`${gpuPercent}%`}}</strong></div><div><span>{{gpu.name||t('GPU 使用率')}}</span><strong>{{gpu.memory_type==='unified'?t('与系统共享内存'):gpu.memory_type==='dedicated'?t('独立显存'):t('内存类型未知')}}</strong></div></div>
   <div v-if="dedicated" class="gauge-card"><div class="donut vram" :style="ring(percent(gpu.total_mib-gpu.free_mib,gpu.total_mib))"><strong>{{percent(gpu.total_mib-gpu.free_mib,gpu.total_mib)?.toFixed(0)}}%</strong></div><div><span>{{t('显存使用率')}}</span><strong>{{gib(gpu.total_mib-gpu.free_mib)}} / {{gib(gpu.total_mib)}} GiB</strong></div></div>
  </div>
  <section v-if="coresExpanded" class="core-panel" :aria-label="t('逐逻辑处理器占用')">
   <p>{{t('来自宿主机的逐逻辑处理器采样')}}</p>
   <div v-if="coreUsage.length" class="core-grid"><article v-for="core in coreUsage" :key="core.id" class="core-cell"><div><span>CPU {{core.id}}</span><strong>{{core.utilization_percent==null?'—':`${core.utilization_percent.toFixed(0)}%`}}</strong></div><div class="core-track" role="meter" :aria-label="`CPU ${core.id}`" :aria-valuenow="core.utilization_percent??undefined" aria-valuemin="0" aria-valuemax="100"><i :style="{width:`${core.utilization_percent??0}%`}"></i></div></article></div>
   <p v-else>{{t('逐核心采样暂不可用')}}</p>
  </section>
  <div class="resource-history"><div><span>{{t('最近一分钟负载')}}</span><small>{{t('每 2.5 秒采样')}}</small></div><svg viewBox="0 0 100 40" preserveAspectRatio="none" role="img" :aria-label="t('CPU 与 GPU 使用率变化')"><path d="M0 36H100" stroke="#e5ecf1" stroke-width=".5"/><polyline v-if="history.some(x=>x.cpu!=null)" :points="points('cpu')" fill="none" stroke="#078a8c" stroke-width="1.6" vector-effect="non-scaling-stroke"/><polyline v-if="history.some(x=>x.gpu!=null)" :points="points('gpu')" fill="none" stroke="#6964db" stroke-width="1.6" vector-effect="non-scaling-stroke"/></svg><div class="legend"><span><i class="cpu-key"></i>CPU</span><span v-if="gpu"><i class="gpu-key"></i>GPU</span></div></div>
  <p v-if="gpu?.memory_type==='unified'" class="resource-note">{{t('GPU 与 CPU 共用系统内存；只按一份内存计算占用。')}}</p>
  <div class="resource-related"><span v-if="node.resident_baseline?.memory_mib||node.resident_baseline?.gpu_memory_mib||node.resident_baseline?.gpu_slots">{{t('常驻基线')}}: {{node.resident_baseline.memory_mib||0}} MiB RAM · {{node.resident_baseline.gpu_memory_mib||0}} MiB GPU · {{node.resident_baseline.gpu_slots||0}} GPU</span><span>{{t('采集心跳')}}: {{sampleTime(node.heartbeat)}}</span><span v-if="node.report?.observed_at">{{t('资源采样')}}: {{sampleTime(node.report.observed_at)}}</span><span v-if="node.vos_metrics">{{t('VOS 采样')}}: {{sampleTime(node.vos_metrics.observed_at)}} · {{t(node.vos_metrics.fresh?'新鲜':'已过期')}}</span><span v-if="node.vos_applications?.fresh">{{t('本机已安装应用')}}: {{node.vos_applications.data.map((a:any)=>a.app_id).join(', ')||'—'}}</span><span v-if="node.executors?.length">{{t('已绑定执行器')}}: {{node.executors.map((e:any)=>e.id).join(', ')}}</span><span v-for="(probe,id) in node.report?.executor_health||{}" :key="id">{{id}}: {{t(probe.healthy?'探测正常':'探测异常')}} · {{sampleTime(probe.observed_at)}}</span></div>
 </div>
</template>
<style scoped>
.resource-observability{grid-column:1/-1;border-top:1px solid #edf0f2;padding-top:18px;min-width:0}.resource-gauges{display:grid;grid-template-columns:repeat(auto-fit,minmax(175px,1fr));gap:12px}.gauge-card{display:flex;align-items:center;gap:12px;background:#f8fafc;border:1px solid #e7edf3;border-radius:9px;padding:12px;min-width:0}.donut{--value:0%;width:66px;height:66px;flex:none;border-radius:50%;display:grid;place-items:center;background:conic-gradient(#078a8c var(--value),#e0e9ef 0);position:relative}.donut:before{content:'';position:absolute;inset:8px;background:#f8fafc;border-radius:50%}.donut strong{position:relative;font-size:14px;color:#193a50}.donut.memory{background:conic-gradient(#27a899 var(--value),#e0e9ef 0)}.donut.gpu{background:conic-gradient(#6964db var(--value),#e0e9ef 0)}.donut.vram{background:conic-gradient(#8a78d9 var(--value),#e0e9ef 0)}.gauge-card>div:last-child{min-width:0}.gauge-card span{display:block;color:#6b7e91;font-size:11px;margin-bottom:6px}.gauge-card>div:last-child strong{font-size:12px;color:#263d53;line-height:1.4}.core-toggle{display:inline-flex;align-items:center;gap:7px;border:0;background:transparent;color:#263d53;font:inherit;font-size:12px;font-weight:650;padding:3px 0;cursor:pointer}.core-toggle span{display:inline!important;margin:0!important;color:#078a8c!important;font-size:14px!important}.core-toggle:focus-visible{outline:2px solid #078a8c;outline-offset:4px;border-radius:3px}.core-panel{margin-top:12px;padding:14px;border:1px solid #e7edf3;border-radius:9px}.core-panel p{font-size:11px;color:#73869a;margin:0 0 12px}.core-panel p:last-child{margin-bottom:0}.core-grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(125px,1fr));gap:10px}.core-cell{padding:9px 10px;background:#f8fafc;border-radius:6px}.core-cell>div:first-child{display:flex;justify-content:space-between;gap:8px;font-size:11px;color:#50657a}.core-cell strong{font-size:11px;font-variant-numeric:tabular-nums}.core-track{height:5px;background:#e0e9ef;border-radius:4px;margin-top:7px;overflow:hidden}.core-track i{display:block;height:100%;background:#078a8c;border-radius:4px}.resource-history{margin-top:13px;padding:12px 14px;border:1px solid #e7edf3;border-radius:9px;display:grid;grid-template-columns:135px minmax(0,1fr) 65px;align-items:center;gap:15px}.resource-history span{font-size:11px;color:#50657a}.resource-history small{display:block;color:#8c9cab;font-size:10px;margin-top:4px}.resource-history svg{width:100%;height:42px}.legend{display:flex;gap:8px;flex-wrap:wrap}.legend span{display:flex;gap:5px;align-items:center;font-size:10px}.legend i{width:7px;height:7px;border-radius:50%;display:block}.cpu-key{background:#078a8c}.gpu-key{background:#6964db}.resource-note{font-size:11px;margin-top:10px}.resource-related{display:flex;flex-wrap:wrap;gap:10px 18px;margin-top:10px;font-size:11px;color:#73869a}.resource-related span{overflow-wrap:anywhere}@media(max-width:700px){.resource-history{grid-template-columns:1fr 55px}.resource-history>div:first-child{grid-column:1/-1}}
</style>
