import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  buildAppAbsoluteUrl,
  getInjectedVosPlatform,
  normalizeTitleForPlatform,
  openDetailInNewTab,
} from '../open-tab';
import type { VosPlatformLike } from '../vos-platform';

const GLOBAL = globalThis as unknown as { window?: unknown };

/** 构造一个最小可用的 iframe window（location + 可选桥接对象） */
function setWindow(platform?: VosPlatformLike, href = 'http://h/app/com.ictrek.modelinfer/#/nodes') {
  GLOBAL.window = { vos_platform: platform, location: { href } };
}

afterEach(() => {
  delete GLOBAL.window;
  vi.restoreAllMocks();
});

describe('buildAppAbsoluteUrl', () => {
  it('以当前文档 URL 为基准带上应用挂载基路径', () => {
    expect(
      buildAppAbsoluteUrl('#/nodes/3', 'http://h/app/com.ictrek.modelinfer/#/nodes'),
    ).toBe('http://h/app/com.ictrek.modelinfer/#/nodes/3');
  });

  it('hashPath 无前导 # 也能构造', () => {
    expect(buildAppAbsoluteUrl('/runs/9', 'http://h/app/com.ictrek.modelinfer/#/runs')).toBe(
      'http://h/app/com.ictrek.modelinfer/#/runs/9',
    );
  });

  it('清空基准 URL 的 query，保证同一目标 URL 确定', () => {
    expect(
      buildAppAbsoluteUrl('#/nodes/3', 'http://h/app/com.ictrek.modelinfer/?x=1#/nodes/2'),
    ).toBe('http://h/app/com.ictrek.modelinfer/#/nodes/3');
    // 同一目标两次构造一致（利于平台按 fullPath 去重）
    expect(buildAppAbsoluteUrl('#/nodes/3', 'http://h/app/com.ictrek.modelinfer/#/nodes')).toBe(
      buildAppAbsoluteUrl('#/nodes/3', 'http://h/app/com.ictrek.modelinfer/#/nodes'),
    );
  });

  it('缺省取 window.location.href', () => {
    setWindow(undefined, 'http://h/app/com.ictrek.modelinfer/#/nodes');
    expect(buildAppAbsoluteUrl('#/nodes/7')).toBe(
      'http://h/app/com.ictrek.modelinfer/#/nodes/7',
    );
  });

  it('无 window / 基准非法时返回空串', () => {
    expect(buildAppAbsoluteUrl('#/nodes/1')).toBe('');
    expect(buildAppAbsoluteUrl('#/nodes/1', 'not-a-url')).toBe('');
  });
});

describe('getInjectedVosPlatform', () => {
  it('未注入（非浏览器 / 无 window）返回 undefined', () => {
    expect(getInjectedVosPlatform()).toBeUndefined();
  });

  it('已注入返回桥接对象', () => {
    const platform: VosPlatformLike = { version: '0.3.0' };
    setWindow(platform);
    expect(getInjectedVosPlatform()).toBe(platform);
  });
});

describe('openDetailInNewTab', () => {
  it('未注入桥接对象回退（返回 false）', async () => {
    expect(await openDetailInNewTab({ hashPath: '#/nodes/1' })).toBe(false);
  });

  it('桥接对象无 platform.openTab 回退（返回 false）', async () => {
    setWindow({ version: '0.2.0' });
    expect(await openDetailInNewTab({ hashPath: '#/nodes/1' })).toBe(false);
  });

  it('openTab 可用时打开并透传含基路径的 URL 与标题（返回 true）', async () => {
    const openTab = vi.fn().mockResolvedValue(undefined);
    setWindow({ version: '0.3.0', platform: { openTab } });

    const handled = await openDetailInNewTab({
      hashPath: '#/nodes/3',
      title: '节点详情 · node21',
    });

    expect(handled).toBe(true);
    expect(openTab).toHaveBeenCalledTimes(1);
    expect(openTab).toHaveBeenCalledWith({
      url: 'http://h/app/com.ictrek.modelinfer/#/nodes/3',
      title: '节点详情 · node21',
    });
  });

  it('平台 >= 0.4.0：多语言标题原样传 map', async () => {
    const openTab = vi.fn().mockResolvedValue(undefined);
    setWindow({ version: '0.4.0', platform: { openTab } });

    const title = { 'zh-CN': '节点详情 · n1', en: 'Node Details · n1' };
    await openDetailInNewTab({ hashPath: '#/nodes/1', title });

    expect(openTab).toHaveBeenCalledWith({
      url: 'http://h/app/com.ictrek.modelinfer/#/nodes/1',
      title,
    });
  });

  it('旧平台（< 0.4.0）：多语言标题折叠为当前语言字符串', async () => {
    const openTab = vi.fn().mockResolvedValue(undefined);
    setWindow({ version: '0.3.0', platform: { openTab } });

    await openDetailInNewTab({
      hashPath: '#/nodes/1',
      title: { 'zh-CN': '节点详情 · n1', en: 'Node Details · n1' },
    });

    expect(openTab).toHaveBeenCalledWith({
      url: 'http://h/app/com.ictrek.modelinfer/#/nodes/1',
      title: '节点详情 · n1',
    });
  });

  it('未传标题时不携带 title 字段', async () => {
    const openTab = vi.fn().mockResolvedValue(undefined);
    setWindow({ version: '0.3.0', platform: { openTab } });

    await openDetailInNewTab({ hashPath: '#/runs/5' });

    expect(openTab).toHaveBeenCalledWith({ url: 'http://h/app/com.ictrek.modelinfer/#/runs/5' });
  });

  it('未登录（PlatformNotAuthenticatedError）不回退：返回 true', async () => {
    const err = new Error('not authenticated');
    err.name = 'PlatformNotAuthenticatedError';
    const openTab = vi.fn().mockRejectedValue(err);
    setWindow({ version: '0.3.0', platform: { openTab } });

    expect(await openDetailInNewTab({ hashPath: '#/nodes/1' })).toBe(true);
  });

  it('其他错误告警并回退：返回 false', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined);
    const openTab = vi.fn().mockRejectedValue(new Error('InvalidRequestError'));
    setWindow({ version: '0.3.0', platform: { openTab } });

    expect(await openDetailInNewTab({ hashPath: '#/nodes/1' })).toBe(false);
    expect(warn).toHaveBeenCalled();
  });

  it('基准 URL 不可用时回退：返回 false 且不调用 openTab', async () => {
    const openTab = vi.fn().mockResolvedValue(undefined);
    // 无 location 的 window（异常场景）
    GLOBAL.window = { vos_platform: { version: '0.3.0', platform: { openTab } } };

    expect(await openDetailInNewTab({ hashPath: '#/nodes/1' })).toBe(false);
    expect(openTab).not.toHaveBeenCalled();
  });
});

describe('normalizeTitleForPlatform', () => {
  it('字符串原样返回（任意平台版本）', () => {
    expect(normalizeTitleForPlatform('标题', { version: '0.3.0' })).toBe('标题');
    expect(normalizeTitleForPlatform('标题', { version: '0.4.0' })).toBe('标题');
  });

  it('多语言 map：新平台原样，旧平台折叠为当前语言', () => {
    const map = { 'zh-CN': '节点详情', en: 'Node Details' };
    expect(normalizeTitleForPlatform(map, { version: '0.4.0' })).toEqual(map);
    expect(normalizeTitleForPlatform(map, { version: '0.3.0' })).toBe('节点详情');
  });
});