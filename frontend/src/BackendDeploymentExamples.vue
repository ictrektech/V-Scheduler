<script setup lang="ts">
const {t}=defineProps<{t:(text:string)=>string}>()
const repository='registry.example.com/team/backend'
const images=[
 ['jetson-orin','linux/arm64','jetson-orin'],
 ['jetson-thor','linux/arm64','jetson-thor'],
 ['dgx-spark','linux/arm64','dgx-spark'],
 ['arm-cpu','linux/arm64','arm-cpu'],
 ['amd-cuda','linux/amd64','amd-cuda'],
 ['amd-cpu','linux/amd64','amd-cpu'],
]
const targets=[
 {kind:'ssh',id:'node-orin',profile:'jetson-orin'},
 {kind:'cluster',id:'member-thor',profile:'jetson-thor'},
]
const imageCatalog=Object.fromEntries(images.map(([profile,,tag])=>[profile,repository+':'+tag+'-v1']))
const common={container_port:8080,cpu_millicores:1500,memory_mib:2048,config:{workers:2,batch_size:4}}
const ssh={request_key:'image-worker-orin',target:targets[0],image:repository+':jetson-orin-v1',...common}
const cluster={request_key:'image-worker-thor',target:targets[1],image:repository+':jetson-thor-v1',host_port:42080,...common}
const json=(value:unknown)=>JSON.stringify(value,null,2)
</script>

<template>
 <section class="section panel backend-examples">
  <h2>{{t('应用后端镜像准备和部署示例')}}</h2>
  <p>{{t('应用为不同 Profile 发布自己的镜像标签，按目标节点架构和加速器构建。后端在容器内监听 0.0.0.0:8080；使用不同端口时，在部署请求里填写实际端口。')}}</p>
  <div class="example-table">
   <table><thead><tr><th>Profile</th><th>{{t('镜像平台')}}</th><th>{{t('示例镜像')}}</th></tr></thead>
    <tbody><tr v-for="[profile,platform,tag] in images" :key="profile"><td><code>{{profile}}</code></td><td><code>{{platform}}</code></td><td><code>{{repository}}:{{tag}}-v1</code></td></tr></tbody>
   </table>
  </div>
  <p>{{t('已登记的计算节点对所有已认证的 VOS 应用可见。应用在已安装的 Compose 中声明各 Profile 的发布镜像，部署时直接选择匹配的节点。')}}</p>
  <details><summary>{{t('应用发布镜像清单示例')}}</summary><pre class="code-block">x-v-scheduler-backend-images: {{JSON.stringify(imageCatalog)}}</pre></details>
  <p>{{t('应用向 POST /backend-deployments 发送所选镜像和参数，轮询 GET /backend-deployments/{id}。状态为 running 时使用 endpoint。集群路径需要已登记的受信 VOS 集群成员，SSH 路径需要已保存的 SSH 连接。')}}</p>
  <div class="example-requests">
   <details><summary>{{t('部署到 SSH 节点')}}</summary><pre class="code-block">{{json(ssh)}}</pre></details>
   <details><summary>{{t('部署到其他 VOS 集群成员')}}</summary><pre class="code-block">{{json(cluster)}}</pre></details>
  </div>
  <p>{{t('后端镜像把 V_SCHEDULER_BACKEND_CONFIG_B64 解码为 UTF-8 JSON，再读取 workers、batch_size 等自己的配置。CPU 和内存字段限制容器使用量。应用凭据通过各自的机密配置渠道管理。')}}</p>
 </section>
</template>

<style scoped>
.backend-examples {margin-top:18px}
.example-table {overflow:auto}
.example-table table {width:100%;border-collapse:collapse}
.example-table th,.example-table td {text-align:left;padding:8px;border-bottom:1px solid #e6edf3}
.example-requests {display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,260px),1fr));gap:12px}
details {padding:8px 0}
summary {color:#135767;cursor:pointer}
.code-block {overflow:auto}
</style>
