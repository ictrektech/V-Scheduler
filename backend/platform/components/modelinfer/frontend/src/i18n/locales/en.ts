import enCommon from './en/common';
import enStatus from './en/status';
import enError from './en/error';
import enModelFamily from './en/modelFamily';
import enPlaza from './en/plaza';
import enCards from './en/cards';
import enNodes from './en/nodes';
import enNodeDetail from './en/nodeDetail';
import enRuns from './en/runs';
import enRunDetail from './en/runDetail';
import enSettings from './en/settings';
import enFileOverride from './en/fileOverride';
import enMetrics from './en/metrics';

/** English locale (aggregated by namespace; keys mirror `zh-CN.ts`). */
export default {
  ...enCommon,
  ...enStatus,
  ...enError,
  modelFamily: enModelFamily,
  plaza: enPlaza,
  cards: enCards,
  nodes: enNodes,
  nodeDetail: enNodeDetail,
  runs: enRuns,
  runDetail: enRunDetail,
  settings: enSettings,
  ...enFileOverride,
  metrics: enMetrics,
};