/**
 * Garbage collection namespace (en): GC modal, interval presets and unavailability reasons.
 */
export default {
  gc: {
    title: 'Garbage Collection',
    unavailable: {
      title: 'Garbage collection is unavailable',
      registry_binary_missing:
        'The registry binary is unavailable: use the built-in image or configure --registry-bin',
      storage_dir_missing:
        'The registry storage directory does not exist: mount the registry storage volume and configure --registry-storage-dir',
      fallback: 'The registry binary or storage directory is unavailable',
    },
    busy: 'A garbage collection task is already running',
    storage: 'Storage Used',
    storageDir: 'Storage Directory',
    lastRun: 'Last Run',
    nextRun: 'Next Automatic Run',
    intro:
      'Garbage collection removes image layers (blobs) and untagged manifests that are no longer referenced by any tag, freeing registry storage. The registry keeps serving during collection; run it during low-activity periods to avoid competing with external image pushes.',
    auto: {
      title: 'Automatic Collection',
      enable: 'Enable Automatic Collection',
      enableHint: 'Disabled by default; once enabled it runs at the configured interval (not immediately)',
      interval: 'Interval',
      deleteUntagged: 'Delete Untagged Images',
      deleteUntaggedHint:
        'Reclaim untagged manifests and layers left behind by renames / tag removals',
      current: 'Current: {interval}',
      save: 'Save Automatic Settings',
    },
    manual: {
      title: 'Run Now',
      deleteUntagged: 'Delete Untagged Images',
      dryRun: 'Dry Run',
      dryRunHint: 'Only count what would be deleted; no data is actually removed',
      run: 'Run Garbage Collection',
      runDry: 'Start Dry Run',
    },
    intervals: {
      h1: 'Every hour',
      h3: 'Every 3 hours',
      h6: 'Every 6 hours',
      h12: 'Every 12 hours',
      h24: 'Every 24 hours (default)',
      d3: 'Every 3 days',
      d7: 'Every 7 days',
    },
    interval: {
      days: '{count} day(s)',
      hours: '{count} hour(s)',
    },
    nextRunText: {
      disabled: 'Automatic collection disabled',
      at: 'Expected {time}',
      pending: 'Starts counting after enabling',
    },
    messages: {
      loadFailed: 'Failed to load garbage collection status: {error}',
      invalidInterval: 'The interval must be an integer between 1 and 720 hours',
      savedEnabled: 'Automatic collection enabled',
      savedDisabled: 'Automatic collection settings saved (disabled)',
      saveFailed: 'Save failed: {error}',
      startedDry: 'Dry run started (no data will be deleted)',
      started: 'Garbage collection started',
      triggerFailed: 'Failed to trigger: {error}',
    },
    result: {
      dryHeadline: 'Dry run (nothing deleted)',
      doneHeadline: 'Collection completed',
      textDry: 'Would delete {blobs} blobs / {manifests} manifests',
      text: 'Deleted {blobs} blobs / {manifests} manifests',
      marked: '{count} blobs still referenced',
      freed: 'Freed {size}',
    },
  },
};