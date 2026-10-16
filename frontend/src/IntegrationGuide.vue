<script setup lang="ts">
import {ref,onMounted} from 'vue'
import {request} from './modelhub/client'
const endpoints=ref<any[]>([])
const locationHost=location.hostname||'<vos-host>'
onMounted(async()=>{try{endpoints.value=(await request('/api/v1/integration')).endpoints}catch{}})
defineProps<{t:(value:string)=>string}>()
</script>
<template>
 <article class="integration-guide panel">
  <h2>{{t('应用接入')}}</h2>
  <p>{{t('模型客户端继续使用网关 API；调度管理使用独立前缀。')}}</p>
  <div class="endpoint-table"><table><thead><tr><th>{{t('用途')}}</th><th>Docker host alias : port</th><th>{{t('宿主机地址 : 映射端口')}}</th></tr></thead><tbody><tr v-for="endpoint in endpoints" :key="endpoint.name"><td>{{endpoint.name}}</td><td><code>{{endpoint.internal}}{{endpoint.path}}</code></td><td><code>http://{{locationHost}}:{{endpoint.host_port}}{{endpoint.path}}</code></td></tr></tbody></table></div>
  <p>{{t('端口显示当前安装配置。模型管理 API、选定权重文件下载及三个 Ollama 接口的详细说明见「模型管理 API」。')}}</p>
  <h3>{{t('本地与云端模型')}}</h3>
  <p>{{t('本地和纳管远端模型先取得资源许可；云端供应商模型不占本地资源预约。资源忙时遵守重试提示，不能绕过网关直接启动同一次计算。')}}</p>
  <h3>{{t('应用和模型自动发现')}}</h3>
  <p>{{t('安装支持的应用后，网关会按发现间隔读取其可用模型与服务。内置三个 Ollama 由本应用直接接入；其他应用支持 Image Studio、V-Voice、V-Music、ML Server 和视频结构化服务。')}}</p>
  <p>{{t('V-Voice 就绪后会主动接入调度器，自动绑定已校准的执行器与模型映射。外部云端服务可在上游页面配置。')}}</p>
  <h3>{{t('开发文档')}}</h3>
  <p><a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/application-integration.md" target="_blank" rel="noopener">{{t('应用接入指南')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/integration-standard.md" target="_blank" rel="noopener">{{t('公开 API 与接入标准')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/unified-application.md" target="_blank" rel="noopener">{{t('统一应用说明')}}</a></p>
 </article>
</template>
<style scoped>.endpoint-table{overflow:auto}.integration-guide{padding:24px;line-height:1.8}.integration-guide h3{margin-top:24px}.integration-guide table{margin:18px 0}.integration-guide a{color:#265cc2}.integration-guide code{overflow-wrap:anywhere}</style>
