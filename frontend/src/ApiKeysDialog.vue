<script setup lang="ts">
import {computed,nextTick,ref,onMounted,onUnmounted} from 'vue'
import {gatewayRequest} from './gateway-client'
import {lanBaseURL} from './http-client.js'
import {isSessionError,recordDiagnostic} from './diagnostics.js'
const props=defineProps<{t:(text:string)=>string}>()
const dialog=ref<HTMLDialogElement>(),keys=ref<any[]>([]),models=ref<any[]>([]),busy=ref(false),error=ref(''),copied=ref(false)
const mode=ref('personal'),name=ref(''),days=ref(0),model=ref(''),supplied=ref(''),confirm=ref('')
const recovering=ref(false),ready=ref(false)
const contextual=ref(false),lanAddress=ref('')
const connection=ref<{model?:string,key:string}|null>(null)
const connectionPanel=ref<HTMLElement>()
const locked=computed(()=>busy.value||recovering.value||!ready.value)
const visibleKeys=computed(()=>keys.value.filter(key=>Array.isArray(key.models)===(mode.value==='model')))
let session=0,retry:ReturnType<typeof setTimeout>|undefined
const current=(id:number)=>id===session&&dialog.value?.open
const request=(path:string,body?:unknown,method?:string)=>gatewayRequest(path,body,method,true)
const ownerUnavailable=(e:unknown)=>e instanceof Error&&e.message.startsWith('API key owner is')
const safeError=(e:unknown)=>{
 const failure=e as {status?:number,message?:string}
 const status=failure.status
 recordDiagnostic(status?`API_KEY_HTTP_${status}`:'API_KEY_OPERATION_FAILED','api_key_management')
 if(status===401&&ownerUnavailable(e))
  return props.t('模型 Key 所属的 VOS 账户不可用。')
 const messages:Record<number,string>={
  403:'当前账户没有管理此 Key 的权限。',
  404:'所选模型或 Key 已不存在，请重新打开面板。',
  409:'当前配置已变更，或所填 Key 已被使用或撤销。',
  422:'Key 设置无效，请检查输入后重试。',
  503:'暂时无法验证 VOS 账户或保存 Key，请稍后重试。',
 }
 return props.t(messages[status||0]||'Key 操作失败，请稍后重试。')
}
function recover(e:unknown,id:number):boolean {
 if(!isSessionError(e)||ownerUnavailable(e))return false
 recovering.value=true
 recordDiagnostic('AUTH_REQUEST_REJECTED','api_key_management')
 clearTimeout(retry)
 retry=setTimeout(()=>{if(current(id))void load(id)},2500)
 return true
}
async function load(id:number,afterWrite=false){
 try{
  const listed=await request('/api/v1/api-keys')
  if(!current(id))return
  const catalog=await request('/api/v1/models')
  if(!current(id))return
  keys.value=listed.keys||[]
  models.value=(catalog.models||[]).filter((m:any)=>m.visibility_manageable)
  recovering.value=false
  ready.value=true
 }catch(e){
  if(current(id)&&!recover(e,id)){
   recovering.value=false
   error.value=afterWrite?props.t('Key 操作已成功，暂时无法刷新列表。'):safeError(e)
  }
 }
}
async function open(target?:string){
 if(dialog.value?.open)return
 dialog.value?.showModal()
 const id=++session
 clearResult();supplied.value='';keys.value=[];models.value=[];confirm.value='';error.value=''
 recovering.value=false;ready.value=false;busy.value=true
 contextual.value=Boolean(target)
 mode.value=target?'model':'personal'
 model.value=target||''
 lanAddress.value=lanBaseURL()
 void request('/api/v1/status').then(status=>{
  if(current(id))lanAddress.value=lanBaseURL(status.base_urls)
 }).catch(()=>{})
 try{await load(id)}finally{if(current(id))busy.value=false}
}
function clearResult(){connection.value=null;copied.value=false}
async function showConnection(key:string,model?:string){
 connection.value={key,model}
 await nextTick()
 connectionPanel.value?.scrollIntoView({block:'nearest'})
 connectionPanel.value?.focus({preventScroll:true})
}
function resetDialog(){
 ++session;clearTimeout(retry)
 clearResult();supplied.value='';confirm.value=''
}
function close(){resetDialog();dialog.value?.close()}
async function run(action:(id:number)=>Promise<void>){
 if(locked.value)return
 const id=session
 busy.value=true;error.value='';clearResult()
 try{await action(id);if(current(id))await load(id,true)}
 catch(e){if(current(id))error.value=recover(e,id)?props.t('连接恢复后可重试，输入已保留。'):safeError(e)}
 finally{if(current(id))busy.value=false}
}
async function create(){await run(async id=>{
 const result=await request('/api/v1/api-keys',{name:name.value,expires_in_days:Number(days.value)})
 if(current(id)){name.value='';await showConnection(result.api_key)}
})}
async function modelKey(value:string|boolean|null){
 await run(async id=>{
  const target=models.value.find(m=>m.upstream_id+'/'+m.id===model.value)
  if(!target)return
  const result=await request('/api/v1/models/access-key',{upstream_id:target.upstream_id,model:target.qualified,access_key:value},'PUT')
  if(!current(id))return
  const key=typeof value==='string'?value:result.model_access_keys?.[0]?.api_key
  supplied.value=''
  if(key)await showConnection(key,target.qualified)
 })
}
async function revoke(keyId:string){await run(async id=>{await request('/api/v1/api-keys/'+encodeURIComponent(keyId),undefined,'DELETE');if(current(id))confirm.value=''})}
function requested(event:Event){void open((event as CustomEvent<string>).detail)}
onMounted(()=>window.addEventListener('vscheduler:model-key',requested))
onUnmounted(()=>{resetDialog();window.removeEventListener('vscheduler:model-key',requested)})
async function copy(value:string){try{await navigator.clipboard.writeText(value);copied.value=true}catch{error.value=props.t('无法自动复制，请选中文本后手动复制。')}}
</script>
<template>
 <button class="quiet" @click="open()">API Key</button>
 <dialog ref="dialog" class="keys-dialog" @cancel.prevent="close">
  <div class="title"><h2>{{t(contextual?(model==='auto/auto'?'Auto 访问 Key':'模型访问 Key'):'API Key 管理')}}</h2><button class="quiet" :aria-label="t('关闭')" @click="close">×</button></div>
  <p>{{t('外部 Agent 使用 Key 调用；VOS 账户鉴权继续可用。Key 只允许推理和查询模型，不允许管理配置。')}}</p>
  <div v-if="!contextual" class="tabs"><button class="quiet" :aria-pressed="mode==='personal'" @click="mode='personal';clearResult();confirm=''">{{t('个人 Key')}}</button><button class="quiet" :aria-pressed="mode==='model'" @click="mode='model';clearResult();confirm=''">{{t('模型专用 Key')}}</button></div>
  <p v-if="error" class="error" role="alert">{{error}}</p>
  <p v-if="recovering" role="status">{{t('正在恢复 VOS 会话，已加载的数据仍可查看。')}}</p>
  <form v-if="mode==='personal'" @submit.prevent="create">
   <p>{{t('个人 Key 的模型权限跟随所属 VOS 用户，账户停用或 Key 撤销后无法再访问私有模型。')}}</p>
   <label>{{t('Key 名称')}}<input v-model="name" required maxlength="80" :placeholder="t('例如：外部 Agent')"></label>
   <label>{{t('有效期')}}<select v-model="days"><option :value="0">{{t('不设到期时间')}}</option><option :value="30">30 {{t('天')}}</option><option :value="90">90 {{t('天')}}</option><option :value="365">365 {{t('天')}}</option></select></label>
   <button type="submit" :disabled="locked||!name.trim()">{{t('生成 Key')}}</button>
  </form>
  <section v-else>
   <p v-if="model==='auto/auto'">{{t('设置 Auto 专用 Key 会关闭匿名 Auto 调用。VOS 登录和个人 Key 仍可调用。Auto 专用 Key 只会路由到公开候选模型。撤销后恢复匿名 Auto 调用。')}}</p>
   <p v-else>{{t('模型专用 Key 仅授权选中的私有模型；public 模型无需 Key，也接受有效或任意填写的 Key。')}}</p>
   <label>{{t('模型')}}<select v-model="model" :disabled="contextual||locked" @change="clearResult();confirm=''"><option value="" disabled>{{t('选择模型')}}</option><option v-for="m in models" :key="m.upstream_id+'/'+m.id" :value="m.upstream_id+'/'+m.id">{{m.qualified}}{{m.effective_visibility?' · '+m.effective_visibility:''}}</option></select></label>
   <div class="actions"><button :disabled="locked||!model" @click="modelKey(true)">{{t('生成并替换模型 Key')}}</button><button class="quiet" :disabled="locked||!model" @click="confirm='model'">{{t('撤销模型 Key')}}</button></div>
   <details><summary>{{t('设置已有 Key')}}</summary><label>{{t('访问 Key（至少 16 个字符）')}}<input v-model="supplied" type="password" autocomplete="off" minlength="16" maxlength="512"></label><button :disabled="locked||!model||supplied.length<16" @click="modelKey(supplied)">{{t('设置并替换')}}</button></details>
   <div v-if="confirm==='model'" class="actions"><span>{{t(model==='auto/auto'?'撤销后将恢复匿名 Auto 调用，确认撤销？':'撤销后该 Key 无法访问私有模型，确认撤销？')}}</span><button :disabled="locked" @click="modelKey(null);confirm=''">{{t('确认撤销')}}</button><button class="quiet" @click="confirm=''">{{t('取消')}}</button></div>
  </section>
  <section v-if="connection" ref="connectionPanel" class="new-secret" aria-labelledby="key-lan-title" tabindex="-1">
   <h3 id="key-lan-title">{{t('局域网客户端配置')}}</h3>
   <p>{{t(connection.model?'在客户端选择 OpenAI 兼容接口，并填写下列三项。':'在客户端选择 OpenAI 兼容接口，填写下列地址和 Key。模型 ID 从该 Key 可见的模型列表中选择。')}}</p>
   <dl class="connection-fields">
    <dt>Base URL</dt><dd><code>{{lanAddress}}</code><button class="quiet" @click="copy(lanAddress)">{{t('复制 Base URL')}}</button></dd>
    <dt>API Key</dt><dd><code>{{connection.key}}</code><button class="quiet" @click="copy(connection.key)">{{t('复制 Key')}}</button></dd>
    <template v-if="connection.model"><dt>{{t('模型 ID')}}</dt><dd><code>{{connection.model}}</code><button class="quiet" @click="copy(connection.model!)">{{t('复制模型 ID')}}</button></dd></template>
   </dl>
   <p>{{t('从远程入口打开时，将 Base URL 的主机名改成客户端可访问的 VOS 局域网地址。')}}</p>
   <p>{{t('请立即复制，关闭后将无法再次查看完整 Key。')}}</p>
   <p v-if="copied" role="status">{{t('已复制')}}</p>
  </section>
  <p>{{t('OpenAI 使用 Authorization: Bearer KEY；Anthropic 使用 x-api-key: KEY。')}}</p>
  <template v-if="!contextual">
  <h3>{{t('我的 Key')}}</h3><p v-if="!visibleKeys.length&&!locked">{{t('还没有 API Key。')}}</p>
  <div v-for="key in visibleKeys" :key="key.id" class="key-row"><div><strong>{{key.name}}</strong><code>{{key.prefix}}…</code><small>{{t(key.active?'可用':'已撤销或已过期')}} · {{key.expires_at?new Date(key.expires_at).toLocaleDateString():t('不设到期时间')}}</small><small v-if="key.models?.length">{{key.models.join(', ')}}</small></div><button v-if="key.active&&confirm!==key.id" class="quiet" :disabled="locked" @click="confirm=key.id">{{t('撤销')}}</button><div v-if="confirm===key.id"><button :disabled="locked" @click="revoke(key.id)">{{t('确认撤销')}}</button><button class="quiet" @click="confirm=''">{{t('取消')}}</button></div></div>
  </template>
 </dialog>
</template>
<style scoped>
.keys-dialog{width:min(760px,calc(100vw - 32px));max-height:85vh;overflow:auto;border:1px solid #dce5ef;border-radius:16px;padding:24px;box-sizing:border-box;color:#21354d;background:white;box-shadow:0 20px 80px #18293b33}.keys-dialog::backdrop{background:#16263c66}.title,.actions,.tabs,.key-row{display:flex;align-items:center;gap:12px;flex-wrap:wrap}.title,.key-row{justify-content:space-between}.title h2{margin:0}.title button{font-size:24px}.keys-dialog p{line-height:1.65;color:#5c7189}.tabs{border-bottom:1px solid #e6ecf3;padding-bottom:12px}.tabs [aria-pressed=true]{background:#e4f3f3;color:#087d86}form{display:flex;gap:12px;align-items:end;flex-wrap:wrap}form p{flex-basis:100%}label{display:flex;flex-direction:column;gap:7px;margin:12px 0}input,select{min-width:180px;max-width:100%;padding:9px;border:1px solid #ccd9e5;border-radius:8px;background:white;color:inherit}form>button{margin-bottom:12px}.new-secret{margin:18px 0;background:#eef8f7;border:1px solid #acd9d4;padding:16px;border-radius:10px}.new-secret code{display:block;overflow-wrap:anywhere;margin:12px 0;user-select:all}.key-row{padding:14px 0;border-top:1px solid #e6ecf3}.key-row code,.key-row small{display:block;margin-top:5px;color:#64778c;overflow-wrap:anywhere}details{margin:16px 0}summary{cursor:pointer}
.connection-fields{grid-template-columns:auto minmax(0,1fr);align-items:center}
.connection-fields dd{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
.connection-fields code{max-width:100%;margin:0}
</style>
