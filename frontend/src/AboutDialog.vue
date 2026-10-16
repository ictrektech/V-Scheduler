<script setup lang="ts">
import {ref} from 'vue'
import {request} from './modelhub/client'
import {apiBases,jsonResponse} from './http-client.js'
defineProps<{t:(text:string)=>string}>()
const dialog=ref<HTMLDialogElement>(),info=ref<any>(null),loading=ref(false),error=ref('')
const labels:Record<string,string>={'scheduler':'Scheduler 与界面','model-management':'模型管理','router':'Router','ollama-qa':'Ollama · 聊天','ollama-embedding':'Ollama · 向量','ollama-rerank':'Ollama · 重排'}
async function open(){dialog.value?.showModal();loading.value=true;error.value='';try{info.value=await request('/api/v1/version')}catch{error.value='无法读取当前镜像版本';try{const response=await fetch(apiBases(location.pathname).scheduler+'setup');const data=await jsonResponse(response);info.value={version:data.data?.version,images:[]}}catch{info.value=null}}finally{loading.value=false}}
</script>
<template>
 <button class="quiet about-trigger" @click="open">{{t('关于')}}</button>
 <dialog ref="dialog" class="about-dialog">
  <div class="about-title"><h2>{{t('关于 V-Scheduler')}}</h2><button class="quiet" :aria-label="t('关闭')" @click="dialog?.close()">×</button></div>
  <p v-if="loading" role="status">{{t('正在读取版本…')}}</p>
  <template v-else><dl class="version-summary"><div><dt>VOS App</dt><dd>{{info?.version?'v'+info.version:t('未知')}}</dd></div><div><dt>{{t('运行配置')}}</dt><dd>{{info?.profile||t('未知')}}</dd></div></dl><p v-if="error" class="error" role="alert">{{t(error)}}</p>
   <div v-for="image in info?.images||[]" :key="image.service" class="image-version"><div><strong>{{t(labels[image.service]||image.service)}}<span v-if="image.components?.length>1"> · Router · {{t('模型仓库')}}</span></strong></div><code>{{image.image?.split('/').pop()||t('未提供镜像信息')}}</code></div>
   <p class="version-note">{{t('镜像信息来自当前安装配置；Scheduler 内置 Router、模型仓库与界面。')}}</p>
  </template>
 </dialog>
</template>
<style scoped>
.about-trigger{white-space:nowrap}.about-dialog{border:1px solid #dce5ef;border-radius:16px;padding:24px;width:min(690px,calc(100vw - 36px));max-height:85vh;overflow:auto;box-sizing:border-box;color:#21354d;background:#fff;box-shadow:0 20px 80px #18293b33}.about-dialog::backdrop{background:#16263c66}.about-title{display:flex;align-items:center;justify-content:space-between;gap:14px}.about-title h2{margin:0;font-size:21px}.about-title button{font-size:24px}.version-summary{display:flex;gap:35px;padding:12px 0}.version-summary dt{font-size:12px;color:#72839a}.version-summary dd{margin:5px 0;font-weight:650}.image-version{padding:13px 0;border-top:1px solid #e6ecf3}.image-version>div{display:flex;justify-content:space-between;gap:16px;flex-wrap:wrap}.image-version span{font-size:12px;color:#476680;background:#f1f5fa;padding:3px 7px;border-radius:6px}.image-version code{display:block;overflow-wrap:anywhere;line-height:1.7;font-size:11px;margin-top:5px;color:#5c7189}.version-note{font-size:12px;color:#72839a;line-height:1.7}
</style>
