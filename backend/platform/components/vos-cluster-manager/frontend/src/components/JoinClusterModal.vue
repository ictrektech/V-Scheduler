<template>
  <a-modal
    :open="open"
    title="加入集群"
    ok-text="加入" cancel-text="取消"
    :confirm-loading="joining"
    @update:open="emit('update:open', $event)"
    @ok="doJoin"
  >
    <a-alert
      type="info"
      show-icon
      style="margin-bottom: 12px"
      message="把本节点加入一个已存在的集群"
      description="入群后本节点将成为该集群的管理节点（自动部署 etcd + postgres-ha）。编排由目标集群的管理节点执行，本节点等待本地 etcd 就绪。"
    />
    <a-form layout="vertical" :model="form">
      <a-form-item label="集群管理节点 IP" required>
        <a-input v-model:value="form.seed_ip" placeholder="如 10.100.10.21" @press-enter="doJoin" />
        <div v-if="seedError" style="color: #cf1322; font-size: 12px">{{ seedError }}</div>
        <div v-else style="color: rgba(0,0,0,0.45); font-size: 12px">
          目标集群任一管理节点的 IP（须已安装本应用且 19906 可达）
        </div>
      </a-form-item>
      <a-form-item label="管理员密码" required>
        <a-input-password v-model:value="form.admin_password" placeholder="该集群节点的管理密码（≥6 位）" />
        <div v-if="pwdError" style="color: #cf1322; font-size: 12px">{{ pwdError }}</div>
      </a-form-item>
      <a-form-item label="本节点名称">
        <a-input v-model:value="form.display_name" placeholder="可选，缺省回退本机 IP" />
      </a-form-item>
      <a-form-item label="本节点 IP">
        <a-input v-model:value="form.node_ip" placeholder="可选，留空自动探测" />
        <div v-if="nodeIpError" style="color: #cf1322; font-size: 12px">{{ nodeIpError }}</div>
        <div v-else style="color: rgba(0,0,0,0.45); font-size: 12px">
          多网卡场景可显式指定到达管理节点所用的本机 IP
        </div>
      </a-form-item>
    </a-form>
    <a-alert
      type="warning"
      show-icon
      message="入群完成后"
      description="请到「节点管理」对本节点点一次「部署入口」（haproxy），以获得本地 15432 统一数据库入口。"
    />
  </a-modal>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue';
import { joinCluster } from '../api/cluster';
import { isValidIpv4, validatePassword } from '../utils/validation';

const props = defineProps<{ open: boolean }>();
const emit = defineEmits<{
  'update:open': [value: boolean];
  success: [];
  error: [message: string];
}>();

const joining = ref(false);

const form = reactive({
  seed_ip: '',
  admin_password: '',
  display_name: '',
  node_ip: '',
});

const seedError = computed(() => (form.seed_ip && !isValidIpv4(form.seed_ip) ? 'IP 地址格式非法' : ''));
const pwdError = computed(() =>
  form.admin_password && !validatePassword(form.admin_password) ? '密码长度需 ≥6' : '',
);
const nodeIpError = computed(() => (form.node_ip && !isValidIpv4(form.node_ip) ? 'IP 地址格式非法' : ''));

/** 每次打开都重置表单，避免复用时残留上次输入 */
watch(
  () => props.open,
  (open) => {
    if (open) resetForm();
  },
);

function resetForm(): void {
  form.seed_ip = '';
  form.admin_password = '';
  form.display_name = '';
  form.node_ip = '';
}

async function doJoin(): Promise<void> {
  if (!form.seed_ip || seedError.value) {
    emit('error', '请填写合法的集群管理节点 IP');
    return;
  }
  if (!form.admin_password || pwdError.value) {
    emit('error', '请填写长度 ≥6 的管理员密码');
    return;
  }
  if (form.node_ip && nodeIpError.value) {
    emit('error', '请填写合法的本节点 IP，或留空自动探测');
    return;
  }
  joining.value = true;
  try {
    await joinCluster({
      seed_ip: form.seed_ip.trim(),
      admin_password: form.admin_password,
      display_name: form.display_name.trim() || undefined,
      node_ip: form.node_ip.trim() || undefined,
    });
    emit('update:open', false);
    resetForm();
    emit('success');
  } catch (e) {
    emit('error', String(e));
  } finally {
    joining.value = false;
  }
}
</script>