import { describe, expect, it } from 'vitest';
import {
  isVersionAtLeast,
  localizedTitle,
  pickLocalized,
  resolveMessagePath,
  supportsLocalizedOpenTabTitle,
  translateIn,
} from '../localized-title';

/** `moduleinfer-detail-tab-title-i18n` 单测：多语言标题工具与版本探测。 */
describe('resolveMessagePath / translateIn', () => {
  const messages = { routes: { nodeDetail: '节点详情' }, num: 1 };

  it('按点路径取值', () => {
    expect(resolveMessagePath(messages, 'routes.nodeDetail')).toBe('节点详情');
  });

  it('缺失 / 非字符串回退 undefined', () => {
    expect(resolveMessagePath(messages, 'routes.missing')).toBeUndefined();
    expect(resolveMessagePath(messages, 'num')).toBeUndefined();
    expect(resolveMessagePath(undefined, 'a.b')).toBeUndefined();
  });

  it('translateIn 取当前语言文案', () => {
    expect(translateIn('zh-CN', 'routes.nodeDetail')).toBe('节点详情');
    expect(translateIn('en', 'routes.nodeDetail')).toBe('Node Details');
  });

  it('translateIn 未知 key 回退 key / fallback', () => {
    expect(translateIn('en', 'routes.unknownKey')).toBe('routes.unknownKey');
    expect(translateIn('en', 'routes.unknownKey', 'X')).toBe('X');
  });
});

describe('localizedTitle', () => {
  it('遍历支持语言产出 map + 实体名后缀', () => {
    expect(localizedTitle('routes.nodeDetail', 'n1')).toEqual({
      'zh-CN': '节点详情 · n1',
      en: 'Node Details · n1',
    });
    expect(localizedTitle('routes.runDetail', 'r1')).toEqual({
      'zh-CN': '运行配置详情 · r1',
      en: 'Run Config Detail · r1',
    });
  });

  it('无后缀', () => {
    expect(localizedTitle('routes.nodes')).toEqual({
      'zh-CN': '节点管理',
      en: 'Nodes',
    });
  });
});

describe('pickLocalized', () => {
  it('字符串原样返回', () => {
    expect(pickLocalized('节点详情', 'en')).toBe('节点详情');
  });

  it('按回退链取单值', () => {
    const text = { 'zh-CN': '节点详情', en: 'Node Details' };
    expect(pickLocalized(text, 'en')).toBe('Node Details');
    expect(pickLocalized(text, 'zh-CN')).toBe('节点详情');
    expect(pickLocalized({ en: 'OnlyEn' }, 'zh-CN')).toBe('OnlyEn');
    expect(pickLocalized({ fr: 'Fr' }, 'en')).toBe('Fr');
    expect(pickLocalized({}, 'en')).toBe('');
  });
});

describe('isVersionAtLeast / supportsLocalizedOpenTabTitle', () => {
  it('版本比较', () => {
    expect(isVersionAtLeast('0.3.0', '0.4.0')).toBe(false);
    expect(isVersionAtLeast('0.4.0', '0.4.0')).toBe(true);
    expect(isVersionAtLeast('0.4.1', '0.4.0')).toBe(true);
    expect(isVersionAtLeast('0.4.0-abc', '0.4.0')).toBe(true);
    expect(isVersionAtLeast(undefined, '0.4.0')).toBe(false);
    expect(isVersionAtLeast('garbage', '0.4.0')).toBe(false);
  });

  it('能力探测', () => {
    expect(supportsLocalizedOpenTabTitle({ version: '0.4.0' })).toBe(true);
    expect(supportsLocalizedOpenTabTitle({ version: '0.3.0' })).toBe(false);
    expect(supportsLocalizedOpenTabTitle(undefined)).toBe(false);
  });
});