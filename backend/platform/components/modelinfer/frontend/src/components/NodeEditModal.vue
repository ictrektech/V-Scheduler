<script setup lang="ts">
/** 节点编辑配置弹窗（列表页与详情页共用）：认证方式 / 凭据 / 通用配置 / 厂商专属 / 高级配置。 */
import { computed, reactive, ref, watch } from 'vue';
import { message } from 'ant-design-vue';
import { useI18n } from 'vue-i18n';
import {
  DEFAULT_NODE_EXPORTER_PORT,
  DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
  updateNode,
  type NodeItem,
} from '@/api/nodes';
import { resolveApiErrorMessage } from '@/i18n/api-error';

const { t } = useI18n();

const props = defineProps<{
  open: boolean;
  node: NodeItem | null;
}>();
const emit = defineEmits<{
  (e: 'update:open', v: boolean): void;
  (e: 'saved', n: NodeItem): void;
}>();

const saving = ref(false);
const form = reactive({
  name: '',
  ssh_user: 'root',
  auth_method: 'password',
  ssh_password: '',
  ssh_key: '',
  sudo_password: '',
  clear_sudo_password: false,
  fscache: false,
  socket_ifname: '',
  ib_hca: '',
  node_exporter_port: DEFAULT_NODE_EXPORTER_PORT,
  nvidia_gpu_exporter_port: DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
});

const isNvidia = computed(() => props.node?.gpu_vendor === 'nvidia');
const ifaceOptions = computed(() =>
  (props.node?.interfaces || []).map((i) => ({
    label: t('common.ifaceLabel', { name: i.name, ips: i.ips.join(', ') }),
    value: i.name,
  })),
);
const ibOptions = computed(() => (props.node?.ib_devices || []).map((d) => ({ label: d, value: d })));

watch(
  () => props.open,
  (v) => {
    if (v && props.node) {
      form.name = props.node.name;
      form.ssh_user = props.node.ssh_user;
      form.sudo_password = '';
      form.clear_sudo_password = false;
      form.auth_method = props.node.auth_method || 'password';
      form.ssh_password = '';
      form.ssh_key = '';
      form.fscache = props.node.fscache;
      form.socket_ifname = props.node.socket_ifname;
      form.ib_hca = props.node.ib_hca;
      form.node_exporter_port = props.node.node_exporter_port || DEFAULT_NODE_EXPORTER_PORT;
      form.nvidia_gpu_exporter_port =
        props.node.nvidia_gpu_exporter_port || DEFAULT_NVIDIA_GPU_EXPORTER_PORT;
    }
  },
);

function close() {
  emit('update:open', false);
}

function filterIb(input: string, option: { label: string }): boolean {
  return option.label.toLowerCase().includes(input.toLowerCase());
}

async function submit() {
  if (!props.node) return;
  if (!form.name.trim()) {
    message.warning(t('nodes.edit.nameRequired'));
    return;
  }
  if (!form.ssh_user.trim()) {
    message.warning(t('nodes.msg.sshUserRequired'));
    return;
  }
  saving.value = true;
  try {
    const updated = await updateNode(props.node.id, {
      name: form.name.trim(),
      ssh_user: form.ssh_user.trim(),
      auth_method: form.auth_method,
      sudo_password: form.clear_sudo_password ? '' : form.sudo_password || undefined,
      // 留空表示不改
      ssh_password: form.ssh_password || undefined,
      ssh_key: form.ssh_key || undefined,
      fscache: form.fscache,
      socket_ifname: form.socket_ifname,
      ib_hca: form.ib_hca ?? '',
      node_exporter_port: form.node_exporter_port,
      nvidia_gpu_exporter_port: form.nvidia_gpu_exporter_port,
    });
    message.success(t('nodes.edit.updated'));
    emit('saved', updated);
    close();
  } catch (e) {
    message.error(resolveApiErrorMessage(e));
  } finally {
    saving.value = false;
  }
}
</script>

<template>
  <a-modal
    :open="open"
    :title="t('nodes.edit.title')"
    :confirm-loading="saving"
    :ok-text="t('common.save')"
    :cancel-text="t('common.cancel')"
    width="680px"
    @ok="submit"
    @cancel="close"
  >
    <a-form layout="vertical">
      <a-row :gutter="12">
        <a-col :span="12">
          <a-form-item :label="t('nodes.form.name')" required>
            <a-input v-model:value="form.name" />
          </a-form-item>
        </a-col>
        <a-col :span="12">
          <a-form-item :label="t('nodes.form.host')">
            <a-input :value="node?.host" disabled />
          </a-form-item>
        </a-col>
      </a-row>

      <div class="mi-form-section">{{ t('nodes.form.authMethod') }}</div>
      <a-form-item :label="t('nodes.form.sshUser')" required>
        <a-input v-model:value="form.ssh_user" autocomplete="username" />
      </a-form-item>
      <a-form-item :label="t('nodes.edit.sshAuthMethod')">
        <a-radio-group v-model:value="form.auth_method">
          <a-radio-button value="password">{{ t('nodes.authMethod.password') }}</a-radio-button>
          <a-radio-button value="key">{{ t('nodes.authMethod.key') }}</a-radio-button>
        </a-radio-group>
      </a-form-item>
      <a-form-item v-if="form.auth_method === 'password'" :label="t('nodes.edit.sshPassword')">
        <a-input-password v-model:value="form.ssh_password" autocomplete="new-password" />
      </a-form-item>
      <a-form-item v-else :label="t('nodes.edit.rootKey')">
        <a-textarea
          v-model:value="form.ssh_key"
          :rows="4"
          placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
        />
      </a-form-item>

      <template v-if="form.ssh_user.trim() && form.ssh_user.trim() !== 'root'">
        <a-form-item :label="t('nodes.edit.sudoPassword')" :extra="t('nodes.form.sudoHint')">
          <a-input-password v-model:value="form.sudo_password" :disabled="form.clear_sudo_password" autocomplete="new-password" />
          <a-checkbox v-model:checked="form.clear_sudo_password">{{ t('nodes.edit.clearSudoPassword') }}</a-checkbox>
        </a-form-item>
      </template>

      <div class="mi-form-section">{{ t('nodes.config.general') }}</div>
      <a-form-item :label="t('nodes.config.enableFscache')">
        <a-switch v-model:checked="form.fscache" />
        <span class="mi-hint" style="margin-left: 8px">
          {{ t('nodes.config.fscacheHint') }}
        </span>
      </a-form-item>

      <template v-if="isNvidia">
        <div class="mi-form-section">{{ t('nodes.config.nvidia') }}</div>
        <a-row :gutter="12">
          <a-col :span="12">
            <a-form-item :label="t('nodes.config.socketIfname')">
              <a-select
                v-model:value="form.socket_ifname"
                :options="ifaceOptions"
                allow-clear
                :placeholder="t('nodes.config.socketIfnamePlaceholder')"
              />
            </a-form-item>
          </a-col>
          <a-col :span="12">
            <a-form-item :label="t('nodes.config.ibHca')">
              <a-select
                v-model:value="form.ib_hca"
                :options="ibOptions"
                :show-search="true"
                allow-clear
                :placeholder="t('nodes.config.ibHcaPlaceholder')"
                :filter-option="filterIb"
              />
            </a-form-item>
          </a-col>
        </a-row>
      </template>

      <a-collapse ghost>
        <a-collapse-panel key="advanced" :header="t('nodes.config.advanced')">
          <a-form-item :label="t('nodes.config.nodeExporterPort')">
            <a-input-number
              v-model:value="form.node_exporter_port"
              :min="1024"
              :max="65535"
              style="width: 200px"
            />
            <span class="mi-hint" style="margin-left: 8px">
              {{ t('nodes.edit.nodeExporterHint') }}
            </span>
          </a-form-item>
          <a-form-item v-if="isNvidia" :label="t('nodes.config.nvidiaGpuExporterPort')">
            <a-input-number
              v-model:value="form.nvidia_gpu_exporter_port"
              :min="1024"
              :max="65535"
              style="width: 200px"
            />
            <span class="mi-hint" style="margin-left: 8px">
              {{ t('nodes.edit.nvidiaGpuExporterHint') }}
            </span>
          </a-form-item>
        </a-collapse-panel>
      </a-collapse>
    </a-form>
  </a-modal>
</template>

<style scoped>
.mi-form-section {
  margin: 8px 0 12px;
  font-size: 13px;
  font-weight: 600;
  color: var(--mi-text);
  border-left: 3px solid var(--mi-primary);
  padding-left: 8px;
}
.mi-hint {
  color: var(--mi-text-secondary);
  font-size: var(--mi-font-hint);
}
</style>