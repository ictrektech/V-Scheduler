<script setup lang="ts">
import {ref,onMounted,onUnmounted} from 'vue'
import {gatewayRequest} from './gateway-client'
import {recordDiagnostic} from './diagnostics.js'
const props=defineProps<{t:(text:string)=>string}>()
const dialog=ref<HTMLDialogElement>(),keys=ref<any[]>([]),models=ref<any[]>([]),busy=ref(false),error=ref(''),secret=ref(''),copied=ref(false)
const mode=ref('personal'),name=ref(''),days=ref(0),model=ref(''),supplied=ref(''),confirm=ref('')
const safeError=(e:unknown)=>{
 recordDiagnostic('API_KEY_OPERATION_FAILED','api_key_management')
 return props.t('操作失败，请确认已使用 VOS 账户登录后重试。')
}
async function load(){keys.value=(await gatewayRequest('/api/v1/api-keys')).keys||[];models.value=((await gatewayRequest('/api/v1/models')).models||[]).filter((m:any)=>m.visibility_manageable)}
async function open(target?:string){dialog.value?.showModal();secret.value='';supplied.value='';keys.value=[];models.value=[];model.value='';confirm.value='';error.value='';busy.value=true;try{await load();if(target){mode.value='model';model.value=target}}catch(e){error.value=safeError(e)}finally{busy.value=false}}
function close(){secret.value='';supplied.value='';confirm.value='';copied.value=false;dialog.value?.close()}
async function run(action:()=>Promise<void>){busy.value=true;error.value='';secret.value='';copied.value=false;try{await action();await load()}catch(e){error.value=safeError(e)}finally{busy.value=false}}
async function create(){await run(async()=>{const result=await gatewayRequest('/api/v1/api-keys',{name:name.value,expires_in_days:Number(days.value)});secret.value=result.api_key;name.value=''})}
async function modelKey(value:string|boolean|null){await run(async()=>{const target=models.value.find(m=>`${m.upstream_id}/${m.id}`===model.value);if(!target)return;const result=await gatewayRequest('/api/v1/models/access-key',{upstream_id:target.upstream_id,model:target.qualified,access_key:value},'PUT');secret.value=result.model_access_keys?.[0]?.api_key||'';supplied.value=''})}
async function revoke(id:string){await run(async()=>{await gatewayRequest('/api/v1/api-keys/'+encodeURIComponent(id),undefined,'DELETE');confirm.value=''})}
function requested(event:Event){void open((event as CustomEvent<string>).detail)}
onMounted(()=>window.addEventListener('vscheduler:model-key',requested))
onUnmounted(()=>window.removeEventListener('vscheduler:model-key',requested))
async function copy(){try{await navigator.clipboard.writeText(secret.value);copied.value=true}catch{error.value=props.t('无法自动复制，请选择并复制 Key。')}}
</script>
<template>
 <button class="quiet" @click="open()">API Key</button>
 <dialog ref="dialog" class="keys-dialog" @cancel.prevent="close" @close="secret='';supplied=''">
  <div class="title"><h2>{{t('API Key 管理')}}</h2><button class="quiet" :aria-label="t('关闭')" @click="close">×</button></div>
  <p>{{t('外部 Agent 使用 Key 调用；VOS 账户鉴权继续可用。Key 只允许推理和查询模型，不允许管理配置。')}}</p>
  <div class="tabs"><button class="quiet" :aria-pressed="mode==='personal'" @click="mode='personal';secret=''">{{t('个人 Key')}}</button><button class="quiet" :aria-pressed="mode==='model'" @click="mode='model';secret=''">{{t('模型专用 Key')}}</button></div>
  <p v-if="error" class="error" role="alert">{{error}}</p>
  <form v-if="mode==='personal'" @submit.prevent="create">
   <p>{{t('个人 Key 的模型权限跟随所属 VOS 用户，账户停用或 Key 撤销后无法再访问私有模型。')}}</p>
   <label>{{t('Key 名称')}}<input v-model="name" required maxlength="80" :placeholder="t('例如：外部 Agent')"></label>
   <label>{{t('有效期')}}<select v-model="days"><option :value="0">{{t('不设到期时间')}}</option><option :value="30">30 {{t('天')}}</option><option :value="90">90 {{t('天')}}</option><option :value="365">365 {{t('天')}}</option></select></label>
   <button type="submit" :disabled="busy||!name.trim()">{{t('生成 Key')}}</button>
  </form>
  <section v-else>
   <p>{{t('模型专用 Key 仅授权选中的私有模型；public 模型无需 Key，也接受有效或任意填写的 Key。')}}</p>
   <label>{{t('模型')}}<select v-model="model"><option value="" disabled>{{t('选择模型')}}</option><option v-for="m in models" :key="m.upstream_id+'/'+m.id" :value="m.upstream_id+'/'+m.id">{{m.qualified}} · {{m.effective_visibility}}</option></select></label>
   <div class="actions"><button :disabled="busy||!model" @click="modelKey(true)">{{t('生成并替换模型 Key')}}</button><button class="quiet" :disabled="busy||!model" @click="confirm='model'">{{t('撤销模型 Key')}}</button></div>
   <details><summary>{{t('设置已有 Key')}}</summary><label>{{t('访问 Key（至少 16 个字符）')}}<input v-model="supplied" type="password" autocomplete="off" minlength="16" maxlength="512"></label><button :disabled="busy||!model||supplied.length<16" @click="modelKey(supplied)">{{t('设置并替换')}}</button></details>
   <div v-if="confirm==='model'" class="actions"><span>{{t('撤销后该 Key 无法访问私有模型，确认撤销？')}}</span><button :disabled="busy" @click="modelKey(null);confirm=''">{{t('确认撤销')}}</button><button class="quiet" @click="confirm=''">{{t('取消')}}</button></div>
  </section>
  <section v-if="secret" class="new-secret" role="status"><strong>{{t('请立即复制，关闭后将无法再次查看完整 Key。')}}</strong><code>{{secret}}</code><button class="quiet" @click="copy">{{t(copied?'已复制':'复制 Key')}}</button></section>
  <p>{{t('OpenAI 使用 Authorization: Bearer KEY；Anthropic 使用 x-api-key: KEY。')}}</p>
  <h3>{{t('我的 Key')}}</h3><p v-if="!keys.length&&!busy">{{t('还没有 API Key。')}}</p>
  <div v-for="key in keys" :key="key.id" class="key-row"><div><strong>{{key.name}}</strong><code>{{key.prefix}}…</code><small>{{t(key.active?'可用':'已撤销或已过期')}} · {{key.expires_at?new Date(key.expires_at).toLocaleDateString():t('不设到期时间')}}</small><small v-if="key.models?.length">{{key.models.join(', ')}}</small></div><button v-if="key.active&&confirm!==key.id" class="quiet" :disabled="busy" @click="confirm=key.id">{{t('撤销')}}</button><div v-if="confirm===key.id"><button :disabled="busy" @click="revoke(key.id)">{{t('确认撤销')}}</button><button class="quiet" @click="confirm=''">{{t('取消')}}</button></div></div>
 </dialog>
</template>
<style scoped>
.keys-dialog{width:min(760px,calc(100vw - 32px));max-height:85vh;overflow:auto;border:1px solid #dce5ef;border-radius:16px;padding:24px;box-sizing:border-box;color:#21354d;background:white;box-shadow:0 20px 80px #18293b33}.keys-dialog::backdrop{background:#16263c66}.title,.actions,.tabs,.key-row{display:flex;align-items:center;gap:12px;flex-wrap:wrap}.title,.key-row{justify-content:space-between}.title h2{margin:0}.title button{font-size:24px}.keys-dialog p{line-height:1.65;color:#5c7189}.tabs{border-bottom:1px solid #e6ecf3;padding-bottom:12px}.tabs [aria-pressed=true]{background:#e4f3f3;color:#087d86}form{display:flex;gap:12px;align-items:end;flex-wrap:wrap}form p{flex-basis:100%}label{display:flex;flex-direction:column;gap:7px;margin:12px 0}input,select{min-width:180px;max-width:100%;padding:9px;border:1px solid #ccd9e5;border-radius:8px;background:white;color:inherit}form>button{margin-bottom:12px}.new-secret{margin:18px 0;background:#eef8f7;border:1px solid #acd9d4;padding:16px;border-radius:10px}.new-secret code{display:block;overflow-wrap:anywhere;margin:12px 0;user-select:all}.key-row{padding:14px 0;border-top:1px solid #e6ecf3}.key-row code,.key-row small{display:block;margin-top:5px;color:#64778c;overflow-wrap:anywhere}details{margin:16px 0}summary{cursor:pointer}
</style>
