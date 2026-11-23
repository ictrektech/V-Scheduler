<script setup lang="ts">
import {computed,onMounted,onUnmounted,ref} from 'vue'

type Node = {
  id: string
  name: string
  host?: string
  ssh_port?: number
  ssh_user?: string
  auth_method?: 'password' | 'key'
  profile?: string
  profile_override?: string
  status: string
  arch: string
  gpu_model: string
  last_checked_at?: string
}
const props = defineProps<{
  request: (path: string, body?: unknown, method?: string) => Promise<any>
  locale: string
}>()
const zh = computed(() => props.locale.startsWith('zh'))
const label = (a: string, b: string) => zh.value ? a : b
const nodes = ref<Node[]>([])
const canManage = ref(false)
const busy = ref(false)
const error = ref('')
const feedback = ref('')
const edit = ref<Node | null>(null)
const form = ref<ReturnType<typeof fields> | null>(null)
const base = 'managed-nodes'
const profiles = ['jetson-thor', 'dgx-spark', 'jetson-orin', 'arm-cpu', 'amd-cpu', 'amd-cuda', 'arm-cuda', 'sophon-bm1688']
let timer: ReturnType<typeof setInterval>

function fields(node?: Node) {
  return {
    name: node?.name || '', host: node?.host || '', ssh_port: node?.ssh_port || 22,
    ssh_user: node?.ssh_user || '', auth_method: node?.auth_method || 'key',
    profile: node?.profile_override || 'auto',
    ssh_password: '', ssh_key: '', sudo_password: '',
  }
}
function describe(reason: unknown) {
  const code = (reason as {code?: string})?.code
  const errors: Record<string, [string,string]> = {
    NODE_SSH_FAILED: ['SSH 连接失败，请检查连接配置。', 'SSH connection failed. Check the connection settings.'],
    NODE_IMAGE_PULL_FAILED: ['镜像拉取失败，请检查节点到镜像仓库的连接。', 'Could not pull the image. Check registry access from the node.'],
    NODE_CONTAINER_START_FAILED: ['测试容器未成功启动。', 'The test container did not start successfully.'],
    NODE_MANAGER_UNAVAILABLE: ['节点服务暂不可用。', 'The node manager is temporarily unavailable.'],
  }
  const message = code && errors[code]
  return message ? label(...message) : label('节点操作失败。', 'Node operation failed.')
}
async function refresh() {
  try {
    const response = await props.request(base)
    nodes.value = response.items
    canManage.value = response.can_manage
    if (!canManage.value) form.value = null
  } catch (reason) {
    nodes.value = []
    canManage.value = false
    if ((reason as {code?: string})?.code !== 'NODE_MANAGER_DISABLED') error.value = describe(reason)
  }
}
function open(node?: Node) {
  edit.value = node || null
  form.value = fields(node)
}
async function perform(action: () => Promise<void>) {
  if (busy.value) return
  busy.value = true
  error.value = ''
  feedback.value = ''
  try { await action() } catch (reason) { error.value = describe(reason) }
  finally { await refresh(); busy.value = false }
}
function save() {
  const values = form.value
  if (!values) return
  const {ssh_password, ssh_key, sudo_password, ...config} = values
  const body = {
    ...config,
    ...(config.auth_method === 'password' && ssh_password ? {ssh_password} : {}),
    ...(config.auth_method === 'key' && ssh_key ? {ssh_key} : {}),
    ...(sudo_password ? {sudo_password} : {}),
  }
  const id = edit.value?.id
  void perform(async () => {
    await props.request(id ? `${base}/${encodeURIComponent(id)}` : base, body, id ? 'PUT' : 'POST')
    form.value = null
  })
}
function check(node: Node, action: 'rescan' | 'runtime') {
  void perform(async () => {
    const result = await props.request(`${base}/${encodeURIComponent(node.id)}/${action}`, {})
    if (action === 'runtime') {
      feedback.value = `${label('镜像拉取和容器启动成功', 'Image pull and container start succeeded')} · ${result.image}`
    }
  })
}
function remove(node: Node) {
  if (!window.confirm(label(`移除节点「${node.name}」的连接配置？`, `Remove the saved connection for ${node.name}?`))) return
  void perform(async () => { await props.request(`${base}/${encodeURIComponent(node.id)}`, undefined, 'DELETE') })
}
onMounted(() => { void refresh(); timer = setInterval(() => { if (!busy.value) void refresh() }, 10000) })
onUnmounted(() => clearInterval(timer))
</script>

<template>
  <section class="section panel compute-nodes">
    <div class="section-head">
      <h2>{{label('SSH 算力节点', 'SSH compute nodes')}}</h2>
      <button v-if="canManage" :disabled="busy" @click="open()">{{label('添加节点', 'Add node')}}</button>
    </div>
    <p v-if="error" class="error" role="alert">{{error}}</p>
    <p v-if="feedback" role="status">{{feedback}}</p>
    <form v-if="form && canManage" class="node-form" @submit.prevent="save">
      <label>{{label('节点名称', 'Node name')}}<input v-model.trim="form.name" required maxlength="128"></label>
      <label>{{label('主机地址', 'Host')}}<input v-model.trim="form.host" required maxlength="255"></label>
      <label>{{label('SSH 端口', 'SSH port')}}<input v-model.number="form.ssh_port" type="number" min="1" max="65535" required></label>
      <label>{{label('SSH 用户', 'SSH user')}}<input v-model.trim="form.ssh_user" required maxlength="64"></label>
      <label>{{label('认证方式', 'Authentication')}}<select v-model="form.auth_method"><option value="key">{{label('私钥', 'Private key')}}</option><option value="password">{{label('密码', 'Password')}}</option></select></label>
      <label>Profile<select v-model="form.profile"><option value="auto">{{label('自动识别', 'Auto-detect')}}</option><option v-for="profile in profiles" :key="profile" :value="profile">{{profile}}</option></select></label>
      <label v-if="form.auth_method==='password'">{{label('SSH 密码', 'SSH password')}}<input v-model="form.ssh_password" type="password" autocomplete="new-password" :required="edit?.auth_method!=='password'"></label>
      <label v-else>{{label('SSH 私钥', 'SSH private key')}}<textarea v-model="form.ssh_key" rows="4" spellcheck="false" :required="edit?.auth_method!=='key'"></textarea></label>
      <label>{{label('sudo 密码（可选）', 'sudo password (optional)')}}<input v-model="form.sudo_password" type="password" autocomplete="new-password"></label>
      <p v-if="edit">{{label('凭据留空会保留已保存的值。', 'Leave credentials blank to keep the saved values.')}}</p>
      <div class="node-actions"><button type="submit" :disabled="busy">{{label('连接并保存', 'Connect and save')}}</button><button type="button" class="quiet" :disabled="busy" @click="form=null">{{label('取消', 'Cancel')}}</button></div>
    </form>
    <p v-if="!nodes.length && !error">{{label('暂无已纳管节点。', 'No managed nodes yet.')}}</p>
    <div v-for="node in nodes" :key="node.id" class="compute-node">
      <div>
        <strong>{{node.name}}</strong>
        <div>{{node.status==='online' ? label('在线','Online') : label('离线','Offline')}} · {{node.arch || '—'}} · {{node.gpu_model || '—'}}</div>
        <small v-if="node.profile">Profile · {{node.profile}}</small>
        <small v-if="canManage">{{node.ssh_user}}@{{node.host}}:{{node.ssh_port}} · {{node.auth_method==='key' ? label('私钥','Private key') : label('密码','Password')}}</small>
      </div>
      <div v-if="canManage" class="node-actions">
        <button class="quiet" :disabled="busy" @click="open(node)">{{label('编辑', 'Edit')}}</button>
        <button class="quiet" :disabled="busy" @click="check(node,'rescan')">{{label('重新扫描', 'Rescan')}}</button>
        <button class="quiet" :disabled="busy" @click="check(node,'runtime')">{{label('验证容器运行', 'Check container runtime')}}</button>
        <button class="quiet" :disabled="busy" @click="remove(node)">{{label('移除', 'Remove')}}</button>
      </div>
    </div>
  </section>
</template>

<style scoped>
.compute-nodes { margin-bottom: 20px; }
.node-form { display: grid; grid-template-columns: repeat(auto-fit,minmax(min(100%, 215px),1fr)); gap: 12px; padding: 14px 0; }
.node-form label { display: grid; align-content: start; gap: 6px; }
.node-form input, .node-form textarea, .node-form select { width: 100%; }
.node-actions { display: flex; flex-wrap: wrap; gap: 8px; align-items: center; }
.compute-node { display: flex; align-items: center; justify-content: space-between; flex-wrap: wrap; gap: 12px; padding: 12px 0; border-top: 1px solid #e6edf3; }
.compute-node small { display: block; margin-top: 3px; }
</style>
