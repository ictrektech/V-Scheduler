# V-Scheduler Resource Scheduling

Open **V-Scheduler** in the VOS application group to find Models & Usage, Models & Providers, inference deployments, Playground, Model Weights, Image Registry, Resources & Tasks, management pages and API Documentation. After creating a personal key from **API Key**, copy the LAN Base URL and key from the result panel. Choose a model visible to that key. Under **API Documentation → Cross-node model weight transfer**, download the specification for preparing weights without a Model Hub mount.

Image Studio shows the main node and each ready compute node in separate rows for the same model. **Test** and **API usage** use that row’s node. Main-node requests use local execution. Remote requests include the node’s `compute_deployment_id`. Under **Model settings**, a VOS administrator can enable or cancel direct admission using `PUT /api/v1/model-routing`.

The Inference Deployment page resumes the VOS session automatically. Loaded data stays visible while the page shows a single recovery status. The current view refreshes once the session is restored.

To run a VOS application on another node, choose a compatible registered compute node in the app, deploy its backend and prepare the model. The backend uses a read-only Model Hub NFS share when available. Otherwise, weight transfer resumes from the recorded byte offsets. Scheduler registers the executor after the files pass SHA-256 validation and the target node matches the model's measured calibration. See the deployment, transfer and execution routes in the Scheduler API reference.

When adding or editing a model in **Models & Providers**, use **Thinking control** to declare the Chat Completions format and effort values supported by the upstream. A compatible client can read these settings from the optional `reasoning_control` model catalog field.

## Image Registry

Sign in as a VOS administrator and open **V-Scheduler → Image Registry**. Pull an upstream image into the local registry, then synchronize the repository or a specific tag. To work offline, import a Docker or OCI archive. Omit the destination to keep the archive's image name. The task panel shows progress and failure details. Management state and image layers persist in the app's `image-hub/` and `image-registry/` directories.

To deploy an image, copy the tag's deployment address from image details. Open **Advanced Management → Node Management → Application Backends**, select the app and node, and use the matching image from the app’s published catalog. The data endpoint defaults to `127.0.0.1:36113` on the VOS host. For remote nodes, set `SCHEDULER_IMAGE_HUB_BIND` to a host address on a trusted network reachable from those nodes and configure their Docker runtime to access that HTTP registry.

After deleting tags or repositories, preview garbage collection and then reclaim unreferenced layers. This acts on the V-Scheduler registry. Keep tags needed by active deployments so their images can be pulled again. For the management endpoints, open **API Documentation → Image Registry API** in the sidebar. Requests use a VOS administrator Bearer token.

VOS administrators can open Weight management → Model integration status to see weight progress, service status, and where requests go. Built-in Ollama shows the local VOS host. External services show the host the gateway connects to. If the upstream uses a proxy, the physical node behind it cannot be verified. Remote weights are marked as managed by the upstream. Select a model in Models & providers, click Test, and send a Playground request. Return to the status page for the selected upstream, result, and time. Playground identifies connection, authentication, model loading, or timeout failures and reports reachability. The latest result per model and upstream survives reload for the same signed-in identity in this browser. Clearing site data or disabling local storage can prevent recovery after reload.

GET /api/v1/model-lifecycle requires a VOS administrator. download is null or contains phase and progress from 0 to 1; model_running is a boolean or null; upstream_models uses weight_source upstream or unlinked. POST /api/v1/upstreams/probe can return diagnosis and test_diagnosis with code and reachability (reachable, unreachable or unknown). Playground calls the normal /v1 APIs. The UI reads id, source, and base_url from the existing GET /api/v1/upstreams endpoint and shows only the host and port from the address. It scopes browser results using auth_method and identity.sub from GET /api/v1/auth/me. Up to 200 model/upstream results are kept with their timestamp and host. Prompt and response text are omitted. The API contract is unchanged.

New: Inference queue and Model library → Model integration status. Playground waits up to 120 seconds with position and cancellation; applications opt into interactive/background waiting through gateway request headers. Built-in Ollama models retain idle weights for 60 seconds by default and reuse them for consecutive calls. The full reservation remains held until confirmed unloading on model switch, idle expiry or manual stop. Administrators can set 0..600 seconds; 0 unloads after each call. Service metadata.model_bindings links exact repository IDs and native served names; one status page combines routes, backend health and scheduler bindings. Existing model storage, nine published endpoints, host aliases and four containers remain compatible.

The unified package includes the model gateway and scheduling controller. Model clients use host port 22888 /v1; scheduling uses /scheduler/api/v1. Router connects directly to its three built-in Ollama backends without Model Hub discovery. It also discovers available models from Image Studio, V-Voice, V-Music, ML Server, and Video Structure. Once ready, V-Voice connects automatically and binds calibrated executors and model mappings. See Application integration in the UI.

Ready V-Voice models inherit the application resource budget automatically and appear in the model catalog. Configured upstream budgets also apply to newly listed models. Use Application integration and the API docs to inspect the integration and available endpoints. Request and response formats are unchanged.

Image Studio automatically binds its calibrated SD Turbo executor on the ARM CPU main node once the weights are ready. In Image Studio, choose Generate → Compute node → Main node for one F16 image up to 512×512 and four steps. Check the execution node in V-Scheduler task details. Generation API requests keep their existing format.


Version __APP_VERSION__

Version 0.0.109 removes the controller container memory limit that caused restart loops when the controller exceeded the previous limit. Upgrade from the VOS App Store to apply the fix. Inference admission, model budgets, and API contracts are unchanged.

Installation checks the PGV dependency. On first opening, sign in as a VOS administrator to automatically initialize a dedicated database, ordinary database role and internal collector identity. Configuration lives in the private ${VOS_APP_STORAGE_PATH}/config directory. No configuration files or terminal commands are required. The page shows initialization status while PGV is unavailable.

Interrupted first-time setup resumes unfinished work. Restarts reuse the existing database. If an existing database or account is unavailable, the app asks an administrator to restore the database or correct its connection settings. If first-time setup fails, follow the displayed instructions and select Retry connection.

The controller uses administrator-approved node and executor budgets. Sign in with your VOS administrator account. UI language follows VOS by default (English / Simplified Chinese / Traditional Chinese).

Host resource collection is built into this app and starts automatically after installation. No separate Agent, terminal command or collector credential is required. Expand the logical processor count to inspect each processor. Jetson Thor uses one shared system RAM pool for CPU and GPU.

App installation and configured execution location are distinct. Cloud inference bypasses local permits. Unconfirmed execution retains its resource reservation; deletion only removes stopped task records and does not delete business outputs.

VOS administrator sign-in and automatic local collection are supported. HA and device-level multi-GPU allocation are not implemented.

The dashboard shows 24-hour queue wait, occupancy P50/P95, task failure rate and model permit history. Live task details include a status timeline, per-attempt assignment/stop times, waiting guidance and controlled retry. Combine status, application and node filters, search application task IDs, and load older tasks beyond the initial 200. P95 requires at least 20 valid samples; old permits retain unknown release times.

Node management reuses the installed ModelInfer app’s remote-node inventory and existing monitoring. Users do not install Agents on individual machines. Save budget plans, manage approved nodes, executors and scoped service credentials, and inspect activity. Plans do not grant execution; remote scheduling control awaits verification. Existing deployments remain managed by their owning VOS app.

Administrators open **Inference deployments** from the V-Scheduler sidebar. Deployment lists, details and model cards open in this workspace. Manage storage clusters in the standalone VOS Cluster Manager app. To import existing deployments, use **Node Management → Migrate ModelInfer deployments**. **API Documentation → Scheduler Management API** lists the existing migration and runtime endpoints.

Open Node management → Nodes and permissions → Nodes discovered by VOS apps to see the same host measurements as Resources & Tasks → Execution resources. The panel shows CPU utilization and logical processors, used/total/available RAM, GPU utilization, and sample time. It indicates when a sample is missing or stale. The node discovery record can still be current. Both views use the existing Scheduling API GET /nodes. Embedded inference samples are in modelinfer_source.node.host_resources.

Inference deployments keeps its tabs visible while their runtimes start or reconnect. The content area shows the reason and retries automatically. Select Retry connection to retry immediately without reloading. Runtime health API contracts stay the same.

Open **Inference deployments → Inference images** to browse registered references and compatible profiles. Administrators can register images, select built-in profiles, and add custom names. Assign a custom profile under **Node management → SSH compute nodes → Edit**.

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

With idle retention enabled, verified callers to built-in Ollama wait up to 30 seconds by default for model handoff and concurrent work. Explicit X-VScheduler-Queue-Wait-Seconds: 0 preserves immediate admission. Anonymous callers and other backends keep the previous behavior.

### Unified application integration

Node management now combines application heartbeats, node health, approved budgets and stop requests. Applications register approved executors through the controller `/integrations` API and follow the returned heartbeat interval. Reservations are released only after the original execution identity confirms native exit and weight unloading. VOS App components report node telemetry automatically; users do not deploy Agents or use SSH. Existing automatic bindings remain compatible; applications without the new stop protocol are not presented as fully managed. See API Documentation → Application integration and the repository integration guide.

On VOS with Traefik v3, opening the app root directly in a browser redirects to its VOS page. Embedded pages and API requests use their existing routes.

In Resources and tasks → Execution resources, administrators can read the observed nodes’ logical CPU count and utilization, available and total system memory, and GPU utilization. Shared memory is counted once. Reports require a working saved SSH connection when using the embedded inference runtime; unavailable samples are identified in the panel. The data is also exposed by the administrator GET /nodes scheduling API.

### Application backend deployments

Open API Documentation → Scheduling API → Application backend deployments for roles, fields, states, and examples. Register the target and Profile, issue an application service credential, and grant repositories and targets with PUT /backend-deployment-access/{app_id}. Submit the selected image, target, and container port to POST /backend-deployments. Poll until running and read endpoint. SSH targets use Docker. Trusted cluster members use VOS Projects. Stop retains the host port. Confirmed deletion releases it.

Administrators can register applications and select target profiles in Node Management → Application backends. The deployment form saves the image, container port, optional host port, CPU and memory caps, and backend JSON settings. API Documentation → Scheduling API → Application backend deployments has examples for SSH and cluster targets.

Under Model weights → Inference services, administrators can set Automatic preload independently for the built-in chat, embedding, and rerank instances. Each switch starts with its installation setting and saved changes persist. Enabling downloads the configured tag if missing and preloads it through scheduler admission. Disabling prevents new preload attempts and lets submitted requests finish. Idle eviction still follows model idle retention. Check the card for preload status.

GET /api/v1/ollama-preload · PUT /api/v1/ollama-preload

A VOS administrator can generate, set, or revoke a model access key for a model registered by the inference app from Models and upstreams → Model access key. The issuing administrator manages that key, which grants access only to the selected private model. Public models are callable without a key.

When adding or editing an SSH compute node in Node Management, disable Model Hub NFS for application backends to copy weights directly. Stop and start an existing backend to update its mount. Prepare models that used the share again.

To remove one Image Studio model from a compute node, call `DELETE /backend-deployments/{id}/models/{model_id}` as the owning app or an administrator. The worker must support model deletion. Active tasks return `BACKEND_TASKS_ACTIVE`. Pending cleanup blocks new admissions for the model. Retry the same request after a cleanup failure. On success, `data.deleted_transfers` is the number of target transfer records removed in this attempt.
