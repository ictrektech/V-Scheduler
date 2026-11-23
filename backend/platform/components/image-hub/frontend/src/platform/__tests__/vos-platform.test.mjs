// platform/vos-platform.ts 单测（node:test + Node 原生 TS strip）
import { test, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import {
  DEFAULT_WAIT_TIMEOUT_MS,
  initPlatformSync,
  mapPlatformLocale,
  mapPlatformTheme,
  waitForVosPlatform,
} from '../vos-platform.ts';

const originalWindow = globalThis.window;

function setWindow({ vosPlatform, isIframe = true }) {
  const w = { vos_platform: vosPlatform };
  w.parent = isIframe ? {} : w;
  globalThis.window = w;
  return w;
}

beforeEach(() => {
  delete globalThis.window;
});

afterEach(() => {
  if (originalWindow === undefined) delete globalThis.window;
  else globalThis.window = originalWindow;
});

test('mapPlatformLocale：zh* → zh-CN，其余 → en，空值忽略', () => {
  assert.equal(mapPlatformLocale('zh-CN'), 'zh-CN');
  assert.equal(mapPlatformLocale('zh'), 'zh-CN');
  assert.equal(mapPlatformLocale('zh-TW'), 'zh-CN');
  assert.equal(mapPlatformLocale('en-US'), 'en');
  assert.equal(mapPlatformLocale('ja-JP'), 'en');
  assert.equal(mapPlatformLocale(''), null);
  assert.equal(mapPlatformLocale(undefined), null);
});

test('mapPlatformTheme：仅 light / dark 有效，themeMode 不参与', () => {
  assert.equal(mapPlatformTheme('dark'), 'dark');
  assert.equal(mapPlatformTheme('light'), 'light');
  assert.equal(mapPlatformTheme('auto'), null);
  assert.equal(mapPlatformTheme(undefined), null);
});

test('waitForVosPlatform：无 window → null', async () => {
  assert.equal(await waitForVosPlatform(10), null);
});

test('waitForVosPlatform：已注入立即返回（含顶层 WebView）', async () => {
  const platform = { version: '0.4.0' };
  // 顶层窗口但有桥接对象（移动 WebView 场景）
  setWindow({ vosPlatform: platform, isIframe: false });
  assert.equal(await waitForVosPlatform(10), platform);
});

test('waitForVosPlatform：非 iframe 且无桥接对象 → 立即 null', async () => {
  setWindow({ vosPlatform: undefined, isIframe: false });
  const start = Date.now();
  assert.equal(await waitForVosPlatform(DEFAULT_WAIT_TIMEOUT_MS), null);
  assert.ok(Date.now() - start < 100, '独立访问不应等待超时');
});

test('waitForVosPlatform：iframe 内延迟注入 → 轮询到对象', async () => {
  const w = setWindow({ vosPlatform: undefined, isIframe: true });
  setTimeout(() => {
    w.vos_platform = { version: '0.4.0' };
  }, 20);
  const platform = await waitForVosPlatform(500);
  assert.equal(platform?.version, '0.4.0');
});

test('waitForVosPlatform：iframe 内超时 → null', async () => {
  setWindow({ vosPlatform: undefined, isIframe: true });
  const start = Date.now();
  assert.equal(await waitForVosPlatform(80), null);
  assert.ok(Date.now() - start >= 70);
});

test('initPlatformSync：先订阅事件再 getState 对齐', async () => {
  const calls = [];
  let localeHandler;
  let themeHandler;
  const platform = {
    version: '0.4.0',
    broadcast: {
      on(event, handler) {
        calls.push(`on:${event}`);
        if (event === 'locale-change') localeHandler = handler;
        if (event === 'theme-change') themeHandler = handler;
        return () => {};
      },
    },
    async getState() {
      calls.push('getState');
      return { locale: 'en-US', theme: 'dark', themeMode: 'auto' };
    },
  };
  setWindow({ vosPlatform: platform, isIframe: true });

  const seen = [];
  await initPlatformSync({
    onLocale: (locale) => seen.push(`locale:${locale}`),
    onTheme: (theme) => seen.push(`theme:${theme}`),
  });

  assert.deepEqual(calls, ['on:locale-change', 'on:theme-change', 'getState']);
  assert.deepEqual(seen, ['locale:en', 'theme:dark']);

  // 后续事件也能更新
  localeHandler({ locale: 'zh-CN' });
  themeHandler({ theme: 'light' });
  assert.deepEqual(seen.slice(-2), ['locale:zh-CN', 'theme:light']);
});

test('initPlatformSync：能力缺失安全降级，不抛错', async () => {
  setWindow({ vosPlatform: { version: '0.1.0' }, isIframe: true });
  await assert.doesNotReject(() => initPlatformSync({}));
});

test('initPlatformSync：未嵌入 → null', async () => {
  setWindow({ vosPlatform: undefined, isIframe: false });
  assert.equal(await initPlatformSync({}), null);
});