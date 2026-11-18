// Keep the latest explicit test per model and upstream within one gateway identity.
const LIMIT = 200;
const PREFIX = 'vscheduler:model-tests:v1:';
let cachedScope;
let cachedResults;
const key = (upstream, model) => JSON.stringify([upstream, model]);
export function modelTestScope(apiBase, auth) {
  const sub = auth?.identity?.sub;
  return typeof sub === 'string' && sub && auth.auth_method && auth.auth_method !== 'anonymous'
    ? PREFIX + JSON.stringify([apiBase, auth.auth_method, sub]) : undefined;
}
function resultsFor(scope) {
  if (scope === cachedScope && cachedResults) return cachedResults;
  let entries = [];
  try {
    const stored = JSON.parse(window.localStorage.getItem(scope) || '[]');
    if (Array.isArray(stored)) entries = stored.slice(-LIMIT).filter(entry =>
      Array.isArray(entry) && typeof entry[0] === 'string' && entry[1] &&
      Object.hasOwn(resultLabels, entry[1].outcome) && Object.hasOwn(reachabilityLabels, entry[1].reachability) &&
      typeof entry[1].at === 'string' && Number.isFinite(Date.parse(entry[1].at)));
  } catch {
    // Browser policy, quota or malformed storage can leave this session in memory only.
  }
  cachedScope = scope;
  cachedResults = new Map(entries);
  return cachedResults;
}
export function lastModelTest(scope, upstream, model) {
  return scope ? resultsFor(scope).get(key(upstream, model)) : undefined;
}
export function rememberModelTest(scope, upstream, model, result, address) {
  if (!scope || !upstream || upstream === 'auto' || !model ||
      !Object.hasOwn(resultLabels, result.outcome) || !Object.hasOwn(reachabilityLabels, result.reachability)) return;
  const results = resultsFor(scope);
  const id = key(upstream, model);
  results.delete(id);
  results.set(id, { outcome: result.outcome, reachability: result.reachability,
    host: upstreamHost(address), at: new Date().toISOString() });
  if (results.size > LIMIT) results.delete(results.keys().next().value);
  try { window.localStorage.setItem(scope, JSON.stringify([...results])); } catch { /* Keep the current page usable. */ }
  window.dispatchEvent(new Event('vscheduler:model-test'));
}

export function executionLocation(upstream, vosHost) {
  if (upstream?.source === 'v-scheduler' && ['qa', 'embedding', 'rerank']
      .some(role => upstream.id === 'v-scheduler/ollama-' + role)) {
    return { label: '本机 VOS', host: vosHost };
  }
  const host = upstreamHost(upstream?.base_url);
  return host ? { label: '上游主机', host } : { label: '执行位置未确认', host: '' };
}

export function classifyModelTest(status, payload, upstreamResponded) {
  const raw = payload?.error;
  const code = String(raw?.code || payload?.code || '').toLowerCase();
  const message = String(raw?.message || (typeof raw === 'string' ? raw : '')).toLowerCase();
  if (code === 'upstream_network_unreachable') return { outcome: 'network', reachability: 'unreachable' };
  if (code === 'upstream_auth_failed') return { outcome: 'auth', reachability: 'reachable' };
  if (code === 'upstream_request_timeout' || (upstreamResponded && [408, 504].includes(status))) {
    return { outcome: 'timeout', reachability: upstreamResponded ? 'reachable' : 'unknown' };
  }
  if (upstreamResponded && [401, 403].includes(status)) return { outcome: 'auth', reachability: 'reachable' };
  if (upstreamResponded && (['model_not_loaded', 'model_not_running'].includes(code)
      || /\bmodel\b.{0,100}\b(?:not loaded|not running|unloaded)\b/.test(message))) {
    return { outcome: 'unloaded', reachability: 'reachable' };
  }
  return { outcome: 'failed', reachability: upstreamResponded ? 'reachable' : 'unknown' };
}
export const resultLabels = {
  passed: '测试通过', network: '网络不可达', auth: '鉴权失败', unloaded: '模型未加载',
  timeout: '请求超时', failed: '测试未通过',
};
export const reachabilityLabels = { reachable: '可达', unreachable: '不可达', unknown: '未确认' };
export function upstreamHost(address) {
  try {
    const url = new URL(address);
    return ['http:', 'https:'].includes(url.protocol) ? url.host : '';
  } catch { return ''; }
}
