<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'

type Profile = { name: string; builtin: boolean }
type CatalogImage = { image_ref: string; profiles: string[]; tested_profiles: string[]; created_at: string; updated_at: string }

const props = defineProps<{
 request: (path: string, body?: unknown) => Promise<any>
 locale: string
 t: (value: string) => string
 administrator: boolean
}>()
const emit = defineEmits<{ deployments: [] }>()

const images = ref<CatalogImage[]>([])
const profiles = ref<Profile[]>([])
const canManage = ref(false)
const busy = ref(false)
const editing = ref(false)
const error = ref('')
const notice = ref('')
const reference = ref('')
const selected = ref<string[]>([])
const profileName = ref('')
const filter = ref('')
const filtered = computed(() => images.value.filter(image =>
 !filter.value || image.profiles.includes(filter.value)
 || filter.value === 'dgx-spark' && image.profiles.includes('jetson-thor'),
))

function describe(reason: unknown) {
 const code = (reason as { code?: string })?.code
 if (code === 'COMPUTE_PROFILE_NOT_REGISTERED') return props.t('请先登记所选 Profile。')
 if (code === 'INVALID_REQUEST') return props.t('请检查镜像地址和 Profile 名称。')
 return props.t('操作未完成，请刷新后重试。')
}

async function load() {
 const [catalog, types] = await Promise.all([
  props.request('inference/images'),
  props.request('compute-profiles'),
 ])
 images.value = catalog.items
 profiles.value = types.items
 canManage.value = catalog.can_manage && types.can_manage
}

async function perform(action: () => Promise<void>) {
 if (busy.value) return
 busy.value = true
 error.value = ''
 notice.value = ''
 try {
  await action()
 } catch (reason) {
  const status = (reason as {status?:number})?.status
  if (status === 401 || status === 403) {
   images.value = []
   profiles.value = []
   canManage.value = false
   editing.value = false
  }
  error.value = describe(reason)
 } finally {
  busy.value = false
 }
}

function open(image?: CatalogImage) {
 reference.value = image?.image_ref ?? ''
 selected.value = image?.profiles.slice() ?? []
 profileName.value = ''
 error.value = ''
 notice.value = ''
 editing.value = true
}

function addProfile() {
 const name = profileName.value.trim()
 if (!name) return
 void perform(async () => {
  const entry: Profile = await props.request('compute-profiles', { name })
  profiles.value = [...profiles.value.filter(profile => profile.name !== entry.name), entry]
  if (!selected.value.includes(entry.name)) selected.value.push(entry.name)
  profileName.value = ''
  notice.value = props.t('Profile 已登记')
 })
}

function save() {
 void perform(async () => {
  await props.request('inference/images', { image_ref: reference.value.trim(), profiles: selected.value })
  await load()
  editing.value = false
  notice.value = props.t('镜像已登记')
 })
}

function copy(image: CatalogImage) {
 void perform(async () => {
  await navigator.clipboard.writeText(image.image_ref)
  notice.value = props.t('已复制')
 })
}

onMounted(() => perform(load))
</script>

<template>
 <section class="image-catalog">
  <div v-if="administrator" class="documentation-tabs">
   <button type="button" @click="emit('deployments')">{{t('部署列表')}}</button>
   <button type="button" class="current" aria-current="page">{{t('推理镜像')}}</button>
  </div>
  <div v-if="error" class="error" role="alert">{{error}}</div>
  <p v-if="notice" role="status">{{notice}}</p>
  <section v-if="editing && canManage" class="section panel">
   <form class="catalog-form" @submit.prevent="save">
    <label class="catalog-field">{{t('镜像地址')}}
     <input v-model="reference" required maxlength="512" spellcheck="false"
      placeholder="registry.example.com/team/vllm:0.1"/>
    </label>
    <fieldset>
     <legend>{{t('支持的 Profile')}}</legend>
     <div class="profile-options">
      <label v-for="profile in profiles" :key="profile.name">
       <input v-model="selected" type="checkbox" :value="profile.name"/>{{profile.name}}
      </label>
     </div>
    </fieldset>
    <div class="profile-register">
     <label class="catalog-field">{{t('新增 Profile')}}
      <input v-model="profileName" maxlength="64" spellcheck="false" placeholder="arm-new-board"
       @keydown.enter.prevent="addProfile"/>
     </label>
     <button type="button" class="quiet" :disabled="busy||!profileName.trim()" @click="addProfile">{{t('登记 Profile')}}</button>
    </div>
    <div class="catalog-actions">
     <button type="button" class="quiet" :disabled="busy" @click="editing=false">{{t('取消')}}</button>
     <button type="submit" :disabled="busy||!reference.trim()||!selected.length">{{t('登记镜像')}}</button>
    </div>
   </form>
  </section>
  <section class="section panel">
   <div class="catalog-toolbar">
    <label>{{t('目标 Profile')}}
     <select v-model="filter">
      <option value="">{{t('全部 Profile')}}</option>
      <option v-for="profile in profiles" :key="profile.name" :value="profile.name">{{profile.name}}</option>
     </select>
    </label>
    <div class="catalog-actions">
     <button type="button" class="quiet" :disabled="busy" @click="perform(load)">{{t('刷新')}}</button>
     <button v-if="canManage" type="button" :disabled="busy" @click="open()">{{t('登记镜像')}}</button>
    </div>
   </div>
   <div v-if="filtered.length" class="table-wrap">
    <table>
     <thead><tr><th>{{t('镜像地址')}}</th><th>{{t('支持的 Profile')}}</th><th>{{t('已运行过的 Profile')}}</th><th>{{t('最近更新')}}</th><th>{{t('操作')}}</th></tr></thead>
     <tbody>
      <tr v-for="image in filtered" :key="image.image_ref">
       <td><code>{{image.image_ref}}</code></td>
       <td><span class="catalog-profile" v-for="profile in image.profiles" :key="profile">{{profile}}</span></td>
       <td>
        <template v-if="image.tested_profiles?.length">
         <span class="catalog-profile tested" v-for="profile in image.tested_profiles" :key="profile">{{profile}}</span>
        </template>
        <span v-else>—</span>
       </td>
       <td>{{new Date(image.updated_at).toLocaleString(locale)}}</td>
       <td><div class="catalog-actions">
        <button type="button" class="quiet" :disabled="busy" @click="copy(image)">{{t('复制地址')}}</button>
        <button v-if="canManage" type="button" class="quiet" :disabled="busy" @click="open(image)">{{t('编辑 Profile')}}</button>
       </div></td>
      </tr>
     </tbody>
    </table>
   </div>
   <p v-else role="status">{{t(busy?'正在处理':'暂无已登记的镜像')}}</p>
  </section>
 </section>
</template>

<style scoped>
.catalog-toolbar,.catalog-actions,.profile-register,.profile-options {display:flex;align-items:center;gap:10px;flex-wrap:wrap}
.catalog-toolbar {justify-content:space-between;margin-bottom:16px}
.catalog-toolbar select {margin-inline-start:8px}
.catalog-form,.catalog-field {display:grid;gap:8px}
.catalog-form {gap:16px}
.catalog-form fieldset {border:1px solid #d9e3ee;border-radius:8px;padding:12px}
.catalog-form legend,.catalog-field {font-weight:550}
.catalog-form input:not([type=checkbox]),.catalog-toolbar select {
 padding:8px 10px;border:1px solid #d9e3ee;border-radius:7px;background:#fff;color:inherit
}
.catalog-form .catalog-field {min-width:min(250px,100%)}
.catalog-form .profile-register {align-items:end}
.catalog-form > .catalog-actions {justify-content:flex-end}
.profile-options label {display:flex;align-items:center;gap:4px;padding:4px 6px;white-space:nowrap}
.catalog-profile {display:inline-block;margin:2px 6px 2px 0;padding:3px 7px;border-radius:5px;background:#e5f4f3;color:#08717a}
.catalog-profile.tested {background:#eef2f7;color:#455c75}
td code {overflow-wrap:anywhere}
</style>
