#!/usr/bin/env bash
set -euo pipefail
# Compatibility entry point: Router is built into the Rust Scheduler image.
args=()
while (($#)); do
  if [[ "$1" == --component ]]; then
    [[ "${2:-}" == backend ]] || { echo "Router is part of the Scheduler backend" >&2; exit 2; }
    shift 2
  else args+=("$1"); shift; fi
done
exec "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/build_image.sh" "${args[@]}"
