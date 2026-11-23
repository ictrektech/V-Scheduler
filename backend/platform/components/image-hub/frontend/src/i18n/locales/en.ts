/**
 * English language pack aggregator: namespaces mirror the zh-CN files one-to-one.
 */
import common from './en/common';
import console from './en/console';
import transfer from './en/transfer';
import gc from './en/gc';
import settings from './en/settings';
import guide from './en/guide';
import tasks from './en/tasks';
import commands from './en/commands';
import errors from './en/errors';

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