<script setup lang="ts">
import {isSessionError,recordDiagnostic} from './diagnostics.js'
import {isTemporaryReadFailure} from './http-client.js'
import {computed,onMounted,onUnmounted,ref,watch} from 'vue'
import {nodeIdentity} from './node-identity'

type Target = {kind:'ssh'|'cluster',id:string,profile:string}
type Node = {id:string,name:string,hostname?:string,address?:string,host?:string,profile?:string,compatible_profiles?:string[],ssh_node_id?:string}
type DeploymentOptions = {app_id:string,targets:Target[],repositories:string[],automatic?:boolean,images?:Record<string,string>}
type Deployment = {id:string,app_id:string,target_kind:Target['kind'],target_id:string,profile:string,host:string,host_port:number,container_port:number,cpu_millicores?:number,memory_mib?:number,config?:Record<string,unknown>,image:string,state:string,endpoint?:string,last_error?:string}
type Candidate = {node:Node,kind:Target['kind'],profiles:string[]}
const props=defineProps<{request:(path:string,body?:unknown,method?:string)=>Promise<any>,t:(text:string)=>string}>()
const emit=defineEmits<{api:[]}>()
const t=props.t
const inventory=ref<Candidate[]>([])
const options=ref<DeploymentOptions[]>([])
const deployments=ref<Deployment[]>([])
const busy=ref(false)
const ready=ref(false)
const error=ref('')
let timer:number|undefined
const deploymentForm=ref<{app_id:string,request_key:string,targetKey:string,image:string,container_port:number,host_port:number|'',cpu_millicores:number|'',memory_mib:number|'',configText:string}|null>(null)
const states:Record<string,string>={pending:'等待部署',running:'运行中',failed:'部署失败',stopped:'已停止',stopping:'正在停止',deleting:'正在删除',removed:'已删除',cleanup_failed:'清理未确认'}
const failureLabels:Record<string,string>={
 BACKEND_DEPLOYMENT_FORBIDDEN:'当前应用身份或镜像不符合部署要求。',
 BACKEND_IMAGE_NOT_PUBLISHED:'应用没有为此节点的硬件 Profile 发布匹配镜像。',
 BACKEND_IMAGE_CATALOG_INVALID:'应用的发布镜像清单格式无效，请检查已安装的软件包。',
 BACKEND_TARGET_UNAVAILABLE:'目标节点未登记、地址已更改或暂不可读。',
 BACKEND_PROFILE_UNSUPPORTED:'目标节点与所选 Profile 不兼容。',
 BACKEND_PORT_IN_USE:'宿主机端口已被占用，请选择其他端口。',
 BACKEND_NO_PORTS:'该节点的部署端口已全部分配。',
 BACKEND_IMAGE_PULL_FAILED:'镜像拉取失败，请检查目标节点的仓库访问权限和镜像地址。',
 BACKEND_DOCKER_ACCESS_DENIED:'SSH 已连接，但登录账户无法访问 Docker。',
 BACKEND_SSH_AUTH_REJECTED:'SSH 拒绝了已保存的凭据，请在节点管理中更新。',
 BACKEND_SSH_UNREACHABLE:'无法建立到目标节点的 SSH 连接。',
 BACKEND_CLUSTER_PEER_UNAVAILABLE:'无法访问 VOS 集群对端，请检查入会状态和 HTTPS 信任。',
 BACKEND_CLUSTER_PROJECT_FAILED:'VOS Projects 部署失败，请查看目标节点的项目任务。',
 BACKEND_RUNTIME_UNAVAILABLE:'暂时无法读取后端运行状态。',
 BACKEND_CLEANUP_FAILED:'目标上未确认清理，请检查连接后重试删除。',
 BACKEND_START_FAILED:'容器启动失败，请检查镜像入口、运行时和端口。',
 BACKEND_BUSY:'正在处理上一次操作，请稍后重试。',
}
function failure(code?:string){return code&&failureLabels[code]?t(failureLabels[code]):code||''}
const kindLabel=(kind:Target['kind'])=>t(kind==='ssh'?'SSH 节点':'VOS 集群成员')
const key=(target:Target)=>JSON.stringify(target)
const node=(kind:Target['kind'],id:string)=>inventory.value.find(row=>row.kind===kind&&row.node.id===id)?.node
const targetLabel=(target:Target)=> {
 const identity=nodeIdentity(node(target.kind,target.id))
 return [kindLabel(target.kind),identity.name||target.id,identity.address,target.profile].filter(Boolean).join(' · ')
}
const availableApps=computed(()=>options.value.filter(item=>item.targets.length&&item.repositories.length))
const currentApp=computed(()=>options.value.find(item=>item.app_id===deploymentForm.value?.app_id))
const currentTarget=computed(()=>currentApp.value?.targets.find(target=>key(target)===deploymentForm.value?.targetKey))
const selectedImage=computed(()=>{
 const target=currentTarget.value
 return currentApp.value?.automatic
  ? (target ? currentApp.value.images?.[target.profile] : undefined)||''
  : deploymentForm.value?.image.trim()||''
})
const filteredDeployments=computed(()=>deployments.value.filter(item=>item.state!=='removed'))

async function run(fn:()=>Promise<void>, userAction=false) {
 if(busy.value)return
 busy.value=true
 error.value=''
 try {await fn()} catch(err) {
  if(isSessionError(err))recordDiagnostic('AUTH_REQUEST_REJECTED','application_backends')
  if(isSessionError(err)||(!userAction&&isTemporaryReadFailure(err))){
   if(userAction)error.value=t('连接恢复后可重试，输入已保留。')
  }else error.value=failure((err as {code?:string})?.code)||(err instanceof Error?err.message:t('无法读取应用后端'))
 }
 finally {busy.value=false}
}
async function loadDeployments() {
 deployments.value=(await props.request('backend-deployments')).items
}
async function load() {
 const [nodes,profiles,deployable]=await Promise.all([
  props.request('managed-nodes'),props.request('compute-profiles'),props.request('backend-deployment-access'),loadDeployments(),
 ])
 const ssh:Node[]=nodes.items
 const registeredProfiles:string[]=profiles.items.map((item:{name:string})=>item.name)
 inventory.value=[
  ...ssh.map(node=>({node,kind:'ssh' as const,profiles:node.compatible_profiles||[]})),
  ...(nodes.cluster_nodes||[]).map((node:Node)=>({
   node,kind:'cluster' as const,
   profiles:ssh.find(peer=>peer.id===node.ssh_node_id)?.compatible_profiles||registeredProfiles,
  })),
 ]
 options.value=deployable.items
 ready.value=true
}
function deploy() {
 deploymentForm.value={app_id:availableApps.value[0]?.app_id||'',request_key:crypto.randomUUID?.()||('backend-'+Date.now()),targetKey:'',image:'',container_port:8080,host_port:'',cpu_millicores:'',memory_mib:'',configText:'{}'}
}
watch(()=>deploymentForm.value?.app_id,()=>{
 const form=deploymentForm.value
 const target=currentApp.value?.targets[0]
 if(form)form.targetKey=target?key(target):''
})
async function submit() {
 if(!deploymentForm.value)return
 const form=deploymentForm.value
 const target=currentTarget.value
 const image=selectedImage.value
 if(!target||!image)return
 let config:unknown
 try {config=JSON.parse(form.configText||'{}')} catch {throw new Error(t('后端配置必须是 JSON 对象。'))}
 if(!config||typeof config!=='object'||Array.isArray(config))throw new Error(t('后端配置必须是 JSON 对象。'))
 await props.request('backend-deployments',{
  app_id:form.app_id,request_key:form.request_key,target,image,container_port:form.container_port,
  host_port:form.host_port||undefined,cpu_millicores:form.cpu_millicores||undefined,memory_mib:form.memory_mib||undefined,config,
 })
 deploymentForm.value=null
 await loadDeployments()
}
async function action(item:Deployment,method:'start'|'stop'|'remove') {
 if(method==='remove'&&!window.confirm(t('删除此部署并释放端口？')))return
 const path='backend-deployments/'+encodeURIComponent(item.id)
 if(method==='remove')await props.request(path,undefined,'DELETE')
 else await props.request(path+'/'+method,undefined,'POST')
 await loadDeployments()
}
onMounted(()=>{
 void run(load)
 timer=window.setInterval(()=>{if(!busy.value){if(ready.value)void loadDeployments().catch(()=>{});else void run(load)}},4500)
})
onUnmounted(()=>window.clearInterval(timer))
</script>

<template>
 <div class="application-backends">
  <div v-if="error" role="alert" class="error">{{error}}</div>
  <section class="section panel">
   <div class="section-head">
    <h2>{{t('可用节点与 Profile')}}</h2>
    <button class="quiet" type="button" @click="emit('api')">{{t('API 文档')}}</button>
   </div>
   <div v-if="inventory.length" class="backend-targets">
    <article v-for="item in inventory" :key="item.kind+item.node.id">
     <strong>{{nodeIdentity(item.node).name || item.node.id}}</strong>
     <small>{{[nodeIdentity(item.node).address,kindLabel(item.kind)].filter(Boolean).join(' · ')}}</small>
     <div class="profile-tags"><code v-for="profile in (item.node.compatible_profiles||item.profiles)" :key="profile">{{profile}}</code>
      <span v-if="!item.node.profile&&item.kind==='cluster'">{{t('尚未识别硬件 Profile')}}</span>
     </div>
    </article>
   </div>
   <p v-else-if="ready" class="empty">{{t('暂无已登记的部署节点。')}}</p>
  </section>
  <section class="section panel">
   <div class="section-head">
    <h2>{{t('后端部署')}}</h2>
    <button class="quiet" type="button" :disabled="busy" @click="run(load)">{{t('刷新')}}</button>
    <button type="button" :disabled="!availableApps.length||busy" @click="deploy()">{{t('部署后端')}}</button>
   </div>
   <form v-if="deploymentForm" class="backend-form" @submit.prevent="run(submit,true)">
    <label>{{t('应用 ID')}}<select v-model="deploymentForm.app_id" required><option v-for="app in availableApps" :key="app.app_id" :value="app.app_id">{{app.app_id}}</option></select></label>
    <label>{{t('目标节点与 Profile')}}<select v-model="deploymentForm.targetKey" required><option v-for="target in currentApp?.targets" :key="key(target)" :value="key(target)">{{targetLabel(target)}}</option></select></label>
    <label v-if="currentApp?.automatic">{{t('镜像地址')}}<input :value="selectedImage" readonly></label>
    <label v-else>{{t('镜像地址')}}<input v-model.trim="deploymentForm.image" required maxlength="512" placeholder="registry.example.com/team/backend:jetson-thor">
     <small v-if="currentApp">{{currentApp.repositories.join(' · ')}}</small></label>
    <label>{{t('容器端口')}}<input v-model.number="deploymentForm.container_port" type="number" min="1" max="65535" required></label>
    <div class="backend-limits">
     <label>{{t('宿主机端口')}}<input v-model.number="deploymentForm.host_port" type="number" min="42000" max="42999" :placeholder="t('自动分配')"></label>
     <label>{{t('CPU 限制（mCPU）')}}<input v-model.number="deploymentForm.cpu_millicores" type="number" min="100" max="128000" :placeholder="t('不限')"></label>
     <label>{{t('内存限制（MiB）')}}<input v-model.number="deploymentForm.memory_mib" type="number" min="16" max="1048576" :placeholder="t('不限')"></label>
    </div>
    <label>{{t('后端 JSON 配置')}}<textarea v-model="deploymentForm.configText" rows="3" spellcheck="false"/><small>{{t('应用镜像可从 V_SCHEDULER_BACKEND_CONFIG_B64 读取配置。')}}</small></label>
    <div class="buttons"><button class="quiet" type="button" @click="deploymentForm=null">{{t('取消')}}</button><button :disabled="busy||!currentTarget||!selectedImage">{{t('部署后端')}}</button></div>
   </form>
   <div v-if="filteredDeployments.length" class="backend-records">
    <article v-for="item in filteredDeployments" :key="item.id">
     <div>
      <strong>{{item.app_id}}</strong>
      <small>{{targetLabel({kind:item.target_kind,id:item.target_id,profile:item.profile})}}</small>
      <small><code>{{item.image}}</code></small>
      <a v-if="item.endpoint" :href="item.endpoint" target="_blank" rel="noopener">{{item.endpoint}}</a>
      <small v-else>{{item.host}}:{{item.host_port}}</small>
      <small :class="{ 'deployment-running': item.state==='running' }">{{t(states[item.state]||item.state)}}</small>
      <small v-if="item.last_error">{{failure(item.last_error)}}</small>
      <details class="backend-parameters">
       <summary>{{t('部署参数')}}</summary>
       <small>{{t('容器端口')}} {{item.container_port}} · {{t('宿主机端口')}} {{item.host_port}}</small>
       <small>CPU {{item.cpu_millicores ? item.cpu_millicores+' mCPU' : t('不限')}} · {{t('内存')}} {{item.memory_mib ? item.memory_mib+' MiB' : t('不限')}}</small>
       <pre v-if="item.config&&Object.keys(item.config).length">{{JSON.stringify(item.config,null,2)}}</pre>
      </details>
     </div>
     <div class="buttons">
      <button v-if="item.state==='failed'||item.state==='stopped'" class="quiet" :disabled="busy" @click="run(()=>action(item,'start'),true)">{{t('启动后端')}}</button>
      <button v-if="item.state==='running'||item.state==='pending'" class="quiet" :disabled="busy" @click="run(()=>action(item,'stop'),true)">{{t('停止')}}</button>
      <button v-if="item.state!=='deleting'" class="quiet" :disabled="busy" @click="run(()=>action(item,'remove'),true)">{{t('删除')}}</button>
     </div>
    </article>
   </div>
   <p v-else-if="ready&&!deploymentForm" class="empty">{{t('暂无应用后端部署。')}}</p>
  </section>
 </div>
</template>

<style scoped>
.application-backends {display:grid;gap:18px}
.backend-targets {display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,215px),1fr));gap:12px}
.backend-targets>article {padding:12px;border:1px solid #e6edf3;border-radius:8px;min-width:0}
.backend-targets small,.backend-records small,.backend-form small {display:block;margin-top:4px;color:#586e82;overflow-wrap:anywhere}
.profile-tags,.buttons {display:flex;align-items:center;flex-wrap:wrap;gap:8px;margin-top:6px}
.profile-tags code {padding:3px 7px;background:#eff5f6;border-radius:6px;color:#13616a}
.backend-records article {display:flex;justify-content:space-between;align-items:flex-start;gap:16px;flex-wrap:wrap;padding:12px 0;border-top:1px solid #e6edf3}
.backend-records article>div:first-child {min-width:0}
.backend-form {display:grid;gap:12px;padding:14px;margin:10px 0;border:1px solid #e6edf3;border-radius:8px}
.backend-form>label,.backend-limits>label {display:grid;gap:5px}
.backend-limits {display:grid;grid-template-columns:repeat(auto-fit,minmax(170px,1fr));gap:12px}
.backend-parameters {margin-top:8px}
.backend-parameters pre {max-width:100%;overflow:auto}
.backend-form input:not([type=checkbox]),.backend-form textarea,.backend-form select {width:100%}
.backend-form fieldset {min-width:0;border:0;padding:0;margin:0}
.backend-form legend {margin:0 0 8px}
.deployment-running {color:#087866!important}
</style>
