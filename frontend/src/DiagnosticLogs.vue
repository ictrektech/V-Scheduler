<script setup lang="ts">
import {ref,onMounted,onUnmounted} from 'vue'
import {subscribeDiagnostics,clearDiagnostics,diagnosticSnapshot} from './diagnostics.js'
const props=defineProps<{t:(value:string)=>string;locale:string}>()
const rows=ref(diagnosticSnapshot()),copied=ref(false)
let dispose:undefined|(()=>void)
onMounted(()=>{dispose=subscribeDiagnostics(value=>{rows.value=value})})
onUnmounted(()=>dispose?.())
async function copy(){try{await navigator.clipboard.writeText(JSON.stringify(rows.value,null,2));copied.value=true;setTimeout(()=>copied.value=false,2000)}catch{copied.value=false}}
const names:Record<string,string>={VOS_JWT_EXPIRED:'平台登录凭据已过期，自动恢复',VOS_AUTH_REFRESHED:'应用登录凭据已自动刷新',VOS_AUTH_RECOVERY_FAILED:'认证恢复暂未完成，将自动重试',VOS_OIDC_STATE_MISMATCH:'授权状态不匹配，已拒绝使用该凭据',AUTH_REQUEST_RETRY:'认证请求正在自动重试',AUTH_REQUEST_REJECTED:'认证尚未恢复，已保留页面内容',VOS_PLATFORM_SESSION_CHANGED:'平台登录状态变化，已更新应用身份'}
</script>
<template><section class="panel diagnostic-logs"><div class="section-head"><h2>{{t('诊断日志')}}</h2><div><button class="quiet" @click="copy">{{t(copied?'已复制':'复制日志')}}</button><button class="quiet" @click="clearDiagnostics">{{t('清空日志')}}</button></div></div><p>{{t('记录当前浏览器会话的认证恢复和请求重试，仅保存时间、阶段和安全错误码，不记录 token、密钥或原始响应。')}}</p><p v-if="!rows.length" class="empty">{{t('暂无诊断事件')}}</p><div v-else class="diagnostic-scroll"><table><thead><tr><th>{{t('时间')}}</th><th>{{t('事件')}}</th><th>{{t('阶段')}}</th><th>{{t('次数')}}</th></tr></thead><tbody><tr v-for="(row,index) in [...rows].reverse()" :key="row.at+index"><td>{{new Date(row.at).toLocaleString(locale)}}</td><td>{{t(names[row.code]||row.code)}}<small>{{row.code}}</small></td><td><code>{{row.stage}}</code></td><td>{{row.count}}</td></tr></tbody></table></div></section></template>
<style scoped>.diagnostic-logs{padding:24px}.diagnostic-logs p{color:#64748b;line-height:24px}.diagnostic-logs .section-head{display:flex;justify-content:space-between;gap:16px;align-items:center}.diagnostic-logs .section-head>div{display:flex;gap:8px}.diagnostic-scroll{overflow:auto}.diagnostic-logs table{width:100%;border-collapse:collapse;font-size:14px}.diagnostic-logs th,.diagnostic-logs td{text-align:left;padding:12px;border-bottom:1px solid #e0e8ef}.diagnostic-logs small{display:block;font-size:12px;color:#64748b;margin-top:4px}@media(max-width:640px){.diagnostic-logs .section-head{align-items:flex-start;flex-direction:column}}</style>
