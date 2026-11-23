/**
 * Status label texts. `utils/status.ts` `statusMeta` returns `status.<kind>.<value>` keys,
 * rendered by `StatusTag.vue` via vue-i18n.
 */
export default {
  status: {
    deployment: {
      stopped: 'Stopped',
      starting: 'Starting',
      running: 'Running',
      stopping: 'Stopping',
      failed: 'Failed',
      degraded: 'Degraded',
    },
    health: {
      healthy: 'Healthy',
      unhealthy: 'Unhealthy',
      unknown: 'Unknown',
    },
    version: {
      available: 'Available',
      missing: 'Missing',
      error: 'Error',
    },
    source: {
      local: 'Local',
      plaza: 'Plaza',
    },
    vrouter: {
      registered: 'Registered',
      error: 'Error',
      disabled: 'Disabled',
      pending: 'Registering',
      unregistered: 'Unregistered',
    },
    node: {
      online: 'Online',
      offline: 'Offline',
      unknown: 'Unknown',
    },
    run: {
      succeeded: 'Succeeded',
      failed: 'Failed',
      running: 'Running',
      pending: 'Pending',
    },
    runKind: {
      start: 'Start',
      stop: 'Stop',
      health: 'Health Probe',
      deploy: 'Deploy',
      cleanup: 'Cleanup',
    },
    trigger: {
      manual: 'Manual',
      auto: 'Auto',
      scheduler: 'Scheduler',
      system: 'System',
    },
    stepState: {
      running: 'Running',
      succeeded: 'Succeeded',
      failed: 'Failed',
      skipped: 'Skipped',
    },
    orchestrator: {
      temporal: 'Temporal',
      scheduler: 'Scheduler',
    },
    usage: {
      idle: 'Idle',
      busy: 'Busy',
    },
    authMethod: {
      password: 'Password',
      key: 'Key',
    },
    gpuVendor: {
      nvidia: 'NVIDIA',
      amd: 'AMD',
      intel: 'Intel',
    },
    monitor: {
      pending: 'Not Deployed',
      deploying: 'Deploying',
      deployed: 'Deployed',
      failed: 'Failed',
    },
  },
} as const