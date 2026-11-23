import zhCNCommon from './zh-CN/common';
import zhCNStatus from './zh-CN/status';
import zhCNError from './zh-CN/error';
import zhCNModelFamily from './zh-CN/modelFamily';
import zhCNPlaza from './zh-CN/plaza';
import zhCNCards from './zh-CN/cards';
import zhCNNodes from './zh-CN/nodes';
import zhCNNodeDetail from './zh-CN/nodeDetail';
import zhCNRuns from './zh-CN/runs';
import zhCNRunDetail from './zh-CN/runDetail';
import zhCNSettings from './zh-CN/settings';
import zhCNMetrics from './zh-CN/metrics';

/** 中文语言包（按命名空间聚合；与 `en.ts` key 完全对等） */
export default {
  ...zhCNCommon,
  ...zhCNStatus,
  ...zhCNError,
  modelFamily: zhCNModelFamily,
  plaza: zhCNPlaza,
  cards: zhCNCards,
  nodes: zhCNNodes,
  nodeDetail: zhCNNodeDetail,
  runs: zhCNRuns,
  runDetail: zhCNRunDetail,
  settings: zhCNSettings,
  metrics: zhCNMetrics,
};