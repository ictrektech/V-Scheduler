import { afterEach, describe, expect, it, vi } from 'vitest';
import { openResourceHub } from '../resource-hub';

afterEach(() => vi.unstubAllGlobals());

describe('下载应用跳转', () => {
  it('在平台标签页打开 V-Scheduler 和 image-hub', async () => {
    const openTab = vi.fn().mockResolvedValue(undefined);
    const open = vi.fn();
    vi.stubGlobal('window', { vos_platform: { platform: { openTab } }, open });
    await openResourceHub('weights');
    await openResourceHub('images');
    expect(openTab.mock.calls).toEqual([
      [{ url: '/app/com.ictrek.v-scheduler/', title: 'V-Scheduler' }],
      [{ url: '/app/com.ictrek.image-hub/#/images', title: 'image-hub' }],
    ]);
    expect(open).not.toHaveBeenCalled();
  });

  it('无桥接或桥接失败时回退浏览器标签页，保留当前配置', async () => {
    const open = vi.fn();
    vi.stubGlobal('window', { open });
    await openResourceHub('weights');
    expect(open).toHaveBeenCalledWith('/app/com.ictrek.v-scheduler/', '_blank', 'noopener,noreferrer');
    const openTab = vi.fn().mockRejectedValue(new Error('unavailable'));
    vi.stubGlobal('window', { open, vos_platform: { platform: { openTab } } });
    await openResourceHub('images');
    expect(open).toHaveBeenCalledWith('/app/com.ictrek.image-hub/#/images', '_blank', 'noopener,noreferrer');
  });
});
