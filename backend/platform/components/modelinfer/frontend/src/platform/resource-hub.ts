import { getSchedulerRuntime } from '@scheduler-auth';
import { getInjectedVosPlatform } from './open-tab';

export type ResourceHub = 'weights' | 'images';

/** V-Scheduler 使用应用默认入口；image-hub 的下载入口位于镜像仓库首页。 */
export function resourceHubUrl(hub: ResourceHub): string {
  return hub === 'weights' ? '/app/com.ictrek.v-scheduler/' : '/app/com.ictrek.image-hub/#/images';
}

/** 保留运行配置页面，优先通过平台标签页访问下载应用。 */
export async function openResourceHub(hub: ResourceHub): Promise<void> {
  const runtime = getSchedulerRuntime();
  if (runtime && hub === 'weights') {
    runtime.openWeights();
    return;
  }
  const url = resourceHubUrl(hub);
  const openTab = getInjectedVosPlatform()?.platform?.openTab;
  if (openTab) {
    try {
      await openTab({ url, title: hub === 'weights' ? 'V-Scheduler' : 'image-hub' });
      return;
    } catch (error) {
      if ((error as { name?: string })?.name === 'PlatformNotAuthenticatedError') return;
    }
  }
  window.open(url, '_blank', 'noopener,noreferrer');
}
