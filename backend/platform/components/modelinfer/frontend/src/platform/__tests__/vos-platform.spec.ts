import { afterEach, describe, expect, it } from 'vitest';
import {
  initPlatformSync,
  mapPlatformLocale,
  mapPlatformTheme,
  type VosPlatformLike,
} from '../vos-platform';

const GLOBAL = globalThis as unknown as { window?: unknown };

afterEach(() => {
  delete GLOBAL.window;
});

describe('mapPlatformLocale', () => {
  it('zh 前缀映射 zh-CN，其余映射 en', () => {
    expect(mapPlatformLocale('zh-CN')).toBe('zh-CN');
    expect(mapPlatformLocale('zh-TW')).toBe('zh-CN');
    expect(mapPlatformLocale('en-US')).toBe('en');
    expect(mapPlatformLocale('ja-JP')).toBe('en');
  });

  it('空值忽略（返回 null）', () => {
    expect(mapPlatformLocale(null)).toBeNull();
    expect(mapPlatformLocale(undefined)).toBeNull();
    expect(mapPlatformLocale('')).toBeNull();
  });
});

describe('mapPlatformTheme', () => {
  it('仅 light / dark 有效', () => {
    expect(mapPlatformTheme('light')).toBe('light');
    expect(mapPlatformTheme('dark')).toBe('dark');
  });

  it('其他值忽略（返回 null）', () => {
    expect(mapPlatformTheme('auto')).toBeNull();
    expect(mapPlatformTheme(null)).toBeNull();
    expect(mapPlatformTheme('')).toBeNull();
  });
});

describe('initPlatformSync', () => {
  it('未注入（非 iframe）时降级返回 null，不抛错', async () => {
    const api = await initPlatformSync({});
    expect(api).toBeNull();
  });

  it('先订阅、后 getState，并应用对齐状态', async () => {
    const calls: string[] = [];
    const handlers: Record<string, (payload: Record<string, unknown>) => void> = {};
    const platform: VosPlatformLike = {
      version: '0.2.0',
      broadcast: {
        on: (event, handler) => {
          calls.push(`on:${event}`);
          handlers[event] = handler;
          return () => undefined;
        },
      },
      getState: async () => {
        calls.push('getState');
        return { locale: 'en-US', theme: 'dark' };
      },
    };
    GLOBAL.window = { vos_platform: platform };

    const seenLocales: string[] = [];
    const seenThemes: string[] = [];
    const api = await initPlatformSync({
      onLocale: (l) => seenLocales.push(l),
      onTheme: (t) => seenThemes.push(t),
    });

    expect(api).toBe(platform);
    // 订阅先于 getState
    expect(calls).toEqual(['on:locale-change', 'on:theme-change', 'getState']);
    // getState 对齐
    expect(seenLocales).toEqual(['en']);
    expect(seenThemes).toEqual(['dark']);

    // 事件到达时映射为应用值
    handlers['locale-change']({ locale: 'zh-CN' });
    handlers['theme-change']({ theme: 'light' });
    expect(seenLocales).toEqual(['en', 'zh-CN']);
    expect(seenThemes).toEqual(['dark', 'light']);
  });

  it('桥接对象缺失 broadcast / getState 时安全降级', async () => {
    GLOBAL.window = { vos_platform: { version: '0.1.0' } as VosPlatformLike };
    const api = await initPlatformSync({ onLocale: () => undefined, onTheme: () => undefined });
    expect(api).not.toBeNull();
  });
});