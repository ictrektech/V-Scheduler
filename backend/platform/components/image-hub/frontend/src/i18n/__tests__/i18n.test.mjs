// i18n 语言包一致性单测（node:test + Node 原生 TS strip）
//
// 直接导入各命名空间片段（而非聚合入口），避免 Node ESM 对无扩展名相对导入的限制。
import { test } from 'node:test';
import assert from 'node:assert/strict';

import zhCommon from '../locales/zh-CN/common.ts';
import zhConsole from '../locales/zh-CN/console.ts';
import zhTransfer from '../locales/zh-CN/transfer.ts';
import zhGc from '../locales/zh-CN/gc.ts';
import zhSettings from '../locales/zh-CN/settings.ts';
import zhGuide from '../locales/zh-CN/guide.ts';
import zhTasks from '../locales/zh-CN/tasks.ts';
import zhCommands from '../locales/zh-CN/commands.ts';
import zhErrors from '../locales/zh-CN/errors.ts';

import enCommon from '../locales/en/common.ts';
import enConsole from '../locales/en/console.ts';
import enTransfer from '../locales/en/transfer.ts';
import enGc from '../locales/en/gc.ts';
import enSettings from '../locales/en/settings.ts';
import enGuide from '../locales/en/guide.ts';
import enTasks from '../locales/en/tasks.ts';
import enCommands from '../locales/en/commands.ts';
import enErrors from '../locales/en/errors.ts';

import { errorCodeKey, resolveErrorCode } from '../error-code.ts';

const zh = {
  ...zhCommon,
  ...zhConsole,
  ...zhTransfer,
  ...zhGc,
  ...zhSettings,
  ...zhGuide,
  ...zhTasks,
  ...zhCommands,
  ...zhErrors,
};
const en = {
  ...enCommon,
  ...enConsole,
  ...enTransfer,
  ...enGc,
  ...enSettings,
  ...enGuide,
  ...enTasks,
  ...enCommands,
  ...enErrors,
};

/** 展平为点分 key 集合 */
function flattenKeys(obj, prefix = '') {
  const keys = [];
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === 'object') {
      keys.push(...flattenKeys(value, path));
    } else {
      keys.push(path);
    }
  }
  return keys;
}

/** 展平 key -> message */
function flattenMessages(obj, prefix = '', acc = {}) {
  for (const [key, value] of Object.entries(obj)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (value && typeof value === 'object') {
      flattenMessages(value, path, acc);
    } else {
      acc[path] = String(value);
    }
  }
  return acc;
}

test('中英 key 完全对等', () => {
  const zhKeys = flattenKeys(zh).sort();
  const enKeys = flattenKeys(en).sort();
  assert.deepEqual(enKeys, zhKeys);
  assert.ok(zhKeys.length > 200, `key 数量应覆盖全部界面（实际 ${zhKeys.length}）`);
});

test('错误码 20801–20812 中英均收录', () => {
  for (let code = 20801; code <= 20812; code += 1) {
    assert.equal(typeof zh.errors[code], 'string', `zh 缺少 ${code}`);
    assert.equal(typeof en.errors[code], 'string', `en 缺少 ${code}`);
  }
  assert.equal(errorCodeKey(20801), 'errors.20801');
});

test('语言包无未转义的 @ / 裸 key', () => {
  for (const [localeName, messages] of [
    ['zh-CN', flattenMessages(zh)],
    ['en', flattenMessages(en)],
  ]) {
    for (const [key, message] of Object.entries(messages)) {
      // vue-i18n 中裸 `@` 会被解析为 linked message；必须写成 {'@'}
      assert.ok(
        !message.replaceAll("{'@'}", '').includes('@'),
        `${localeName}.${key} 含未转义 @：${message}`,
      );
      assert.ok(message.trim().length > 0, `${localeName}.${key} 为空`);
    }
  }
});

test('resolveErrorCode：已知码本地化、未知码带错误码兜底', () => {
  const dictionary = { 'errors.20809': 'invalid-ref', 'errors.unknownWithCode': 'unknown:{code}' };
  const has = (key) => key in dictionary;
  const translate = (key, params) =>
    String(dictionary[key] ?? key).replace('{code}', String(params?.code ?? ''));

  assert.equal(resolveErrorCode(20809, has, translate), 'invalid-ref');
  assert.equal(resolveErrorCode(29999, has, translate), 'unknown:29999');
});