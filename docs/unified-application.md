# Unified model gateway and scheduler

> 接入操作统一参见 [V-Scheduler 统一接入指南](application-integration.md)。本页保留协议或部署参考。

One VOS application, `com.ictrek.v-scheduler`, contains the original V-Router gateway and V-Scheduler controller. The gateway remains a Go process; the controller remains a Rust process with its existing embedded local agent. The Vue application and all new frontend development live in this repository. Backend images retain the existing Feishu component columns `v_router_backend` and `v-scheduler`; no new frontend image column is required.

## API and feature compatibility

| Caller | Entry point | Owner |
|---|---|---|
| Existing model clients | `http://<host>:22888/v1/*` | Original gateway handlers, protocols, authentication and streaming |
| Gateway management | `http://<host>:22888/api/v1/*` | Original upstream, model, registration, routing and usage interfaces |
| New scheduler clients | `http://<host>:22888/scheduler/api/v1/*` | Controller authentication, tasks, permits, workers and resources |
| VOS unified API | `/api/com.ictrek.v-scheduler` plus the paths above | Gateway public entry point |
| Legacy VOS gateway clients | `/api/com.ictrek.v-router/*` | Compatibility alias in the unified package |
| Existing controller/Agent clients | `http://v-scheduler:38120/api/v1/*` | Retained controller interface |
| Legacy VOS controller clients | `/app/com.ictrek.v-scheduler/api/v1/*` | Retained controller mount |

The model, upstream, registration, routing, protocol bridge, playground, usage, diagnostic and API documentation functionality remains implemented by the original gateway. The package preserves its discovery configuration, intervals, service DNS aliases (`v-router`, `v-router-backend`) with the unique preferred alias `v-scheduler-gateway` and default port `22888`. The existing resource collector, task views and scheduler API documentation remain available in the same navigation. API documentation has one entry with gateway and scheduler categories.

New model callers use `http://v-scheduler-gateway:8080/v1` on the Docker network. Its unique alias avoids resolving the legacy standalone router when both generations coexist. Existing compatibility aliases remain available; private Scheduler inference credentials must use the unique alias. HybRAG gateway identity handoff migrates an existing approved identity to this origin with a higher binding revision and the same key.

Model callers use the gateway. Local and managed remote model computation requires approved scheduler admission; external cloud providers do not reserve local CPU/GPU. Business applications retain their job records, outputs and executor contracts. The controller does not proxy model payloads. Losing the controller must not block cloud-only routing.

## Retain the original gateway data in place

1. Record the host directory actually mounted at the old gateway's `/data`, along with its application installation settings, profiles and image digests. Make a protected recovery snapshot and a file hash inventory without printing credentials or request content. Do not guess a storage path from an app ID.
2. Wait for model work to finish and reconcile all active/suspect permits and asynchronous jobs. Uninstall the standalone V-Router with `remove_data=false`. Verify its containers and port binding have gone, and that the recorded data directory still exists. Do not remove that directory.
3. Set the unified app's `V_ROUTER_DATA_PATH` to the recorded **host** directory. Both the old `state.json` and the old `scheduler.local.json`, usage files and `scheduler-jobs` directory stay in place. Retain the original gateway discovery, authentication, host-port and service URL configuration in the unified installation. Controller configuration and database remain under the existing Scheduler ownership.
4. Install/upgrade V-Scheduler with the corresponding architecture profile. The gateway bind-mounts the retained directory at `/data`; it does not copy or rename business data. A fresh installation leaves `V_ROUTER_DATA_PATH` blank and uses `${VOS_APP_STORAGE_PATH}/gateway`.
5. Verify model/upstream identities and settings, automatic discovery, usage history, pending asynchronous recovery, the original port/API and the prefixed controller API. Compare immutable files or normalized state fields against the inventory; mutable logs may grow and leased discovery records may legitimately refresh. Validate real local and cloud calls, cancellation, streaming and resource release.

Rollback: stop the unified gateway before reinstalling the old Router with its original retained directory and settings. Keep the existing controller version/configuration compatible with any newly recorded tasks and permits; do not restore a stale database over completed work. Never run two writers against the same gateway directory or bind both installations to port `22888`.

Applications that declared a required dependency on `com.ictrek.v-router` must publish compatible package metadata using `com.ictrek.v-scheduler` before consolidation on that node. Existing internal gateway URLs stay valid through the preserved DNS aliases. Dependency requirements are resolved from the current published GitHub Release or App Store catalog during packaging; resolution failure stops packaging.

## Release and acceptance

Build the original gateway image only when gateway code changes. Build the `v-scheduler` image when the controller or unified UI changes. Keep both original Feishu image column names and fixed platform/date tag rules. The Scheduler package resolves both images from the proper AMD/ARM sheets. Its dependency app versions are read from published releases, not guessed from source versions.

Acceptance includes every original gateway feature, all automatic discoverers, gateway and controller protocol tests, all six supported profiles, one-navigation UI interactions and locales, ordinary-user access without scheduler-admin rights, retained-volume migration, old API compatibility and real model/resource lifecycle checks. Test installation removes the old Router first while retaining its data. A published package alone does not prove a tested installation or reboot recovery.

On a Linux build host, build the gateway binary at `~/dev/build/V-Scheduler/gateway/v-router` and the unified UI at `~/dev/build/V-Scheduler/unified-ui`. Run `scripts/e2e-statistics.mjs` with `KEEP_TEST_SERVICES=1` and `SCHEDULER_UI_DIR` pointing to that UI directory. In a second session, run `SCHEDULER_TEST_RUN=<printed-fixture-directory> KEEP_TEST_SERVICES=1 node scripts/e2e-unified.mjs`. The latter uses the real gateway and controller with an isolated OpenAI upstream and identity endpoint; it checks authentication, namespace separation, upstream CRUD, models, streaming, usage and API documentation. Retained fixtures contain private test credentials and must remain protected. They do not replace validation on an installed VOS device.

## Source ownership

The Go gateway source is `gateway/` in this repository; its module/import paths, binary name, service aliases and public APIs remain compatible. Build from this checkout on a Linux host with `cd gateway && go test ./... && go build -o ~/dev/build/V-Scheduler/gateway/v-router .`. The Rust controller and unified frontend retain their existing build commands.

Publish gateway images using `docker/build_gateway_image.sh --component backend --sheet AMD_with_cuda` on an AMD host, or `--sheet ARM_with_cuda` on an ARM host. This retains the `v_router_backend` Feishu column, repository and daily tags; `docker/build_image.sh` publishes the separate controller image in the existing `v-scheduler` column. Both scripts use this checkout and keep build artifacts under `~/dev/build`.
