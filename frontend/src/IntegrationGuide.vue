<script setup lang="ts">
defineProps<{t:(value:string)=>string}>()
</script>
<template>
 <article class="integration-guide panel">
  <h2>{{t('接入与迁移')}}</h2>
  <p>{{t('模型客户端继续使用网关 API；调度管理使用独立前缀。')}}</p>
  <table><thead><tr><th>{{t('用途')}}</th><th>{{t('地址')}}</th></tr></thead><tbody>
   <tr><td>{{t('模型调用')}}</td><td><code>http://&lt;vos-host&gt;:22888/v1</code></td></tr>
   <tr><td>{{t('网关管理')}}</td><td><code>http://&lt;vos-host&gt;:22888/api/v1</code></td></tr>
   <tr><td>{{t('调度管理 API')}}</td><td><code>http://&lt;vos-host&gt;:22888/scheduler/api/v1</code></td></tr>
   <tr><td>VOS API</td><td><code>/api/com.ictrek.v-scheduler</code></td></tr>
  </tbody></table>
  <h3>{{t('本地与云端模型')}}</h3>
  <p>{{t('本地和纳管远端模型先取得资源许可；云端供应商模型不占本地资源预约。资源忙时遵守重试提示，不能绕过网关直接启动同一次计算。')}}</p>
  <h3>{{t('保留原 V-Router 数据')}}</h3>
  <p>{{t('先记录旧容器 /data 对应的宿主目录并备份，等待执行停止；卸载旧 V-Router 时保留数据，然后将该目录填入 V_ROUTER_DATA_PATH。新版直接挂载原目录，不需要搬移文件。')}}</p>
  <p>{{t('保留原有发现地址、认证配置和端口。应用升级前核对依赖声明；同一数据目录和端口只能由一个网关使用。')}}</p>
  <h3>{{t('开发文档')}}</h3>
  <p><a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/application-integration.md" target="_blank" rel="noopener">{{t('应用接入指南')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/integration-standard.md" target="_blank" rel="noopener">{{t('公开 API 与接入标准')}}</a> · <a href="https://github.com/ictrektech/V-Scheduler/blob/main/docs/unified-application.md" target="_blank" rel="noopener">{{t('统一应用与迁移')}}</a></p>
 </article>
</template>
<style scoped>.integration-guide{padding:24px;line-height:1.8}.integration-guide h3{margin-top:24px}.integration-guide table{margin:18px 0}.integration-guide a{color:#265cc2}.integration-guide code{overflow-wrap:anywhere}</style>
