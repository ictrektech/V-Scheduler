# Unified model gateway and scheduler

> 接入操作统一参见 [V-Scheduler 统一接入指南](application-integration.md)。本页保留协议或部署参考。

One VOS application, `com.ictrek.v-scheduler`, contains the original V-Router gateway and V-Scheduler controller. The gateway, controller and model workspace run in one Rust process with the existing embedded local collector. The package starts four containers: this unified service and three role-specific Ollama instances. The Vue application and all new frontend development live in this repository. The unified service uses the existing `v-scheduler` Feishu image column; Ollama still uses its original `ollama_server` column and profile-specific images.

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

The model, upstream, registration, routing, protocol bridge, playground, usage, diagnostic and API documentation functionality remains implemented by the Rust router. The package preserves its discovery configuration, intervals, service DNS aliases (`v-router`, `v-router-backend`) with the single primary alias `v-scheduler`; all other names of this container are compatibility aliases and default port `22888`. The existing resource collector, task views and scheduler API documentation remain available in the same navigation. API documentation has one entry with gateway and scheduler categories.

New model callers use `http://v-scheduler:8080/v1` on the Docker network. The primary alias avoids resolving the legacy standalone router when both generations coexist. Existing compatibility aliases remain available; private Scheduler inference credentials must use the unique alias. Existing HybRAG identity handoffs retain their versioned, validated compatibility-origin contract; changing the recommended name does not rewrite an installed private credential or bypass origin validation.

Model callers use the gateway. Local and managed remote model computation requires approved scheduler admission; external cloud providers do not reserve local CPU/GPU. Business applications retain their job records, outputs and executor contracts. Admission calls dispatch to the controller in memory. A temporary PGV outage does not block cloud routing or model management; local computation still requires valid admission. The unified components share a process lifetime, so a process failure restarts all three.

## Retain the original gateway data in place

1. Record the host directory actually mounted at the old gateway's `/data`, along with its application installation settings, profiles and image digests. Make a protected recovery snapshot and a file hash inventory without printing credentials or request content. Do not guess a storage path from an app ID.
2. Wait for model work to finish and reconcile all active/suspect permits and asynchronous jobs. Uninstall the standalone V-Router with `remove_data=false`. Verify its containers and port binding have gone, and that the recorded data directory still exists. Do not remove that directory.
3. Set the unified app's `V_ROUTER_DATA_PATH` to the recorded **host** directory. Both the old `state.json` and the old `scheduler.local.json`, usage files and `scheduler-jobs` directory stay in place. Retain the original gateway discovery, authentication, host-port and service URL configuration in the unified installation. Controller configuration and database remain under the existing Scheduler ownership.
4. Install/upgrade V-Scheduler with the corresponding architecture profile. The gateway bind-mounts the retained directory at `/gateway-data`; it does not copy or rename business data. A fresh installation leaves `V_ROUTER_DATA_PATH` blank and uses `${VOS_APP_STORAGE_PATH}/gateway`.
5. Verify model/upstream identities and settings, automatic discovery, usage history, pending asynchronous recovery, the original port/API and the prefixed controller API. Compare immutable files or normalized state fields against the inventory; mutable logs may grow and leased discovery records may legitimately refresh. Validate real local and cloud calls, cancellation, streaming and resource release.

Rollback: stop the unified gateway before reinstalling the old Router with its original retained directory and settings. Keep the existing controller version/configuration compatible with any newly recorded tasks and permits; do not restore a stale database over completed work. Never run two writers against the same gateway directory or bind both installations to port `22888`.

Applications that declared a required dependency on `com.ictrek.v-router` must publish compatible package metadata using `com.ictrek.v-scheduler` before consolidation on that node. Existing internal gateway URLs stay valid through the preserved DNS aliases. Dependency requirements are resolved from the current published GitHub Release or App Store catalog during packaging; resolution failure stops packaging.

## Release and acceptance

Build and publish the single `v-scheduler` image for AMD and ARM when any unified backend or frontend changes. Reuse unchanged published Ollama images. Dependency versions are resolved from published releases during packaging.

Acceptance includes every original gateway feature, all automatic discoverers, gateway and controller protocol tests, all six supported profiles, one-navigation UI interactions and locales, ordinary-user access without scheduler-admin rights, retained-volume migration, old API compatibility and real model/resource lifecycle checks. Test installation removes the old Router first while retaining its data. A published package alone does not prove a tested installation or reboot recovery.

On a synchronized Linux build host run `cargo test --locked --manifest-path backend/Cargo.toml`, build the candidate binary under `~/dev/build/V-Scheduler/cargo`, then run `scripts/e2e-api-contract.py`. This uses one actual Rust process, isolated PostgreSQL, synthetic identity/model endpoints and protected temporary data. Run `scripts/e2e-model-hub.py --binary <candidate> --build-root <build-directory>` for model management acceptance. Installed-host model calls and recovery checks remain separate acceptance requirements.

## Source ownership

Rust gateway: `backend/src/router/`; scheduler: `backend/src/controller.rs` and related modules; model workspace: `backend/src/modelhub/`; Vue UI: `frontend/`. The original Go implementation is retained in Git history; `gateway/docs/` contains protocol and historical verification references.

Publish with `docker/build_image.sh --sheet AMD_with_cuda` on the AMD build host and `--sheet ARM_with_cuda` on the ARM build host. The old `build_gateway_image.sh` command delegates to that unified builder. No separate Router or model-workspace image/container is installed. The controller keeps `v-router`, `v-router-backend`, `v-scheduler-gateway`, `v-scheduler-model-hub` and `v-scheduler-model-hub-backend` aliases and all previous port mappings. `/data` mounts the same installation-selected shared model directory; `/gateway-data` mounts the retained gateway state. Every profile selects its original Ollama image.
