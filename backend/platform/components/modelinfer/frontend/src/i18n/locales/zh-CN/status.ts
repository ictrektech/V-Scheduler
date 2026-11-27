/**
 * 状态标签文案。`utils/status.ts` 的 `statusMeta` 返回 `status.<kind>.<value>` key，
 * `StatusTag.vue` 经 vue-i18n 渲染。
 */
export default {
  status: {
    deployment: {
      stopped: '已停止',
      starting: '启动中',
      running: '运行中',
      stopping: '停止中',
      failed: '失败',
      degraded: '降级',
    },
    health: {
      healthy: '健康',
      unhealthy: '异常',
      unknown: '未知',
    },
    version: {
      available: '可用',
      missing: '缺失',
      error: '异常',
    },
    source: {
      local: '本地',
      plaza: '广场',
    },
    vrouter: {
      registered: '已注册',
      error: '错误',
      disabled: '已禁用',
      pending: '注册中',
      unregistered: '未注册',
    },
    node: {
      online: '在线',
      offline: '离线',
      unknown: '未知',
    },
    run: {
      succeeded: '成功',
      failed: '失败',
      running: '进行中',
      pending: '等待中',
    },
    runKind: {
      start: '启动',
      stop: '停止',
      health: '健康巡检',
      deploy: '部署',
      cleanup: '清理',
    },
    trigger: {
      manual: '手动',
      auto: '自动',
      scheduler: '调度器',
      system: '系统',
    },
    stepState: {
      running: '进行中',
      succeeded: '成功',
      failed: '失败',
      skipped: '跳过',
    },
    orchestrator: {
      temporal: 'Temporal',
      scheduler: '调度器',
    },
    usage: {
      idle: '空闲',
      busy: '占用',
    },
    authMethod: {
      password: '密码',
      key: '密钥',
      local: '本机',
    },
    gpuVendor: {
      nvidia: 'NVIDIA',
      amd: 'AMD',
      intel: 'Intel',
    },
    monitor: {
      pending: '未部署',
      deploying: '部署中',
      deployed: '已部署',
      failed: '失败',
    },
  },
} as const