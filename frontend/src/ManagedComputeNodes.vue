<script setup lang="ts">
import {isSessionError,recordDiagnostic} from './diagnostics.js'
import {computed,onMounted,onUnmounted,ref} from 'vue'
import {nodeIdentity} from './node-identity'

type Node = {
  id: string
  name: string
  host?: string
  address?: string
  hostname?: string
  ssh_port?: number
  ssh_user?: string
  auth_method?: 'password' | 'key'
  profile?: string
  profile_override?: string
  status: string
  arch: string
  gpu_model: string
  last_checked_at?: string
  connection_error?: string
}
const props = defineProps<{
  request: (path: string, body?: unknown, method?: string) => Promise<any>
  locale: string
}>()
const zh = computed(() => props.locale.startsWith('zh'))
const label = (a: string, b: string) => zh.value ? a : b
const nodes = ref<Node[]>([])
const clusterNodes = ref<{id: string, name: string, role: string, ssh_node_id: string | null, address?: string, hostname?: string, profile?: string}[]>([])
const clusterOnly = computed(() => clusterNodes.value.filter(node => !node.ssh_node_id))
const canManage = ref(false)
const busy = ref(false)
const error = ref('')
const listFailed = ref(false)
const recovering = ref(false)
const lastFailure = ref('')
const refreshing = ref(false)
let failures = 0
const loaded = ref(false)
const feedback = ref('')
const edit = ref<Node | null>(null)
const form = ref<ReturnType<typeof fields> | null>(null)
const base = 'managed-nodes'
const profiles = ref<string[]>(['jetson-thor', 'dgx-spark', 'jetson-orin', 'arm-cpu', 'amd-cpu', 'amd-cuda', 'arm-cuda', 'sophon-bm1688'])
let timer: ReturnType<typeof setInterval>
let refreshRevision = 0
let lastRefresh=0

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
    NODE_SSH_FAILED: ['SSH 连接或命令执行失败。请检查连接配置，或稍后重试。', 'SSH connection or command failed. Check the connection settings or try again shortly.'],
    NODE_IMAGE_PULL_FAILED: ['镜像拉取失败，请检查节点到镜像仓库的连接。', 'Could not pull the image. Check registry access from the node.'],
    NODE_CONTAINER_START_FAILED: ['测试容器未成功启动。', 'The test container did not start successfully.'],
    UNAUTHENTICATED: ['VOS 会话暂不可用，正在尝试恢复。', 'The VOS session is temporarily unavailable. Retrying.'],
    FORBIDDEN: ['当前身份没有读取节点列表的权限。', 'This identity is not authorized to read the node inventory.'],
    NODE_MANAGER_DISABLED: ['SSH 节点管理服务未启用。', 'The SSH node manager is disabled.'],
    NODE_SSH_AUTH_REJECTED: ['SSH 鉴权未通过，请检查连接账户和凭据。', 'SSH authentication failed. Check the account and saved credentials.'],
    NODE_SSH_SUDO_FAILED: ['SSH 已连通，但无法通过已保存的方式取得 sudo 权限。', 'SSH connected, but the saved sudo configuration did not authorize the check.'],
    NODE_SSH_CHECK_TIMEOUT: ['SSH 连接验证超时。', 'SSH verification timed out.'],
    NODE_SSH_UNREACHABLE: ['无法连接节点的 SSH 端口。', 'Could not reach the SSH port.'],
    NODE_MANAGER_UNAVAILABLE: ['节点服务暂不可用。', 'The node manager is temporarily unavailable.'],
  }
  const message = code && errors[code]
  return message ? label(...message) : label('节点操作失败。', 'Node operation failed.')
}
async function refresh() {
  const revision = ++refreshRevision
  lastRefresh=Date.now()
  refreshing.value=true
  try {
    const response = await props.request(base)
    if (revision !== refreshRevision) return
    if (!Array.isArray(response?.items)) throw new Error('Invalid node inventory')
    nodes.value = response.items
    if (Array.isArray(response.cluster_nodes)) clusterNodes.value = response.cluster_nodes
    canManage.value = response.can_manage
    loaded.value = true
    listFailed.value = false
    recovering.value = false
    failures = 0
    if (!canManage.value) form.value = null
  } catch (reason) {
    if (revision !== refreshRevision) return
    recovering.value=isSessionError(reason)
    if(recovering.value){recordDiagnostic('AUTH_REQUEST_REJECTED','managed_nodes');return}
    failures+=1
    lastFailure.value=describe(reason)
    listFailed.value=failures>=2
  } finally {
    if(revision===refreshRevision)refreshing.value=false
  }
}
async function open(node?: Node) {
  edit.value = node || null
  form.value = fields(node)
  try {
    const types = await props.request('compute-profiles')
    profiles.value = types.items.map((profile: { name: string }) => profile.name)
  } catch { /* built-in Profiles remain available */ }
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
onMounted(() => { void refresh(); timer = setInterval(() => { if (!busy.value&&!refreshing.value&&(recovering.value||Date.now()-lastRefresh>=10000)) void refresh() }, 2500) })
onUnmounted(() => clearInterval(timer))
</script>

<template>
  <section class="section panel compute-nodes">
    <div class="section-head">
      <h2>{{label('SSH 算力节点', 'SSH compute nodes')}}</h2>
      <button v-if="canManage" :disabled="busy" @click="open()">{{label('添加节点', 'Add node')}}</button>
      <button v-if="listFailed" class="quiet" :disabled="busy||refreshing" @click="refresh">{{label('重试读取', 'Retry loading')}}</button>
    </div>
    <p v-if="error" class="error" role="alert">{{error}}</p>
    <p v-if="recovering&&!loaded" role="status">{{label('正在恢复连接，节点列表将自动刷新。','Recovering the connection. The node list will refresh automatically.')}}</p>
  <p v-if="!loaded&&!listFailed&&!recovering" role="status">{{label('正在读取节点列表…','Loading nodes…')}}</p>
    <p v-if="listFailed&&!recovering" role="status">
      {{loaded
        ? label('节点列表暂时无法刷新，以下为上次读取的记录。', 'Could not refresh nodes. Showing the last loaded list.')
        : label('暂时无法读取节点列表。', 'Could not load the node list.')}} {{lastFailure}}
    </p>
    <p v-if="feedback" role="status">{{feedback}}</p>
    <form v-if="form && canManage" class="node-form" @submit.prevent="save">
      <label>{{label('节点名称', 'Node name')}}<input v-model.trim="form.name" required maxlength="128"></label>
      <label>{{label('主机地址', 'Host')}}<input v-model.trim="form.host" required maxlength="255"></label>
      <label>{{label('SSH 端口', 'SSH port')}}<input v-model.number="form.ssh_port" type="number" min="1" max="65535" required></label>
      <label>{{label('SSH 用户', 'SSH user')}}<input v-model.trim="form.ssh_user" required maxlength="64"></label>
      <label>{{label('认证方式', 'Authentication')}}<select v-model="form.auth_method"><option value="key">{{label('私钥', 'Private key')}}</option><option value="password">{{label('密码', 'Password')}}</option></select></label>
      <label>Profile<select v-model="form.profile"><option value="auto">{{label('自动识别', 'Auto-detect')}}</option><option v-if="form.profile!=='auto'&&!profiles.includes(form.profile)" :value="form.profile">{{form.profile}}</option><option v-for="profile in profiles" :key="profile" :value="profile">{{profile}}</option></select></label>
      <label v-if="form.auth_method==='password'">{{label('SSH 密码', 'SSH password')}}<input v-model="form.ssh_password" type="password" autocomplete="new-password" :required="edit?.auth_method!=='password'"></label>
      <label v-else>{{label('SSH 私钥', 'SSH private key')}}<textarea v-model="form.ssh_key" rows="4" spellcheck="false" :required="edit?.auth_method!=='key'"></textarea></label>
      <label>{{label('sudo 密码（可选）', 'sudo password (optional)')}}<input v-model="form.sudo_password" type="password" autocomplete="new-password"></label>
      <p v-if="edit">{{label('凭据留空会保留已保存的值。', 'Leave credentials blank to keep the saved values.')}}</p>
      <div class="node-actions"><button type="submit" :disabled="busy">{{label('连接并保存', 'Connect and save')}}</button><button type="button" class="quiet" :disabled="busy" @click="form=null">{{label('取消', 'Cancel')}}</button></div>
    </form>
    <p v-if="loaded && !listFailed && !nodes.length && !clusterOnly.length">{{label('暂无已纳管节点。', 'No managed nodes yet.')}}</p>
    <div v-for="node in nodes" :key="node.id" class="compute-node">
      <div>
        <strong>{{nodeIdentity(node).name}}</strong>
        <div>{{[nodeIdentity(node).address, nodeIdentity(node).profile].filter(Boolean).join(' · ')}}</div>
        <small>{{node.status==='online' ? label('在线','Online') : label('离线','Offline')}}<template v-if="node.connection_error"> · {{describe({code:node.connection_error})}}</template></small>
        <small v-if="clusterNodes.some(member => member.ssh_node_id === node.id)">{{label('VOS 集群成员','VOS cluster member')}}</small>
      </div>
      <div v-if="canManage" class="node-actions">
        <button class="quiet" :disabled="busy" @click="open(node)">{{label('编辑', 'Edit')}}</button>
        <button class="quiet" :disabled="busy" @click="check(node,'rescan')">{{label('重新扫描', 'Rescan')}}</button>
        <button class="quiet" :disabled="busy" @click="check(node,'runtime')">{{label('验证容器运行', 'Check container runtime')}}</button>
        <button class="quiet" :disabled="busy" @click="remove(node)">{{label('移除', 'Remove')}}</button>
      </div>
    </div>
    <div v-for="member in clusterOnly" :key="member.id" class="compute-node">
      <div>
        <strong>{{nodeIdentity(member).name}}</strong>
        <div>{{[nodeIdentity(member).address, nodeIdentity(member).profile].filter(Boolean).join(' · ')}}</div>
        <small>{{label('VOS 集群成员','VOS cluster member')}} · {{member.role}}</small>
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
