import { ref } from 'vue';
import { fetchMirrors, type MirrorOption } from '@/api/imageHub';

/**
 * 镜像源目录（`GET /api/registry/mirrors`）缓存：进程内加载一次、多个弹窗共享。
 *
 * 优雅降级：接口失败时目录保持为空（或沿用上一次结果），UI 仍可「不使用镜像源」或
 * 「手动输入」，不阻断拉取。
 */
const catalog = ref<MirrorOption[]>([]);
const loaded = ref(false);
const loading = ref(false);
let inflight: Promise<void> | null = null;

export function useMirrors() {
  /** 确保目录已加载（`force` 时强制刷新） */
  async function ensureLoaded(force = false): Promise<void> {
    if (loading.value || (loaded.value && !force)) return;
    if (inflight && !force) return inflight;
    inflight = (async () => {
      loading.value = true;
      try {
        const data = await fetchMirrors();
        catalog.value = data.mirrors ?? [];
        loaded.value = true;
      } catch {
        // 目录不可用：保持现状（空目录），由调用方降级为「不使用 + 手动输入」
      } finally {
        loading.value = false;
        inflight = null;
      }
    })();
    return inflight;
  }

  return { catalog, loaded, loading, ensureLoaded };
}