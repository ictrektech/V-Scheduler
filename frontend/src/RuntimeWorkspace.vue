<script setup lang="ts">
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { getVOSBearerToken, clearCachedVOSBearerToken, type SchedulerRuntimeFrame } from './vos-auth'

const props = defineProps<{
 kind: 'inference' | 'cluster'
 images?: boolean
 locale: string
 request: (path: string) => Promise<unknown>
 t: (value: string) => string
}>()
const emit = defineEmits<{ weights: []; nodes: []; images: []; runtime: [] }>()

const views = computed(() => props.kind === 'inference' ? [
 { route: '/runs', label: '部署列表' },
 { route: '/cards', label: '模型卡片' },
 { route: '/plaza', label: '模型广场' },
 { route: '/nodes', label: '节点监控' },
 { route: '/settings', label: '推理设置' },
] : [
 { route: '/overview', label: '集群概览' },
 { route: '/nodes', label: '集群成员' },
])
const route = ref(views.value[0].route)
const initialRoute = ref(route.value)
const src = computed(() => new URL(`./${props.kind}/#${initialRoute.value}`, location.href).href)
const health = ref<'loading' | 'ready' | 'unavailable'>('loading')
const checking = ref(false)
const failureDetail = ref('')
let frame: SchedulerRuntimeFrame | null = null
let child: Window | null = null
let timer: ReturnType<typeof setInterval> | undefined

async function check() {
 if (checking.value) return
 checking.value = true
 try {
  const result = await props.request(props.kind + '/runtime/health') as { enabled?: boolean; ready?: boolean } | null
  const ready = props.kind === 'cluster' || result?.ready
  health.value = ready ? 'ready' : 'unavailable'
  failureDetail.value = ready ? '' : result?.enabled === false
   ? props.t('VOS 应用配置中尚未启用内置推理运行时。')
   : props.t('推理运行时尚未就绪。')
 } catch (error) {
  health.value = 'unavailable'
  failureDetail.value = error instanceof Error ? error.message : ''
 } finally {
  checking.value = false
 }
}

function setFrame(element: unknown) {
 frame = element as SchedulerRuntimeFrame | null
 if (frame) {
  frame.schedulerRuntime = {
   token: getVOSBearerToken,
   rejectToken: clearCachedVOSBearerToken,
   locale: () => props.locale,
   openWeights: () => emit('weights'),
  }
 } else {
  child?.removeEventListener('hashchange', readRoute)
  child = null
 }
}

function readRoute() {
 if (child) route.value = child.location.hash.slice(1) || views.value[0].route
}

function onLoad() {
 child?.removeEventListener('hashchange', readRoute)
 child = frame?.contentWindow ?? null
 child?.addEventListener('hashchange', readRoute)
 readRoute()
}

function select(next: string) {
 if (props.images) emit('runtime')
 route.value = next
 if (child) child.location.hash = next
 else initialRoute.value = next
}

watch(() => props.locale, () => {
 child?.dispatchEvent(new Event('scheduler:locale'))
})
onMounted(() => { void check(); timer=setInterval(() => {if(health.value==='unavailable')void check()}, 5000) })
onBeforeUnmount(() => {clearInterval(timer);child?.removeEventListener('hashchange', readRoute)})
</script>

<template>
 <section class="runtime-workspace">
  <div class="documentation-tabs" :aria-label="t(kind==='inference'?'推理部署':'VOS 集群管理')">
   <button v-for="view in views" :key="view.route" type="button"
    :class="{current:!images&&(route===view.route||route.startsWith(view.route+'/'))}"
    :aria-current="!images&&(route===view.route||route.startsWith(view.route+'/'))?'page':undefined"
    @click="select(view.route)">{{t(view.label)}}</button>
   <button v-if="kind==='inference'" type="button" :class="{current:images}" :aria-current="images?'page':undefined" @click="emit('images')">{{t('推理镜像')}}</button>
  </div>
  <slot v-if="images" name="images"/>
  <iframe v-if="health==='ready'" v-show="!images" :ref="setFrame" :src="src" :title="t(kind==='inference'?'推理部署':'VOS 集群管理')" @load="onLoad"/>
 <section v-else-if="!images" class="section panel" aria-live="polite">
  <p v-if="health==='loading'">{{t('正在处理')}}</p>
  <template v-else>
   <p>{{t('运行时暂不可用。应用正在自动重试，请检查 VOS 应用配置和服务状态。')}}</p>
   <p v-if="failureDetail">{{failureDetail}}</p>
   <button type="button" class="quiet" :disabled="checking" @click="check">{{t('重试连接')}}</button>
   <button v-if="kind==='inference'" type="button" class="quiet" @click="emit('nodes')">{{t('节点管理')}}</button>
  </template>
 </section>
 </section>
</template>

<style scoped>
.runtime-workspace {min-width:0}
.runtime-workspace .documentation-tabs {margin-bottom:16px;flex-wrap:wrap}
iframe {display:block;width:100%;height:max(430px,calc(100dvh - 184px));border:0;background:transparent}
</style>
