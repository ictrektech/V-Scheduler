import {CAPABILITIES} from './model-options.js';
const capabilityLabels=Object.fromEntries(CAPABILITIES);
// Presentation only: retain every model, upstream identity and existing API contract.
export function sourceLabel(source) {
  return ({api: '外部上游', registered: '主动注册', 'v-scheduler': 'V-Scheduler',
    'v-voice': 'V-Voice', 'image-studio': 'Image Studio', 'ml-server': 'ML Server',
    'v-sub': 'V-Sub', 'video-structure': '视频结构化', 'model-hub': 'Model Hub'})[source] || source || '其他来源';
}

export function groupCatalog(upstreams, models, mode = 'upstream', query = '', translate = label=>label) {
  const entries = new Map(upstreams.map(u => [u.id, {...u, catalogModels: []}]));
  for (const m of models) {
    const id = m.upstream_id || 'catalog-aliases';
    if (!entries.has(id)) entries.set(id, {id, name: m.upstream_name || '快捷模型',
      source: m.upstream_source || 'aliases', base_url: m.upstream_url || '', manageable: false, catalogModels: []});
    entries.get(id).catalogModels.push(m);
  }
  const needle = query.trim().toLocaleLowerCase();
  const groups = new Map();
  for (const u of entries.values()) {
    const matchesSource = [u.name, u.base_url, u.id, sourceLabel(u.source),translate(sourceLabel(u.source))].join(' ').toLocaleLowerCase().includes(needle);
    const rows = !needle || matchesSource ? u.catalogModels : u.catalogModels.filter(m =>
      [m.id, m.qualified, m.kind, ...(m.capabilities || []).flatMap(c=>[c,capabilityLabels[c]||'',translate(capabilityLabels[c]||c)])].join(' ').toLocaleLowerCase().includes(needle));
    if (needle && !matchesSource && !rows.length) continue;
    const key = mode === 'application' ? (u.source === 'registered' && u.app_id ? u.app_id : u.source || 'other') : u.id;
    if (!groups.has(key)) groups.set(key, {key: mode + ':' + key,
      name: mode === 'application' ? sourceLabel(key) : u.name || u.id, upstreams: [], count: 0});
    const group = groups.get(key);
    group.upstreams.push({...u, catalogModels: rows});
    group.count += rows.length;
  }
  return [...groups.values()];
}
