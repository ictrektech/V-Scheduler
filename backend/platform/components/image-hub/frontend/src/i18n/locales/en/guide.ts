/**
 * Access guide namespace (en): upload/download commands, one-liner/manual modes and principles.
 */
export default {
  guide: {
    title: 'Access Guide',
    intro:
      'Relay third-party images through this registry: when pushing, keep the full upstream image name; when pulling, use socat + 127.0.0.1 to avoid HTTPS and retag to restore the upstream name. The image-hub.sh one-liner is recommended (it creates and cleans up socat automatically).',
    address: 'Registry Address',
    addressHint: ' (host-network HTTP, no auth)',
    chooseAddress: 'Select a NIC address',
    copy: 'Copy',
    upstream: 'Upstream Image',
    upstreamPlaceholder: 'docker.io/foo/bar:tag',
    invalid: 'Expected repository:tag, e.g. docker.io/foo/bar:tag (or alpine:3)',
    normalized:
      'Normalized image name: {ref}; relay reference after pushing to this registry: {local} (the repository name stored in the registry is the full upstream name {repository})',
    upload: {
      title: '1. Push Image (run on the image source machine)',
      oneTitle:
        'One-liner push (the script sets up socat, tags, pushes and cleans up socat automatically)',
    },
    download: {
      title: '2. Pull Image (run on the target machine; the name matches upstream after pulling)',
      oneTitle:
        'One-liner pull (the script sets up socat, pulls, restores the upstream name and cleans up socat automatically)',
    },
    mode: {
      one: 'One-liner (recommended)',
      manual: 'Manual',
    },
    noCommand: 'Enter a valid upstream image name and select a registry address',
    downloadAlt: 'Or download and run it:',
    prereq:
      'The target machine needs docker and socat; the script does not hard-code addresses; pass one via --ih-addr.',
    copyAll: 'Copy All',
    manualHint: 'Run the steps manually (the last step stops the background socat process)',
    principles: {
      title: 'How It Works & Notes',
      colon: ': ',
      name: {
        name: 'Name Preservation',
        socat: 'socat and HTTPS-free access',
        script: 'Getting the Script',
        delete: 'Deletion and Garbage Collection',
        security: 'Security Boundary',
      },
      nameItem:
        'When pushing, the local image name is exactly the repository name stored in the registry. Keep the full upstream name {upstream} as one segment of the local name (<registry-ip>:{port}/{upstream}:tag); the repository name in the registry then naturally equals the upstream name and the tag is unchanged.',
      socatItem:
        'docker treats 127.0.0.1:<port> as HTTP by default and requires HTTPS for other host names. The one-liner script (image-hub.sh) picks a free local port for the socat mapping and cleans it up on exit (including signal interruptions), backing off on port conflicts. If you run the steps manually, always run the last step to stop the background socat. If the target machine is this device, use the registry address directly without socat.',
      scriptItem:
        '{command} or pipe it directly; the script only depends on bash / docker / socat and does not hard-code addresses. (The console is served over HTTPS by Traefik with a self-signed certificate, so curl uses -k to skip certificate verification.)',
      deleteItem:
        'The registry only supports deleting by manifest digest; deleted image layers (blobs) are freed only after registry garbage collection (GC).',
      securityItem:
        'The registry is HTTP and unauthenticated, suitable only for trusted internal networks. Do not expose it to untrusted networks.',
    },
  },
};