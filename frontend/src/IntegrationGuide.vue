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
  <p>{{t('统一容器的正式 host alias 只有 v-scheduler：8080 为网关，38120 为调度，6005 为模型管理。表中其余名称均为兼容别名，指向同一容器；三个 Ollama 是独立容器。新应用和已有应用后续适配均推荐使用 v-scheduler。')}}</p>
  <div class="endpoint-table"><table><thead><tr><th>{{t('用途')}}</th><th>{{t('正式 host alias : 内部端口')}}</th><th>{{t('兼容别名')}}</th><th>{{t('宿主机地址 : 映射端口')}}</th></tr></thead><tbody><tr v-for="endpoint in endpoints" :key="endpoint.name"><td>{{endpoint.name}}</td><td><code>{{endpoint.internal}}{{endpoint.path}}</code></td><td><div v-for="alias in endpoint.aliases||[]" :key="alias"><code>{{alias}}</code></div><div v-for="alias in endpoint.compatibility_aliases||[]" :key="alias">{{t('兼容')}}：<code>{{alias}}</code></div><span v-if="!endpoint.aliases?.length&&!endpoint.compatibility_aliases?.length">—</span></td><td><code>http://{{locationHost}}:{{endpoint.host_port}}{{endpoint.path}}</code></td></tr></tbody></table></div>
  <p>{{t('端口显示当前安装配置。模型管理 API、选定权重文件下载及三个 Ollama 接口的详细说明见「模型管理 API」。')}}</p>
  <p>{{t('正式别名由当前 V-Scheduler 安装声明；兼容别名仅供旧客户端使用。新应用优先使用正式别名；与旧 V-Router 共存时不要向兼容别名发送私有凭据。所有别名均指向现有容器，不新增服务。')}}</p>
  <h3>{{t('本地与云端模型')}}</h3>
  <p>{{t('本地和纳管远端模型先取得资源许可；云端供应商模型不占本地资源预约。资源忙时遵守重试提示，不能绕过网关直接启动同一次计算。')}}</p>
  <h3>{{t('应用和模型自动发现')}}</h3>
  <p>{{t('安装支持的应用后，网关会按发现间隔读取其可用模型与服务。内置三个 Ollama 由本应用直接接入；其他应用支持 Image Studio、V-Voice、V-Music、ML Server 和视频结构化服务。')}}</p>
  <p>{{t('V-Voice 就绪后会主动接入调度器，自动绑定已校准的执行器与模型映射。外部云端服务可在上游页面配置。')}}</p>
  <h3>{{t('推理等待队列')}}</h3>
  <p>{{t('本地推理设置 X-VScheduler-Queue-Wait-Seconds（1..300）、X-VScheduler-Priority（interactive/background）和唯一 X-VScheduler-Request-ID。开启内置驻留且身份已验证时，未设置等待默认等 30 秒；显式 0 保持立即准入。试聊默认等待 120 秒，可查看位置并取消。')}}</p>
  <p><code>GET /api/v1/inference-queue</code> · <code>DELETE /api/v1/inference-queue/{id}</code></p>
  <p>{{t('队列按用户和应用隔离；每等待 30 秒提升一级优先级，不抢占运行中的计算。等待不预约资源，重启不重放请求；总超时应覆盖排队、冷加载和推理。')}}</p>
  <h3>{{t('模型驻留与卸载')}}</h3>
  <p>{{t('在模型仓库的「模型接入状态」设置空闲保留秒数，默认 60，范围 0..600；0 表示每次卸载。驻留保留完整资源预约，同模型可复用；切换或到期先确认卸载再释放。')}}</p>
  <p><code>GET /api/v1/model-residency</code> · <code>PUT /api/v1/model-residency</code> · <code>{"idle_seconds":60}</code></p>
  <h3>{{t('模型接入状态')}}</h3>
  <p>{{t('通过推理服务的 metadata.model_bindings 将后端模型名称关联到仓库模型 ID。状态页统一显示权重、网关路由、后端健康与调度绑定，并说明不可调用原因；配置就绪不代表当前资源一定可用。')}}</p>
  <p><code>GET /api/v1/model-lifecycle</code> · <code>{"metadata":{"model_bindings":{"served-name":"hf://org/repo"}}}</code></p>
  <p>{{t('这些接口属于网关：容器内使用 http://v-scheduler:8080，宿主机使用上表的网关映射端口。驻留设置和完整接入状态需要管理员身份。')}}</p>
  <h3>{{t('开发文档')}}</h3>
  <p><a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/application-integration.md" target="_blank" rel="noopener">{{t('应用接入指南')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/integration-standard.md" target="_blank" rel="noopener">{{t('公开 API 与接入标准')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/unified-application.md" target="_blank" rel="noopener">{{t('统一应用说明')}}</a></p>
 </article>
</template>
<style scoped>.endpoint-table{overflow:auto}.integration-guide{padding:24px;line-height:1.8}.integration-guide h3{margin-top:24px}.integration-guide table{margin:18px 0}.integration-guide a{color:#265cc2}.integration-guide code{overflow-wrap:anywhere}</style>
