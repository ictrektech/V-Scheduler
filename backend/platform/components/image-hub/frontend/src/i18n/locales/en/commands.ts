/**
 * Command generation namespace (en): manual push/pull step titles (commands stay language-neutral).
 */
export default {
  commands: {
    pull: {
      socat:
        'Map remote {ip}:{port} to local port {port} (docker treats 127.0.0.1 as HTTP by default, no HTTPS configuration needed)',
      pull: 'Pull the image from the relay registry',
      tag: 'Remove the prefix and restore the upstream image name',
      rmi: 'Remove the relay registry image',
      stop: 'Stop the background socat port mapping (avoid holding the local port)',
    },
    push: {
      socat: 'Map remote {ip}:{port} to local port {port}',
      tag: 'Tag the upstream image for the relay registry (keep the full upstream name, e.g. docker.io/foo/bar)',
      push: 'Push the image to the relay registry',
      stop: 'Stop the background socat port mapping (avoid holding the local port)',
    },
  },
};