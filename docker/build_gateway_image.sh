#!/usr/bin/env bash
set -euo pipefail

# =============================================================================
# V-Scheduler gateway build/push script
# Builds the retained v-router backend image, pushes them to SWR, then writes the
# image tags to the release sheets for the current architecture.
#
# Usage:
#   ./build_gateway_image.sh
#   ./build_gateway_image.sh --sheet AMD_with_cuda
#   ./build_gateway_image.sh --sheet ARM_with_cuda --sheet l4t
#   ./build_gateway_image.sh --component backend
#
# Supported sheets:
#   AMD: AMD_with_cuda, AMD_with_mxn100
#   ARM: ARM_without_cuda, ARM_with_cuda, l4t, thor_spark, SOPHON_bm1688
# =============================================================================

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT_DIR/docker"

FEISHU_CONFIG_FILE="${HOME}/.feishu.json"
FEISHU_SPREADSHEET_TOKEN="Htotsn3oahO1zxt73YMcaB1zn8e"
REGISTRY="swr.cn-southwest-2.myhuaweicloud.com/ictrek"

BACKEND_COMPONENT_NAME="v_router_backend"
BACKEND_REPOSITORY="${REGISTRY}/v-router-backend"

# Sheets accepted by --sheet (validation); defaults below only write the two
# sheets package.sh actually reads for the amd/arm profiles.
AMD_SHEETS=("AMD_with_cuda" "AMD_with_mxn100")
ARM_SHEETS=("ARM_without_cuda" "ARM_with_cuda" "l4t" "thor_spark" "SOPHON_bm1688")
DEFAULT_AMD_SHEETS=("AMD_with_cuda")
DEFAULT_ARM_SHEETS=("ARM_with_cuda")

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
    curl --fail -sS -X "$method" "$url" \
      -H "Authorization: Bearer ${token}" \
      -H "Content-Type: application/json" \
      --data "$body"
  else
    curl --fail -sS -X "$method" "$url" \
      -H "Authorization: Bearer ${token}"
  fi
}

get_feishu_token() {
  local app_id="$1"
  local app_secret="$2"
  local resp

  resp=$(
    curl --fail -sS -X POST "https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal" \
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
    raise SystemExit(f"get_feishu_token failed: {data}")
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
    raise SystemExit(f"query sheets failed: {data}")
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
    raise SystemExit(f"read header failed: {data}")
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
    raise SystemExit(f"query sheets failed: {data}")
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
    raise SystemExit(f"add component column failed: {data}")
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
    raise SystemExit(f"read date column failed: {data}")
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
  local token="$1"
  local spreadsheet_token="$2"
  local sheet_id="$3"
  local today="$4"
  local resp

  resp=$(feishu_api_json "POST" \
    "https://open.feishu.cn/open-apis/sheets/v2/spreadsheets/${spreadsheet_token}/values_prepend" \
    "$token" \
    "{\"valueRange\":{\"range\":\"${sheet_id}!A4:A4\",\"values\":[[\"${today}\"]]}}")

  python3 - "$resp" <<'PY'
import json, sys
data = json.loads(sys.argv[1])
if data.get("code") != 0:
    raise SystemExit(f"prepend date row failed: {data}")
PY
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
    raise SystemExit(f"write cell failed: {data}")
PY
}

case "$(uname -m)" in
  x86_64|amd64)
    ARCH_TAG="amd"
    BASE_ARCH="amd64"
    DEFAULT_SHEETS=("${DEFAULT_AMD_SHEETS[@]}")
    ;;
  aarch64|arm64)
    ARCH_TAG="arm"
    BASE_ARCH="arm64"
    DEFAULT_SHEETS=("${DEFAULT_ARM_SHEETS[@]}")
    ;;
  *)
    die "unsupported architecture: $(uname -m)"
    ;;
esac

TARGET_SHEETS=()
COMPONENTS=()
if [[ -n "${V_ROUTER_RELEASE_SHEETS:-}" ]]; then
  IFS=',' read -r -a ENV_SHEETS <<< "${V_ROUTER_RELEASE_SHEETS}"
  for sheet in "${ENV_SHEETS[@]}"; do
    [[ -n "$sheet" ]] && TARGET_SHEETS+=("$sheet")
  done
fi

while [[ $# -gt 0 ]]; do
  case "$1" in
    --component)
      COMPONENTS+=("$2")
      shift 2
      ;;
    --sheet)
      TARGET_SHEETS+=("$2")
      shift 2
      ;;
    -h|--help)
      cat <<'EOF'
Usage:
  ./build_gateway_image.sh                      # default: only the sheet package.sh reads
  ./build_gateway_image.sh --sheet AMD_with_cuda
  ./build_gateway_image.sh --sheet ARM_with_cuda --sheet l4t
  ./build_gateway_image.sh --component backend

By default amd writes AMD_with_cuda and arm writes ARM_with_cuda — the only
sheets consumed by ictrek.app packaging. Use --sheet to record the tag on
additional platform sheets when needed.
EOF
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
done

if [[ ${#TARGET_SHEETS[@]} -eq 0 ]]; then
  TARGET_SHEETS=("${DEFAULT_SHEETS[@]}")
fi

if [[ ${#COMPONENTS[@]} -eq 0 ]]; then
  COMPONENTS=("backend")
fi

for sheet in "${TARGET_SHEETS[@]}"; do
  if [[ "$ARCH_TAG" == "amd" ]]; then
    contains "$sheet" "${AMD_SHEETS[@]}" || die "sheet ${sheet} is not valid for amd"
  else
    contains "$sheet" "${ARM_SHEETS[@]}" || die "sheet ${sheet} is not valid for arm"
  fi
done

for component in "${COMPONENTS[@]}"; do
  case "$component" in
    backend) ;;
    *) die "unsupported component: $component" ;;
  esac
done

BUILD_ARGS=(--build-arg "BASE_ARCH=${BASE_ARCH}")
if [[ -n "${PROXY:-}" ]]; then
  log "Using PROXY=${PROXY}"
  BUILD_ARGS+=(--build-arg "PROXY=${PROXY}")
fi

for arg in GO_IMAGE RUNTIME_IMAGE; do
  if [[ -n "${!arg:-}" ]]; then
    BUILD_ARGS+=(--build-arg "${arg}=${!arg}")
  fi
done

DATE=$(date +%Y%m%d)
TAG="${ARCH_TAG}_${DATE}"

require_cmd curl
require_cmd docker
require_cmd python3
docker buildx version >/dev/null 2>&1 || die "docker buildx is required"

if [[ ! -f "$FEISHU_CONFIG_FILE" ]]; then
  die "Feishu config not found: $FEISHU_CONFIG_FILE"
fi

FEISHU_APP_ID="$(read_feishu_field "feishu_app_id")"
FEISHU_APP_SECRET="$(read_feishu_field "feishu_app_secret")"
[[ -n "$FEISHU_APP_ID" && -n "$FEISHU_APP_SECRET" ]] || die "feishu_app_id or feishu_app_secret missing in $FEISHU_CONFIG_FILE"

log "Architecture: ${ARCH_TAG}"
log "Sheets: ${TARGET_SHEETS[*]}"
log "Components: ${COMPONENTS[*]}"
log "Tag: ${TAG}"

BUILD_DIR="${HOME}/dev/build/V-Scheduler/gateway-images/${ARCH_TAG}"
mkdir -p "$BUILD_DIR/context"
tar --exclude=.git --exclude=node_modules --exclude=target --exclude=dist --exclude="*.local.json" --exclude=.env -cf - -C "$ROOT_DIR" gateway | tar -xf - -C "$BUILD_DIR/context"

for component in "${COMPONENTS[@]}"; do
  case "$component" in
    backend)
      IMAGE="${BACKEND_REPOSITORY}:${TAG}"
      log "Building backend image: ${IMAGE}"
      docker buildx build \
        --builder default \
        --load \
        --provenance=false \
        --sbom=false \
        "${BUILD_ARGS[@]}" \
        -t "v-router-backend:${TAG}" \
        -t "${IMAGE}" \
        -f "$BUILD_DIR/context/gateway/Dockerfile" \
        "$BUILD_DIR/context"
      ;;

  esac
  docker push "${IMAGE}"
  log "Image pushed: ${IMAGE}"
done

for sheet in "${TARGET_SHEETS[@]}"; do
  FEISHU_TOKEN="$(get_feishu_token "$FEISHU_APP_ID" "$FEISHU_APP_SECRET")"
  SHEET_ID="$(get_sheet_id_by_title "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$sheet")"
  log "Resolved sheet: ${sheet} -> ${SHEET_ID}"

  FEISHU_TOKEN="$(get_feishu_token "$FEISHU_APP_ID" "$FEISHU_APP_SECRET")"
  DATE_ROW="$(find_date_row "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "$DATE")"
  if [[ -z "$DATE_ROW" ]]; then
    log "Date ${DATE} not found, creating a new row at top of data area"
    FEISHU_TOKEN="$(get_feishu_token "$FEISHU_APP_ID" "$FEISHU_APP_SECRET")"
    prepend_date_row "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "$DATE"
    DATE_ROW=4
  else
    log "Date ${DATE} already exists at row ${DATE_ROW}"
  fi

  for component in "${COMPONENTS[@]}"; do
    case "$component" in
      backend)
        COMPONENT_NAME="$BACKEND_COMPONENT_NAME"
        REPOSITORY="$BACKEND_REPOSITORY"
        ;;

    esac

    FEISHU_TOKEN="$(get_feishu_token "$FEISHU_APP_ID" "$FEISHU_APP_SECRET")"
    COMPONENT_COL="$(find_component_column_letter "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "$COMPONENT_NAME")"
    write_cell "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "${COMPONENT_COL}1" "$COMPONENT_NAME"
    write_cell "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "${COMPONENT_COL}2" "$REPOSITORY"
    log "Resolved component column: ${COMPONENT_NAME} -> ${COMPONENT_COL}"

    FEISHU_TOKEN="$(get_feishu_token "$FEISHU_APP_ID" "$FEISHU_APP_SECRET")"
    write_cell "$FEISHU_TOKEN" "$FEISHU_SPREADSHEET_TOKEN" "$SHEET_ID" "${COMPONENT_COL}${DATE_ROW}" "$TAG"
    log "Feishu updated: ${sheet}!${COMPONENT_COL}${DATE_ROW} = ${TAG}"
  done
done
