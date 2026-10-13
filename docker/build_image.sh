#!/usr/bin/env bash
set -euo pipefail
# Reuses the proven V-Router Feishu read/write helpers. Run on the build host.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
FEISHU_CONFIG_FILE="${HOME}/.feishu.json"
FEISHU_SPREADSHEET_TOKEN="Htotsn3oahO1zxt73YMcaB1zn8e"
REPOSITORY="swr.cn-southwest-2.myhuaweicloud.com/ictrek/v-scheduler"
COMPONENT_NAME="v-scheduler"
log() { echo "[INFO] $*"; }
err() { echo "[ERROR] $*" >&2; }
die() { err "$*"; exit 1; }

require_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing command: $1"
}

contains() {
  local needle="$1"
  shift
  local item
  for item in "$@"; do
    [[ "$item" == "$needle" ]] && return 0
  done
  return 1
}

read_feishu_field() {
  local field="$1"
  python3 - "$FEISHU_CONFIG_FILE" "$field" <<'PY'
import json, sys
path, field = sys.argv[1], sys.argv[2]
with open(path, "r", encoding="utf-8") as f:
    data = json.load(f)
val = data.get(field, "")
if not isinstance(val, str):
    val = str(val)
print(val)
PY
}

feishu_api_json() {
  local method="$1"
  local url="$2"
  local token="$3"
  local body="${4:-}"

  if [[ -n "$body" ]]; then
    curl --fail --connect-timeout 10 --max-time 60 -sS -X "$method" "$url" \
      -H "Authorization: Bearer ${token}" \
      -H "Content-Type: application/json" \
      --data "$body"
  else
    curl --fail --connect-timeout 10 --max-time 60 -sS -X "$method" "$url" \
      -H "Authorization: Bearer ${token}"
  fi
}

get_feishu_token() {
  local app_id="$1"
  local app_secret="$2"
  local resp

  resp=$(
    curl --fail --connect-timeout 10 --max-time 60 -sS -X POST "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal" \
      -H "Content-Type: application/json" \
      -d "{\"app_id\":\"${app_id}\",\"app_secret\":\"${app_secret}\"}"
  ) || return 1

  python3 - "$resp" <<'PY'
import json, sys
resp = sys.argv[1]
if not resp:
    raise SystemExit("empty token response")
data = json.loads(resp)
if data.get("code") != 0:
    raise SystemExit("get_feishu_token failed")
print(data["tenant_access_token"])
PY
}

get_sheet_id_by_title() {
  local token="$1"
  local spreadsheet_token="$2"
  local target_title="$3"
  local resp

  resp=$(feishu_api_json "GET" \
    "https://open.feishu.cn/open-apis/sheets/v3/spreadsheets/${spreadsheet_token}/sheets/query" \
    "$token")

  python3 - "$target_title" "$resp" <<'PY'
import json, sys
target, resp = sys.argv[1], sys.argv[2]
data = json.loads(resp)
if data.get("code") != 0:
    raise SystemExit("query sheets failed")
for sheet in data["data"]["sheets"]:
    if sheet.get("title") == target:
        print(sheet["sheet_id"])
        raise SystemExit(0)
raise SystemExit(f"sheet title not found: {target}")
PY
}

get_range_values() {
  local token="$1"
  local spreadsheet_token="$2"
  local range="$3"

  feishu_api_json "GET" \
    "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${spreadsheet_token}/values/${range}" \
    "$token"
}

find_component_column_letter() {
  local token="$1"
  local spreadsheet_token="$2"
  local sheet_id="$3"
  local component_name="$4"
  local resp result status value cell resp2 meta_resp column_count

  resp=$(get_range_values "$token" "$spreadsheet_token" "${sheet_id}!A1:ZZ2")

  result=$(python3 - "$component_name" "$resp" <<'PYFIND'
import json, sys

target = sys.argv[1]
resp = sys.argv[2]
data = json.loads(resp)
if data.get("code") != 0:
    raise SystemExit("read header failed")
values = data.get("data", {}).get("valueRange", {}).get("values", [])
rows = values[:2]
header = rows[0] if rows else []
repo = rows[1] if len(rows) > 1 else []

def text(v):
    if v is None:
        return ""
    if isinstance(v, str):
        return v.strip()
    if isinstance(v, dict):
        return str(v.get("text") or v.get("link") or "").strip()
    if isinstance(v, list):
        return "".join(text(x) for x in v).strip()
    return str(v).strip()

def col(n):
    s = ""
    while n > 0:
        n, r = divmod(n - 1, 26)
        s = chr(ord("A") + r) + s
    return s

for i, v in enumerate(header, start=1):
    if text(v) == target:
        print(f"FOUND\t{col(i)}")
        raise SystemExit(0)

last = 1
for row in (header, repo):
    for i, v in enumerate(row, start=1):
        if text(v):
            last = max(last, i)
print(f"MISSING\t{last}")
PYFIND
  )

  status="${result%%$'\t'*}"
  value="${result#*$'\t'}"
  if [[ "$status" == "FOUND" ]]; then
    echo "$value"
    return 0
  fi
  if [[ "$status" != "MISSING" ]]; then
    err "find_component_column_letter: unexpected result: $result"
    return 1
  fi

  meta_resp=$(feishu_api_json "GET" \
    "https://open.feishu.cn/open-apis/sheets/v3/spreadsheets/${spreadsheet_token}/sheets/query" \
    "$token") || {
    err "query sheet metadata failed"
    return 1
  }

  column_count=$(python3 - "$sheet_id" "$meta_resp" <<'PYSHEET'
import json, sys
sheet_id, resp = sys.argv[1], sys.argv[2]
data = json.loads(resp)
if data.get("code") != 0:
    raise SystemExit("query sheets failed")
for sheet in data["data"]["sheets"]:
    if sheet.get("sheet_id") == sheet_id:
        print(sheet.get("grid_properties", {}).get("column_count", 0))
        raise SystemExit(0)
raise SystemExit(f"sheet id not found: {sheet_id}")
PYSHEET
  )

  if (( value >= column_count )); then
    resp2=$(feishu_api_json "POST" \
      "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${spreadsheet_token}/dimension_range" \
      "$token" \
      "{\"dimension\":{\"sheetId\":\"${sheet_id}\",\"majorDimension\":\"COLUMNS\",\"length\":1}}") || {
      err "append component column failed"
      return 1
    }
  else
    resp2=$(feishu_api_json "POST" \
      "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${spreadsheet_token}/insert_dimension_range" \
      "$token" \
      "{\"dimension\":{\"sheetId\":\"${sheet_id}\",\"majorDimension\":\"COLUMNS\",\"startIndex\":${value},\"endIndex\":$((value + 1))},\"inheritStyle\":\"BEFORE\"}") || {
      err "insert component column failed"
      return 1
    }
  fi

  python3 - "$resp2" <<'PYCHECK'
import json, sys
resp = sys.argv[1]
data = json.loads(resp)
if data.get("code") != 0:
    raise SystemExit("add component column failed")
PYCHECK

  cell=$(python3 - "$value" <<'PYCOL'
import sys
n = int(sys.argv[1]) + 1
s = ""
while n > 0:
    n, r = divmod(n - 1, 26)
    s = chr(ord("A") + r) + s
print(s)
PYCOL
  )

  write_cell "$token" "$spreadsheet_token" "$sheet_id" "${cell}1" "$component_name"
  echo "$cell"
}

find_date_row() {
  local token="$1"
  local spreadsheet_token="$2"
  local sheet_id="$3"
  local target_date="$4"
  local resp

  resp=$(get_range_values "$token" "$spreadsheet_token" "${sheet_id}!A4:A2000")

  python3 - "$target_date" "$resp" <<'PY'
import json, sys
target, resp = sys.argv[1], sys.argv[2]
data = json.loads(resp)
if data.get("code") != 0:
    raise SystemExit("read date column failed")
values = data.get("data", {}).get("valueRange", {}).get("values", [])
for row_num, row in enumerate(values, start=4):
    value = str(row[0]).strip() if row else ""
    if len(value) == 8 and value.isdigit() and value == target:
        print(row_num)
        raise SystemExit(0)
print("")
PY
}

prepend_date_row() {
  local token="$1" spreadsheet_token="$2" sheet_id="$3" today="$4" resp
  resp="$(feishu_api_json POST "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${spreadsheet_token}/insert_dimension_range" "$token" "{\"dimension\":{\"sheetId\":\"${sheet_id}\",\"majorDimension\":\"ROWS\",\"startIndex\":3,\"endIndex\":4},\"inheritStyle\":\"AFTER\"}")"
  printf '%s' "$resp" | python3 -c 'import sys,json; assert json.load(sys.stdin).get("code")==0, "date row insertion failed"'
  write_cell "$token" "$spreadsheet_token" "$sheet_id" A4 "$today"
}

write_cell() {
  local token="$1"
  local spreadsheet_token="$2"
  local sheet_id="$3"
  local cell="$4"
  local value="$5"
  local resp

  resp=$(feishu_api_json "PUT" \
    "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${spreadsheet_token}/values" \
    "$token" \
    "{\"valueRange\":{\"range\":\"${sheet_id}!${cell}:${cell}\",\"values\":[[\"${value}\"]]}}")

  python3 - "$resp" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
if data.get("code") != 0:
    raise SystemExit("write cell failed")
PY
}


usage() { echo 'Usage: docker/build_image.sh --sheet AMD_with_cuda|ARM_with_cuda [--dry-run]'; }
sheet=""; dry_run=false
while (($#)); do
 case "$1" in
  --sheet) [[ $# -ge 2 ]] || die 'missing sheet'; [[ -z "$sheet" ]] || die 'one sheet per build invocation'; sheet="$2"; shift 2 ;;
  --dry-run) dry_run=true; shift ;;
  -h|--help) usage; exit 0 ;;
  *) usage >&2; exit 1 ;;
 esac
done
case "$sheet" in
 AMD_with_cuda) profile=amd; arch=amd64 ;;
 ARM_with_cuda) profile=arm; arch=arm64 ;;
 *) die 'select an explicit supported sheet; no architecture auto-detection' ;;
esac
DATE="$(TZ=Asia/Shanghai date +%Y%m%d)"
TAG="${profile}_${DATE}"
IMAGE="${REPOSITORY}:${TAG}"
BUILD_DIR="${HOME}/dev/build/V-Scheduler/images/${profile}"
if [[ "$dry_run" == true ]]; then
 printf 'profile=%s platform=linux/%s component=%s image=%s sheet=%s build=%s
' "$profile" "$arch" "$COMPONENT_NAME" "$IMAGE" "$sheet" "$BUILD_DIR"
 exit 0
fi
for cmd in docker curl python3 tar; do require_cmd "$cmd"; done
[[ -f "$FEISHU_CONFIG_FILE" ]] || die 'missing write-capable ~/.feishu.json'
case "$(uname -m):$arch" in x86_64:amd64|aarch64:arm64|arm64:arm64) ;; *) die 'use a build host matching the selected profile';; esac
mkdir -p "$BUILD_DIR"
rm -rf "$BUILD_DIR/context"
mkdir -p "$BUILD_DIR/context"
# Local staging only; no Git, SSH or rsync inside the build script.
tar --exclude=.git --exclude=node_modules --exclude=target --exclude=dist --exclude='*.local.json' --exclude=.env -cf - -C "$ROOT_DIR" . | tar -xf - -C "$BUILD_DIR/context"
FEISHU_APP_ID="$(read_feishu_field feishu_app_id)"
FEISHU_APP_SECRET="$(read_feishu_field feishu_app_secret)"
[[ -n "$FEISHU_APP_ID" && -n "$FEISHU_APP_SECRET" ]] || die 'missing Feishu credentials'
FEISHU_TOKEN="$(get_feishu_token "$FEISHU_APP_ID" "$FEISHU_APP_SECRET")"
SHEET_ID="$(get_sheet_id_by_title "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$sheet")"
docker buildx build --platform "linux/${arch}" --load --provenance=false --sbom=false -f "$BUILD_DIR/context/docker/Dockerfile" -t "$IMAGE" "$BUILD_DIR/context"
# Verify runtime linkage and bundled UI before any registry publication.
docker run --rm --network none "$IMAGE" --help > "$BUILD_DIR/runtime-help.txt"
docker run --rm --network none --entrypoint /bin/sh "$IMAGE" -c 'test -s /opt/v-scheduler/ui/index.html'
docker push "$IMAGE"
# Check the actual registry publication before recording a tag.
docker manifest inspect "$IMAGE" > "$BUILD_DIR/manifest.json"
COL="$(find_component_column_letter "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "$COMPONENT_NAME")"
write_cell "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "${COL}1" "$COMPONENT_NAME"
write_cell "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "${COL}2" "$REPOSITORY"
ROW="$(find_date_row "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "$DATE")"
if [[ -z "$ROW" ]]; then prepend_date_row "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "$DATE"; ROW=4; fi
write_cell "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "${COL}${ROW}" "$TAG"
VALUE="$(get_range_values "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "${SHEET_ID}!${COL}${ROW}:${COL}${ROW}")"
printf '%s' "$VALUE" | python3 -c 'import json,sys; v=json.load(sys.stdin); assert v.get("code")==0 and v["data"]["valueRange"]["values"]==[[sys.argv[1]]], "Feishu readback mismatch"' "$TAG"
log "Published and recorded ${IMAGE} on ${sheet}"
