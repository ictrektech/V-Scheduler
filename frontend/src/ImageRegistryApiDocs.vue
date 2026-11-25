<script setup lang="ts">
defineProps<{ t: (source: string) => string }>()

const endpoints = [
 { method: 'GET', path: '/health' },
 { method: 'GET', path: '/registry/info' },
 { method: 'GET', path: '/registry/mirrors' },
 { method: 'GET', path: '/network/addresses' },
 { method: 'GET', path: '/registry/images' },
 { method: 'GET', path: '/registry/images/detail', fields: 'repository' },
 { method: 'DELETE', path: '/registry/images', fields: 'repository, tag?' },
 { method: 'POST', path: '/registry/pull', fields: 'image, arch?, insecure?, mirror?, timeout_minutes?, retry_times?' },
 { method: 'POST', path: '/registry/sync', fields: 'repository, tag?, arch?, insecure?, mirror?, timeout_minutes?, retry_times?' },
 { method: 'POST', path: '/registry/import', fields: 'file, repository?, tag?, timeout_minutes?, retry_times?' },
 { method: 'POST', path: '/registry/clone', fields: 'repository, tag?, target_repository?, target_tag?, timeout_minutes?, retry_times?' },
 { method: 'POST', path: '/registry/rename', fields: 'repository, tag?, target_repository?, target_tag?, timeout_minutes?, retry_times?' },
 { method: 'GET', path: '/registry/gc' },
 { method: 'PUT', path: '/registry/gc' },
 { method: 'POST', path: '/registry/gc/run' },
 { method: 'GET', path: '/tasks' },
 { method: 'GET', path: '/tasks/{id}' },
 { method: 'DELETE', path: '/tasks/{id}' },
 { method: 'POST', path: '/tasks/{id}/cancel' },
 { method: 'GET', path: '/settings' },
 { method: 'PUT', path: '/settings/registries' },
 { method: 'DELETE', path: '/settings/registries' },
 { method: 'PUT', path: '/settings/proxy' },
]
</script>

<template>
 <section class="panel registry-api-docs" aria-labelledby="registry-api-title">
  <h2 id="registry-api-title">{{t('镜像仓库 API')}}</h2>
  <p><code>/app/com.ictrek.v-scheduler/api/v1/image-hub/runtime</code></p>
  <p>{{t('使用 VOS 管理员 Bearer Token。镜像管理返回 code、category、data、msg 和 version，code 为 0 表示成功。')}}</p>
  <p>{{t('拉取、同步、归档导入和回收返回 task_id，可查询任务进度。归档导入使用 multipart/form-data，其他写入请求使用 JSON。')}}</p>
  <table>
   <thead><tr><th scope="col">{{t('接口')}}</th><th scope="col">{{t('参数')}}</th></tr></thead>
   <tbody>
    <tr v-for="{method, path, fields} in endpoints" :key="method+' '+path">
     <td><span class="method" :class="method.toLowerCase()">{{method}}</span> <code>{{path}}</code></td>
     <td><code v-if="fields">{{fields}}</code></td>
    </tr>
   </tbody>
  </table>
 </section>
</template>

<style scoped>
.registry-api-docs p {margin-top:.65rem;overflow-wrap:anywhere}
.registry-api-docs table {table-layout:fixed;margin-top:1rem}
.registry-api-docs th,.registry-api-docs td {white-space:normal;overflow-wrap:anywhere}
.registry-api-docs th:first-child {width:44%}
</style>
