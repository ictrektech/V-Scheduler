#!/usr/bin/env bash
set -euo pipefail
APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION_FILE="$APP_DIR/VERSION"
usage() { echo 'Usage: ictrek.app/scripts/update_version.sh [patch|minor|major]'; }
[[ "${1:-}" != --help && "${1:-}" != -h ]] || { usage; exit 0; }
[[ $# -le 1 ]] || { usage >&2; exit 1; }
part="${1:-patch}"
case "$part" in patch|minor|major) ;; *) usage >&2; exit 1 ;; esac
ROOT="$(git -C "$APP_DIR" rev-parse --show-toplevel)"
cd "$ROOT"
[[ -z "$(git status --porcelain)" ]] || { echo 'Commit all code changes before releasing.' >&2; exit 1; }
branch="$(git branch --show-current)"
[[ -n "$branch" ]] || { echo 'Release requires a named branch.' >&2; exit 1; }
current="$(tr -d '[:space:]' < "$VERSION_FILE")"
[[ "$current" =~ ^(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$ ]] || { echo 'Invalid VERSION.' >&2; exit 1; }
IFS=. read -r major minor patch <<< "$current"
case "$part" in patch) patch=$((patch+1));; minor) minor=$((minor+1));patch=0;; major) major=$((major+1));minor=0;patch=0;; esac
version="$major.$minor.$patch"
tag="vos-v-scheduler-v$version"
public="v$version"
for candidate in "$tag" "$public"; do
 if git rev-parse -q --verify "refs/tags/$candidate" >/dev/null; then echo "Tag already exists: $candidate" >&2; exit 1; fi
done
# A network/auth failure must not be interpreted as an absent tag.
remote_tags="$(git ls-remote --tags origin)" || { echo 'Cannot verify remote release tags.' >&2; exit 1; }
for candidate in "$tag" "$public"; do
 if printf '%s\n' "$remote_tags" | awk -v ref="refs/tags/$candidate" '$2==ref {found=1} END {exit !found}'; then echo "Remote tag already exists: $candidate" >&2; exit 1; fi
done
printf '%s\n' "$version" > "$VERSION_FILE"
git add "$VERSION_FILE"
git commit -m "chore: release VOS v-scheduler $version"
git tag "$tag"
# Branch and trigger tag are published together; never create the public tag here.
git push --atomic origin "$branch" "$tag" || { echo 'Push failed; retain the prepared commit/tag and retry that push, without bumping again.' >&2; exit 1; }
echo "Pushed $tag; check GitHub Actions and release v$version for publication results."
