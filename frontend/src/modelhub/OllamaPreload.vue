<script setup lang="ts">
defineProps<{preload:{model:string,enabled:boolean,state:string,error:string|null},busy:boolean,t:(message:string)=>string}>()
const emit=defineEmits<{toggle:[enabled:boolean]}>()
const states:Record<string,string>={
 disabled:'关闭',pending:'等待预热',preparing:'正在预热',completed:'预热完成',failed:'预热失败',
}
</script>

<template>
 <div class="preload">
  <label>
   <input type="checkbox" :checked="preload.enabled" :disabled="busy"
     @click.prevent="emit('toggle',!preload.enabled)">
   {{t('自动预热')}}
  </label>
  <code>{{preload.model}}</code>
  <small role="status">{{t(states[preload.state]||preload.state)}}</small>
  <p v-if="preload.error" class="error" role="alert">{{preload.error}}</p>
 </div>
</template>

<style scoped>
.preload{display:flex;align-items:center;gap:8px 12px;flex-wrap:wrap;margin:12px 0}
.preload label{display:inline-flex;align-items:center;gap:7px}
.preload p{width:100%;margin:0}
</style>
