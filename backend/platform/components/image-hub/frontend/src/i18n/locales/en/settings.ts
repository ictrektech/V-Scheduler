/**
 * Registry settings namespace (en): upstream credentials and proxy configuration.
 */
export default {
  settings: {
    title: 'Registry Settings',
    tabs: {
      registry: 'Upstream Credentials',
      proxy: 'Proxy',
    },
    auth: {
      intro1:
        'Configure username/password for an upstream registry. When a pull or sync hits that host, image-hub first tries ',
      introCode: 'skopeo login',
      intro2:
        ' (login failures do not abort the pull). Passwords are stored locally on the device and are never displayed.',
      host: 'Registry',
      username: 'Username',
      password: 'Password',
      save: 'Save',
    },
    columns: {
      username: 'Username',
      password: 'Password',
      action: 'Actions',
    },
    status: {
      set: 'Set',
      unset: 'Not set',
    },
    deleteConfirm: 'Delete credentials for {host}?',
    empty: 'No upstream registry credentials configured (anonymous pulls by default)',
    proxy: {
      intro1:
        'When enabled, the following proxy environment variables are injected into the skopeo process during pulls/syncs; the local registry and loopback addresses are added to ',
      introCode: 'no_proxy',
      intro2: ' automatically.',
      enable: 'Enable Proxy',
      save: 'Save Proxy Settings',
    },
    messages: {
      loadFailed: 'Failed to load settings: {error}',
      missingFields: 'Enter the registry address and username',
      saved: 'Saved (passwords are never displayed)',
      saveFailed: 'Save failed: {error}',
      deleted: 'Credentials for {host} deleted',
      deleteFailed: 'Delete failed: {error}',
      proxySaved: 'Proxy settings saved',
    },
  },
};