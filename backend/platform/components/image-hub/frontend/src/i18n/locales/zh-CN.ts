/**
 * 中文语言包聚合入口：按命名空间拆分，便于中英对等维护。
 */
import common from './zh-CN/common';
import console from './zh-CN/console';
import transfer from './zh-CN/transfer';
import gc from './zh-CN/gc';
import settings from './zh-CN/settings';
import guide from './zh-CN/guide';
import tasks from './zh-CN/tasks';
import commands from './zh-CN/commands';
import errors from './zh-CN/errors';

export default {
  ...common,
  ...console,
  ...transfer,
  ...gc,
  ...settings,
  ...guide,
  ...tasks,
  ...commands,
  ...errors,
};