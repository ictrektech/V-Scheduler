// overview.ts 纯函数单测（node:test，Node 24 原生 TS strip 运行，无需测试框架）
// 运行：node --test src/utils/overview.test.mjs
import { test } from 'node:test';
import assert from 'node:assert/strict';
import {
  clusterStatusLabel,
  aggregateClusterStatus,
  pgRoleMeta,
  etcdTopologyLabel,
  haproxyMeta,
  temporalMeta,
} from './overview.ts';

test('集群状态标签映射', () => {
  assert.equal(clusterStatusLabel('created'), '已创建');
  assert.equal(clusterStatusLabel('initialized'), '已初始化');
  assert.equal(clusterStatusLabel('creating'), 'creating');
  assert.equal(clusterStatusLabel(''), '');
});

test('聚合 cluster/status → 就绪位（缺省 false）', () => {
  const agg = aggregateClusterStatus({
    etcd_ok: true,
    cluster: { cluster_id: 'c1', cluster_name: 'demo', status: 'created', created_at: 't' },
    infra: { etcd_ok: true, postgres_ha_ok: true, updated_at: 't' },
    nodes_count: 3,
    manager_count: 2,
    worker_count: 1,
    db_ok: true,
  });
  assert.deepEqual(agg, {
    hasCluster: true,
    clusterStatusLabel: '已创建',
    etcdOk: true,
    pgOk: true,
    dbOk: true,
    nodesCount: 3,
    managerCount: 2,
    workerCount: 1,
  });
});

test('聚合：无集群 / infra 缺失 → hasCluster false / pgOk false', () => {
  const agg = aggregateClusterStatus({
    etcd_ok: false,
    cluster: null,
    infra: null,
    nodes_count: 0,
    db_ok: false,
  });
  assert.deepEqual(agg, {
    hasCluster: false,
    clusterStatusLabel: '',
    etcdOk: false,
    pgOk: false,
    dbOk: false,
    nodesCount: 0,
    managerCount: 0,
    workerCount: 0,
  });
});

test('patroni 角色元数据（主/从/未知）', () => {
  assert.deepEqual(pgRoleMeta('primary'), { text: '主', color: 'green' });
  assert.deepEqual(pgRoleMeta('master'), { text: '主', color: 'green' });
  assert.deepEqual(pgRoleMeta('replica'), { text: '从', color: 'blue' });
  assert.deepEqual(pgRoleMeta('standby'), { text: '从', color: 'blue' });
  assert.deepEqual(pgRoleMeta('unknown'), { text: 'unknown', color: 'default' });
  assert.deepEqual(pgRoleMeta(''), { text: '未知', color: 'default' });
});

test('etcd 拓扑标签（不可达/未知/节点数）', () => {
  assert.deepEqual(etcdTopologyLabel(false, 3), { text: 'etcd 不可达', color: 'red' });
  assert.deepEqual(etcdTopologyLabel(true, undefined), { text: '状态未知', color: 'default' });
  assert.deepEqual(etcdTopologyLabel(true, 3), { text: '3 节点', color: 'green' });
});
// ── 二期 S1：haproxyMeta ────────────────────────────────────────────────────
test('haproxyMeta：视图缺失（etcd 不可达）→ 未就绪', () => {
  const m = haproxyMeta(undefined);
  assert.equal(m.text, '未就绪');
  assert.equal(m.color, 'default');
  assert.equal(m.deployedCount, 0);
  assert.deepEqual(m.backends, []);
});

test('haproxyMeta：就绪（有实例 + 本机入口可达）', () => {
  const m = haproxyMeta({
    haproxy_ok: true,
    front_port: 15432,
    stats_port: 18404,
    pg_backends: ['10.0.0.1', '10.0.0.2'],
    deployed_count: 2,
    local_pg_front_reachable: true,
    patroni_leader: '10.0.0.1',
  });
  assert.equal(m.text, '就绪（2 实例）');
  assert.equal(m.color, 'green');
  assert.equal(m.frontPort, 15432);
  assert.deepEqual(m.backends, ['10.0.0.1', '10.0.0.2']);
  assert.equal(m.leader, '10.0.0.1');
});

test('haproxyMeta：有实例但本机入口不可达 → 橙', () => {
  const m = haproxyMeta({
    haproxy_ok: false,
    front_port: 15432,
    stats_port: 18404,
    pg_backends: [],
    deployed_count: 1,
    local_pg_front_reachable: false,
    patroni_leader: null,
  });
  assert.equal(m.text, '本机入口不可达');
  assert.equal(m.color, 'orange');
  assert.equal(m.localReachable, false);
});

test('haproxyMeta：零实例 → 未部署', () => {
  const m = haproxyMeta({
    haproxy_ok: false,
    front_port: 15432,
    stats_port: 18404,
    pg_backends: ['10.0.0.1'],
    deployed_count: 0,
    local_pg_front_reachable: false,
    patroni_leader: null,
  });
  assert.equal(m.text, '未部署');
  assert.equal(m.color, 'default');
});

// ── 二期 S2：temporalMeta ───────────────────────────────────────────────────
test('temporalMeta：视图缺失（etcd 不可达）→ 未就绪', () => {
  const m = temporalMeta(null);
  assert.equal(m.text, '未就绪');
  assert.equal(m.color, 'default');
  assert.equal(m.deployedCount, 0);
});

test('temporalMeta：就绪（deployed + frontend 可达）', () => {
  const m = temporalMeta({
    temporal_ok: true,
    frontend_port: 7333,
    haproxy_frontend_port: 7133,
    ui_port: 8133,
    deployed_count: 1,
    local_frontend_reachable: true,
    front_via_haproxy_reachable: true,
    db_front_reachable: true,
  });
  assert.equal(m.text, '就绪');
  assert.equal(m.color, 'green');
  assert.equal(m.frontendPort, 7333);
  assert.equal(m.haproxyFrontendPort, 7133);
  assert.equal(m.uiPort, 8133);
  assert.equal(m.viaHaproxyReachable, true);
});

test('temporalMeta：有实例但本机 frontend 不可达 → 实例异常', () => {
  const m = temporalMeta({
    temporal_ok: false,
    frontend_port: 7333,
    haproxy_frontend_port: 7133,
    ui_port: 8133,
    deployed_count: 1,
    local_frontend_reachable: false,
    front_via_haproxy_reachable: false,
    db_front_reachable: true,
  });
  assert.equal(m.text, '实例异常');
  assert.equal(m.color, 'orange');
});

test('temporalMeta：零实例 → 未部署', () => {
  const m = temporalMeta({
    temporal_ok: false,
    frontend_port: 7333,
    haproxy_frontend_port: 7133,
    ui_port: 8133,
    deployed_count: 0,
    local_frontend_reachable: false,
    front_via_haproxy_reachable: false,
    db_front_reachable: false,
  });
  assert.equal(m.text, '未部署');
  assert.equal(m.color, 'default');
});
