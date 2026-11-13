// Store only the last explicit test per model and upstream in this app tab.
const results = new Map();
const key = (upstream, model) => JSON.stringify([upstream, model]);
export function lastModelTest(upstream, model) {
  return results.get(key(upstream, model));
}
export function rememberModelTest(upstream, model, result) {
  if (!upstream || upstream === 'auto' || !model) return;
  const id = key(upstream, model);
  results.delete(id);
  results.set(id, { ...result, at: new Date().toISOString() });
  if (results.size > 200) results.delete(results.keys().next().value);
  window.dispatchEvent(new Event('vscheduler:model-test'));
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
  try { return new URL(address).host; } catch { return ''; }
}
