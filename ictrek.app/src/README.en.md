# V-Scheduler Resource Scheduling

New: Inference queue and Model library → Model integration status. Playground waits up to 120 seconds with position and cancellation; applications opt into interactive/background waiting through gateway request headers. Built-in Ollama models retain idle weights for 60 seconds by default and reuse them for consecutive calls. The full reservation remains held until confirmed unloading on model switch, idle expiry or manual stop. Administrators can set 0..600 seconds; 0 unloads after each call. Service metadata.model_bindings links exact repository IDs and native served names; one status page combines routes, backend health and scheduler bindings. Existing model storage, nine published endpoints, host aliases and four containers remain compatible.

The unified package includes the model gateway and scheduling controller. Model clients use host port 22888 /v1; scheduling uses /scheduler/api/v1. Router connects directly to its three built-in Ollama backends without Model Hub discovery. It also discovers available models from Image Studio, V-Voice, V-Music, ML Server, and Video Structure. Once ready, V-Voice connects automatically and binds calibrated executors and model mappings. See Application integration in the UI.

Version __APP_VERSION__

Installation checks the PGV dependency. On first opening, sign in as a VOS administrator to automatically initialize a dedicated database, ordinary database role and internal collector identity. Configuration lives in the private ${VOS_APP_STORAGE_PATH}/config directory. No configuration files or terminal commands are required. The page shows initialization status while PGV is unavailable.

The controller uses administrator-approved node and executor budgets. Sign in with your VOS administrator account. UI language follows VOS by default (English / Simplified Chinese / Traditional Chinese).

Host resource collection is built into this app and starts automatically after installation. No separate Agent, terminal command or collector credential is required. Expand the logical processor count to inspect each processor. Jetson Thor uses one shared system RAM pool for CPU and GPU.

App installation and configured execution location are distinct. Cloud inference bypasses local permits. Unconfirmed execution retains its resource reservation; deletion only removes stopped task records and does not delete business outputs.

VOS administrator sign-in and automatic local collection are supported. HA and device-level multi-GPU allocation are not implemented.

The dashboard shows 24-hour queue wait, occupancy P50/P95, task failure rate and model permit history. Live task details include a status timeline, per-attempt assignment/stop times, waiting guidance and controlled retry. Combine status, application and node filters, search application task IDs, and load older tasks beyond the initial 200. P95 requires at least 20 valid samples; old permits retain unknown release times.

Node management reuses the installed ModelInfer app’s remote-node inventory and existing monitoring. Users do not install Agents on individual machines. Save budget plans, manage approved nodes, executors and scoped service credentials, and inspect activity. Plans do not grant execution; remote scheduling control awaits verification. Existing deployments remain managed by their owning VOS app.

## Docker aliases and host ports

The defaults below apply to every profile. Replace `<vos-host>` with the installed VOS host IP/name. Docker aliases resolve on `vos_default`; prefix addresses with `http://`. All listed host ports are installer parameters. The in-app API guide displays the configured values.

The unified container has one primary host alias: `v-scheduler`. Ports `8080`, `38120` and `6005` select gateway, scheduling and model management. All other names for this container are compatibility aliases. New applications and future updates to existing applications should use `v-scheduler`. The three Ollama containers keep their own names.

| Function | All Docker aliases : internal port | Host : published port | API / installer parameter |
|---|---|---|---|
| Unified inference, gateway management and UI | `v-scheduler:8080`; compatibility: `v-scheduler-gateway:8080`, `v-router:8080`, `v-router-backend:8080` | `<vos-host>:22888` | `/v1/*`, `/api/v1/*`, `/`; `V_ROUTER_HOST_PORT` |
| Scheduling, takeover and resources | `v-scheduler:38120`; compatibility: `v-scheduler-backend:38120`, `v-scheduler-frontend:38120` | `<vos-host>:38120` | `/api/v1/*`; `SCHEDULER_HOST_PORT` |
| Model library management | `v-scheduler:6005`; compatibility: `v-scheduler-model-hub:6005`, `v-scheduler-model-hub-backend:6005` | `<vos-host>:36005` | `/api/v1/*`, `/openapi.json`; `MODEL_HUB_BACKEND_HOST_PORT` |
| Ollama QA native management | `v-scheduler-ollama-qa:11434` | `<vos-host>:12434` | `/api/*`; `OLLAMA_QA_HOST_PORT` |
| Ollama QA inference and metrics | `v-scheduler-ollama-qa:11535` | `<vos-host>:12535` | `/v1/chat/completions`, `/metrics`; `OLLAMA_QA_GATEWAY_HOST_PORT` |
| Ollama embedding native management | `v-scheduler-ollama-embedding:11434` | `<vos-host>:12435` | `/api/*`; `OLLAMA_EMBEDDING_HOST_PORT` |
| Ollama embedding inference and metrics | `v-scheduler-ollama-embedding:11535` | `<vos-host>:12536` | `/v1/embeddings`, `/metrics`; `OLLAMA_EMBEDDING_GATEWAY_HOST_PORT` |
| Ollama rerank native management | `v-scheduler-ollama-rerank:11434` | `<vos-host>:12436` | `/api/*`; `OLLAMA_RERANK_HOST_PORT` |
| Ollama rerank inference and metrics | `v-scheduler-ollama-rerank:11535` | `<vos-host>:12537` | `/v1/rerank`, `/metrics`; `OLLAMA_RERANK_GATEWAY_HOST_PORT` |

Aliases in one row reach the same container/port. `host.docker.internal` is the gateway container's `host-gateway` alias for the Docker host, e.g. `host.docker.internal:38120`; it is not a separate service or port.

The gateway also proxies scheduling at port 22888 `/scheduler/api/v1/*` and model management at `/model-hub/api/v1/*`. The VOS UI uses `/app/com.ictrek.v-scheduler/`; the model-management API uses `/app/com.ictrek.v-scheduler/model-hub/api/v1/*`; the gateway API prefix is `/api/com.ictrek.v-scheduler`. Use the unified gateway `/v1/*` for cross-backend model names and scheduling. Retain the original Model Hub installer-selected public directory to adopt existing models in place.
