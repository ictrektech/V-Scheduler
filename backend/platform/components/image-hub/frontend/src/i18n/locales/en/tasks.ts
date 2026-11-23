/**
 * Tasks namespace (en): task drawer (status/phase/title/counts/failure/GC summary/logs).
 */
export default {
  tasks: {
    title: 'Tasks',
    refresh: 'Refresh',
    empty:
      'No tasks yet. Click "Pull Image", "Garbage Collection" or "Sync" on an image, then track progress here.',
    status: {
      pending: 'Pending',
      running: 'Running',
      succeeded: 'Succeeded',
      failed: 'Failed',
      cancelled: 'Cancelled',
    },
    phase: {
      queued: 'Queued',
      queuedMany: 'Queued ({count} tag(s))',
      login: 'Upstream Login',
      loginRunning: 'Logging in to the upstream registry',
      preparing: 'Fetching Manifest',
      preparingRunning: 'Fetching the image manifest',
      copying: 'Copying Layers',
      copyingSub: 'Copying image — sub-image {current}/{total}',
      copyingSubLayers: 'Copying image — sub-image {current}/{total} ({layers} layers copied)',
      copyingLayers: 'Copying layers ({count} copied)',
      writing: 'Writing Manifest',
      writingRunning: 'Writing the manifest',
      storing: 'Writing Signatures',
      gc: 'Scanning & Collecting',
      gcStart: 'Scanning and collecting unreferenced image layers…',
      gcDryStart: 'Running a garbage collection dry run (no data will be deleted)…',
      gcProgress: 'Garbage collection in progress ({blobs} blobs / {manifests} manifests deleted)',
      done: 'Completed',
      doneRunning: 'Task completed',
      failed: 'Failed',
      failedRunning: 'Task failed',
      cancelled: 'Cancelled',
      cancelledRunning: 'Task cancelled',
      running: 'Running',
      stopping: 'Stopping…',
    },
    attempt: {
      running: 'Attempt {attempt}/{max}',
      retry: 'Attempt {attempt}/{max} failed; retrying in {seconds}s',
      retrySoon: 'Attempt {attempt}/{max} failed; retrying soon',
    },
    kind: {
      pull: 'Pull',
      sync: 'Sync',
      clone: 'Clone',
      rename: 'Rename',
      import: 'Import',
      gc: 'Garbage Collection',
      task: 'Task',
    },
    titles: {
      pull: 'Pull {ref}',
      syncOne: 'Sync {ref}',
      syncMany: 'Sync {ref} ({count} tag(s))',
      clone: 'Clone {source} → {dest}',
      rename: 'Rename {source} → {dest}',
      importOne: 'Import {file} → {dest}',
      importMany: 'Import {file} ({count} name(s))',
      gc: 'Garbage Collection',
      gcDry: 'Garbage Collection (dry run)',
    },
    counts: {
      items: 'Tags {done}/{total}',
      sub: 'Sub-images {current}/{total}',
      layers: '{count} layers copied',
    },
    action: {
      stop: 'Stop',
      stopping: 'Stopping…',
      clean: 'Clean Up',
    },
    confirm: {
      stop: 'Stop this task? Transferred data is not rolled back, but no new tags will be created.',
      stopOk: 'Stop',
      clean: 'Remove this task record?',
      cleanOk: 'Remove',
    },
    messages: {
      cleaned: 'Task record removed',
    },
    meta: {
      mirror: 'Mirror: {mirror}',
      arch: 'Architecture: {arch}',
      timeout: 'Timeout: {timeout}',
      retry: 'Retries: {retry}',
      created: 'Created: {time}',
      finished: 'Finished: {time}',
      storageDir: 'Storage directory: {dir}',
    },
    refs: {
      source: 'Source',
      dest: 'Destination',
    },
    logs: {
      header: 'Detailed logs ({count} lines, for troubleshooting)',
    },
    hint: {
      timeout:
        'Transfer timed out: increase the timeout (or choose "Unlimited") and/or raise the retry count when starting the task again',
      transferFailed:
        'Transfer failed: increase the timeout / retry count, or check the upstream address and network connectivity (you can also switch mirrors)',
      retries:
        'Still failing after several attempts: check the upstream address and network connectivity, or switch to another mirror and retry',
    },
  },
};