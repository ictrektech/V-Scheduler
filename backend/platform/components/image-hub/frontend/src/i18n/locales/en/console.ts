/**
 * Console namespace (en): image list and image detail pages.
 */
export default {
  images: {
    title: 'Image Registry',
    subtitle:
      'Images already uploaded to the local offline registry. Pull images from upstream, sync updates and manage tags here; click a repository name to open its details.',
    actions: {
      pull: 'Pull Image',
      import: 'Import Image',
      gc: 'Garbage Collection',
      settings: 'Registry Settings',
      guide: 'Access Guide',
      tasks: 'Tasks',
      tasksWithCount: 'Tasks ({count})',
      refresh: 'Refresh',
    },
    registry: {
      label: 'Registry Address',
      hint: ' (host-network HTTP, no auth; docker clients connect directly)',
      placeholder: 'Select a NIC address',
      copy: 'Copy',
      copySuccess: 'Registry address copied',
      note: 'Tip: the device may have multiple NIC addresses. Choose the one reachable from the image distribution target network.',
    },
    search: {
      placeholder: 'Search repositories (e.g. docker.io/library)',
      count: 'Repositories: {count}',
    },
    columns: {
      repository: 'Repository',
      tags: 'Tags',
      size: 'Size',
      created: 'Created',
      action: 'Actions',
    },
    row: {
      tagCount: '{count} tag(s)',
      detail: 'Details',
      sync: 'Sync',
      clone: 'Clone',
      rename: 'Rename',
      deleteRepo: 'Delete All',
    },
    confirm: {
      deleteRepo: 'Delete all tags of repository {repository}?',
    },
    empty:
      'No images yet. Click "Pull Image" to pull from upstream, or follow the "Access Guide" to push images to this registry.',
    messages: {
      loadFailed: 'Failed to load images: {error}',
      deleteSuccess: 'All tags of repository {repository} deleted',
      deleteFailed: 'Delete failed: {error}',
      cancelRequested: 'Stop requested. Waiting for the task to finish.',
      cancelFailed: 'Failed to stop: {error}',
    },
  },
  detail: {
    titleFallback: 'Image Detail',
    subtitle: 'Local reference {registry} / {count} tag(s) / {size} total',
    actions: {
      back: 'Back to List',
      guide: 'Access Guide',
      tasks: 'Tasks',
      tasksWithCount: 'Tasks ({count})',
      syncAll: 'Sync All',
      cloneRepo: 'Clone Repository',
      renameRepo: 'Rename Repository',
      deleteRepo: 'Delete All',
      refresh: 'Refresh',
    },
    upstream: {
      label: 'Upstream Image',
      copy: 'Copy',
      copySuccess: 'Image name copied',
      hint1:
        'The repository name in the registry is the full upstream image name; after downloading, it can be restored to ',
      hint2: '.',
    },
    columns: {
      tag: 'Tag',
      digest: 'Digest',
      size: 'Size',
      platforms: 'Platforms',
      created: 'Created',
      action: 'Actions',
    },
    row: {
      copyDeploymentAddress: 'Copy deployment image address',
      download: 'Download Command',
      sync: 'Sync',
      clone: 'Clone',
      rename: 'Rename',
      delete: 'Delete',
    },
    confirm: {
      deleteTag: 'Delete {repository}:{tag}?',
      deleteRepo: 'Delete all tags of repository {repository}?',
    },
    expand: {
      digest: 'Digest',
      size: 'Size',
      created: 'Created',
      mediaType: 'Media Type',
      platforms: 'Platform Manifests',
      singlePlatform: ' (single platform)',
    },
    platformCount: '{count} platform(s)',
    empty: 'This image has no tags',
    messages: {
      loadFailed: 'Failed to load image details: {error}',
      deleteTagSuccess: 'Deleted {repository}:{tag}',
      deleteRepoSuccess: 'All tags of repository {repository} deleted',
      deleteFailed: 'Delete failed: {error}',
      cancelRequested: 'Stop requested. Waiting for the task to finish.',
      cancelFailed: 'Failed to stop: {error}',
    },
    download: {
      title: 'Download Image {repository}:{tag}',
      description:
        'The one-liner is recommended (it creates and cleans up socat automatically and restores the upstream image name after pulling). You may also run the manual steps (the last step stops socat).',
      oneLinerHint: 'Recommended: one-liner download (auto socat setup/cleanup, restores upstream name)',
    },
  },
};