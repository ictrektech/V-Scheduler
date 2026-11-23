<template>
  <a-modal
    :open="open"
    title="加入节点"
    ok-text="加入" cancel-text="取消"
    :confirm-loading="adding"
    @update:open="emit('update:open', $event)"
    @ok="doAdd"
  >
    <a-form layout="vertical" :model="addForm">
      <a-form-item label="节点 IP" required>
        <a-input v-model:value="addForm.node_ip" placeholder="如 10.100.10.23" @press-enter="doAdd" />
        <div v-if="ipError" style="color: #cf1322; font-size: 12px">{{ ipError }}</div>
      </a-form-item>
      <a-form-item label="管理员密码" required>
        <a-input-password v-model:value="addForm.admin_password" placeholder="节点 ssh/管理密码（≥6 位）" />
        <div v-if="pwdError" style="color: #cf1322; font-size: 12px">{{ pwdError }}</div>
      </a-form-item>
      <a-form-item label="名称">
        <a-input v-model:value="addForm.display_name" placeholder="可选，缺省回退节点 IP" />
      </a-form-item>
      <a-form-item label="角色">
        <a-radio-group v-model:value="addForm.role">
          <a-radio v-for="r in NODE_ROLES" :key="r" :value="r">{{ roleLabel(r) }}</a-radio>
        </a-radio-group>
        <div style="color: rgba(0,0,0,0.45); font-size: 12px">
          管理节点：运行 etcd + 高可用 postgres（同步维护集群状态）；工作节点：不部署基础设施，可运行集群化负载。
        </div>
      </a-form-item>
      <a-form-item v-if="addForm.role === 'manager'">
        <a-switch v-model:checked="addForm.join_infra" />
        <span style="margin-left: 8px">联动部署 etcd / postgres-ha 子项目（加入后自动编排，未勾选仅登记元数据）</span>
      </a-form-item>
      <a-form-item v-else>
        <a-alert type="info" show-icon message="工作节点不部署 etcd / postgres-ha" description="加入后仅登记元数据；如需扩展管理面（etcd raft / postgres 副本），请选择「管理节点」。">
        </a-alert>
      </a-form-item>
    </a-form>
  </a-modal>
</template>

<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue';
import { addNode } from '../api/nodes';
import { isValidIpv4, validatePassword } from '../utils/validation';
import { NODE_ROLES, roleLabel, type NodeRole } from '../utils/roles';

const props = withDefaults(
  defineProps<{
    open: boolean;
    /** 打开时默认选中的角色（缺省 worker） */
    defaultRole?: NodeRole;
  }>(),
  { defaultRole: 'worker' },
);
const emit = defineEmits<{
  'update:open': [value: boolean];
  success: [];
  error: [message: string];
}>();

const adding = ref(false);

const addForm = reactive({
  node_ip: '',
  admin_password: '',
  display_name: '',
  role: 'worker' as NodeRole,
  join_infra: true as boolean,
});

const ipError = computed(() => (addForm.node_ip && !isValidIpv4(addForm.node_ip) ? 'IP 地址格式非法' : ''));
const pwdError = computed(() => (addForm.admin_password && !validatePassword(addForm.admin_password) ? '密码长度需 ≥6' : ''));

/** 重置为打开前的初始态（角色回到 defaultRole） */
function resetForm(): void {
  addForm.node_ip = '';
  addForm.admin_password = '';
  addForm.display_name = '';
  addForm.role = props.defaultRole;
  addForm.join_infra = true;
}

/** 每次打开都重置表单：避免跨页面复用时残留上次输入（角色回落到 defaultRole） */
watch(
  () => props.open,
  (open) => {
    if (open) resetForm();
  },
);

async function doAdd(): Promise<void> {
  if (!addForm.node_ip || ipError.value) {
    emit('error', '请填写合法的节点 IP');
    return;
  }
  if (!addForm.admin_password || pwdError.value) {
    emit('error', '请填写长度 ≥6 的管理员密码');
    return;
  }
  // worker 不部署基础设施（后端 join_infra+worker → 20501，前端显式避免）
  const joinInfra = addForm.role === 'manager' ? addForm.join_infra : false;
  adding.value = true;
  try {
    await addNode({
      node_ip: addForm.node_ip.trim(),
      admin_password: addForm.admin_password,
      role: addForm.role,
      display_name: addForm.display_name.trim() || undefined,
      join_infra: joinInfra,
    });
    emit('update:open', false);
    resetForm();
    emit('success');
  } catch (e) {
    emit('error', String(e));
  } finally {
    adding.value = false;
  }
}
</script>
