import { computed, onMounted, ref, watch } from 'vue';
import { fetchAddresses, fetchRegistryInfo } from '@/api/imageHub';
import { addressListOrFallback } from '@/utils/ip';

/**
 * registry 地址选择（NIC 下拉）：端口来自后端配置，IP 列表来自平台设备网卡，
 * 默认选中与当前页面主机一致的地址；空列表兜底当前主机名。
 *
 * 用户选择的地址会**持久化到 localStorage**，跨页面（镜像列表 / 详情页）与各弹窗共享，
 * 从而使后续生成的镜像拉取/上传命令行参数与上次所选地址一致；当持久化地址不再出现在网卡
 * 列表中时回退到默认（当前页面 IP 优先）。
 */
const STORAGE_KEY = 'image-hub.registry-ip';

function readSavedIp(): string {
  if (typeof localStorage === 'undefined') return '';
  try {
    return localStorage.getItem(STORAGE_KEY) ?? '';
  } catch {
    return '';
  }
}

function saveIp(ip: string): void {
  if (typeof localStorage === 'undefined') return;
  try {
    if (ip) localStorage.setItem(STORAGE_KEY, ip);
    else localStorage.removeItem(STORAGE_KEY);
  } catch {
    // 存储不可用（隐私模式等）时静默忽略
  }
}

export function useRegistryAddress() {
  const port = ref(import.meta.env.VITE_SCHEDULER_IMAGE_HUB === 'true' ? 36113 : 19913);
  const addresses = ref<string[]>([]);
  const selectedIp = ref('');

  const currentHost = typeof window !== 'undefined' ? window.location.hostname : '';

  async function reload(): Promise<void> {
    let publishedAddress: string | undefined;
    try {
      const info = await fetchRegistryInfo();
      if (info?.registry_port) port.value = info.registry_port;
      publishedAddress = info?.published_address;
    } catch {
      // 后端不可用时保留默认端口
    }
    if (publishedAddress && publishedAddress !== '0.0.0.0' && publishedAddress !== '::') {
      addresses.value = [publishedAddress];
      selectedIp.value = publishedAddress;
      return;
    }
    let list: string[] = [];
    try {
      const res = await fetchAddresses();
      list = res.ip_addr ?? [];
    } catch {
      list = [];
    }
    addresses.value = addressListOrFallback(list, currentHost);
    const saved = readSavedIp();
    selectedIp.value =
      saved && addresses.value.includes(saved) ? saved : (addresses.value[0] ?? '');
  }

  // 持久化用户选择，作用于后续命令行参数生成
  watch(selectedIp, (v) => saveIp(v));

  const registryHost = computed(() =>
    selectedIp.value ? `${selectedIp.value}:${port.value}` : '',
  );
  const registryUrl = computed(() => (registryHost.value ? `http://${registryHost.value}` : ''));

  onMounted(reload);

  return { port, addresses, selectedIp, registryHost, registryUrl, reload };
}