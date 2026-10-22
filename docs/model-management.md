# Model management in V-Scheduler

Model Hub functionality is implemented in V-Scheduler's Rust/Axum backend and Vue/TypeScript frontend. No Model Hub Python server, React bundle or separate Model Hub app installation is required. Ollama remains built by the Ollama submodule. The package reads the original `ollama_server` Feishu column for AMD_with_cuda, ARM_with_cuda, ARM_without_cuda, l4t and thor_spark, without publishing replacement Ollama images.

Open **Model library / 模型仓库** for local models, downloads, archive imports, download tasks, inference-service cards, aliases and container references. Open **API documentation → Model management API** for the same integration guide and full operation reference. Chat uses the existing V-Scheduler playground. Router directly connects the three built-in Ollamas as `v-scheduler/ollama-qa`, `v-scheduler/ollama-embedding`, `v-scheduler/ollama-rerank`; it does not discover them through Model Hub. Existing local resource admission requirements remain in force.

## Shared directory adoption and rollback

At VOS installation, set `MODEL_HUB_SHARED_MODELS_PATH` to the exact directory used by the existing Model Hub installation. Default `/data/vos_workspace/model_hub`; shared-directory hint remains `[com.ictrek.model.storage]`. The model-management Rust service and all three Ollamas mount it at `/data`, matching the old application. Retain this directory when uninstalling either app.

The following existing data is adopted in place: `registry/models.json`, `aliases.json`, `containers.json`, `inference_services.json`, `shared/tasks/*.json`, `ollama/models/{blobs,manifests}`, `export/{hf,ms}/<org>/<repo>/snapshots` and `current`. Legacy `huggingface/hub/models--<org>--<repo>` and `modelscope/hub` caches are also recognized. No startup move, copy or redownload is required. Registry operations use the same Linux flock files under `shared/locks`. Unknown metadata fields are preserved.

New downloads stage under `shared/tmp`; size and supplied SHA256 are verified before publishing a snapshot. `current` switches after the selected scope completes. Updates and cancellation preserve the old snapshot. Append preserves previously downloaded files, including unchanged weight inodes when hard links are supported. Failed tasks retain their file selection and can resume partial files with HTTP Range; an upstream version change invalidates a partial from the old version. Interrupted V-Scheduler tasks become retryable; another app's task is not marked failed on startup.

The old and new applications use different DNS aliases and published ports and may coexist. Avoid downloading/updating/deleting the same model simultaneously in both apps. Fresh shared task records block competing downloads. Rollback consists of retaining this shared directory and opening the original Model Hub again; both use the same public registry/task schemas. No old model files or legacy registries are removed as a migration step.

## Internal and host endpoints

These are defaults; all published ports are installation parameters. The application guide reads the configured values. Replace `<vos-host>` with the VOS host IP or hostname. Docker aliases resolve only for containers on `vos_default`. A container's localhost refers to that container.

| Function | Docker host alias : internal port | VOS host : published port | API root |
|---|---|---|---|
| Unified model inference | `v-router:8080` | `<vos-host>:22888` | `/v1` |
| Gateway management | `v-router:8080` | `<vos-host>:22888` | `/api/v1` |
| Scheduler management/takeover | `v-scheduler:38120` | `<vos-host>:38120` | `/api/v1` |
| Model management | `v-scheduler:6005` | `<vos-host>:36005` | `/api/v1` |
| Ollama QA native management | `v-scheduler-ollama-qa:11434` | `<vos-host>:12434` | `/api` |
| Ollama QA metered inference | `v-scheduler-ollama-qa:11535` | `<vos-host>:12535` | `/v1` |
| Ollama embedding native management | `v-scheduler-ollama-embedding:11434` | `<vos-host>:12435` | `/api` |
| Ollama embedding metered inference | `v-scheduler-ollama-embedding:11535` | `<vos-host>:12536` | `/v1` |
| Ollama rerank native management | `v-scheduler-ollama-rerank:11434` | `<vos-host>:12436` | `/api` |
| Ollama rerank metered inference | `v-scheduler-ollama-rerank:11535` | `<vos-host>:12537` | `/v1` |

Model management moves old internal5005/host35005 to6005/36005, plus1000. Original Model Hub did not publish the three Ollama ports; the new role-specific host range starts at11434+1000 and11535+1000. Stable aliases are unique to V-Scheduler; the old `model-hub-*` aliases remain owned by Model Hub.

VOS browser model-management prefix: `/app/com.ictrek.v-scheduler/model-hub`. API gateway alternative: `/api/com.ictrek.v-scheduler/model-hub`. Append `/api/v1/...` to either. Opening the unified UI directly at host22888 uses `/model-hub/api/v1/...`. Scheduler APIs remain `/scheduler/api/v1/...` on host22888. Model download tasks and scheduler execution tasks are different resources.

Model management preserves Model Hub's anonymous API contract, including writes. HTTPS does not authenticate these API calls. Keep native management and model-management ports on a trusted network. Scheduler and unified inference authentication remain unchanged; external backend enrollment remains separate from downloading models; the app’s own three Ollama backends are registered automatically.

## Download exact weights

```sh
BASE=http://v-scheduler:6005
# Outside Docker: BASE=http://<vos-host>:36005
curl -fsS --get "$BASE/api/v1/models/repository-files" \
  --data-urlencode source=hf --data-urlencode model_id=org/repo
curl -fsS -X POST "$BASE/api/v1/models/pull" \
  -H 'Content-Type: application/json' \
  -d '{"source":"hf","model_id":"hf://org/repo","files":["config.json","weights/model-Q4_K_M.gguf"]}'
curl -fsS "$BASE/api/v1/tasks/detail/<returned-task-id>"
curl -fsS --get "$BASE/api/v1/models/files" \
  --data-urlencode model_id=hf://org/repo
curl -fsS -X POST "$BASE/api/v1/models/pull" \
  -H 'Content-Type: application/json' \
  -d '{"source":"hf","model_id":"hf://org/repo","files":["weights/model-Q8_0.gguf"],"append":true}'
```

Use source `ms` and ID `ms://org/repo` for ModelScope. Omit `files` for the whole repository. Selected paths must be exact relative paths, with no wildcards, absolute paths or parent traversal. READY means the selected scope completed, not that every file needed by an inference engine is installed. Poll phase `PENDING / DOWNLOADING / VERIFYING / READY / FAILED`; inspect `progress`0..1, `speed`bytes/s, `eta`seconds and `error_msg`. Cancel/retry with `POST /api/v1/tasks/{id}/cancel|retry`. Deleting a task does not delete its model. File metadata lookup does not download weights.

HF mirroring defaults off. Enabling `HF_MIRROR_ENABLED` uses `HF_ENDPOINT`; otherwise the official HuggingFace endpoint is used. Private repository tokens may be supplied to the service environment as `HF_TOKEN` or `MODELSCOPE_API_TOKEN`; never embed credentials in URLs. ModelScope update checks compare master revision timestamps rather than the unchanging branch name.

## Runtime and external backends

The three Ollamas share downloaded blobs/manifests while retaining separate slot/context settings. All preload switches default false. Runtime cards expose installed/running models, memory, slots, phases, throughput and confirmed start/stop. Raw native11434 calls bypass gateway metrics; use11535 for backend inference metrics, or V-Router for unified calls with resource admission.

Register external services with `POST /api/v1/inference-services/register`. Fields: `service_id`, `name`, `kind` (`ollama`, `openai_compatible`, `custom`), `base_url`, optional `gateway_url`, `role`, `metadata`, `api_contract`. Ollama contracts are filled from native APIs. Other kinds require actual implemented contract paths: `health_path`, `version_path`, `models_path`, `running_models_path`, `runtime_status_path`, `metrics_path`, `start_model_path`, `stop_model_path`, `model_action_status_path`. Start/stop sends `{model,mode?,keep_alive?}`; confirmation uses running-state polling and optionally `model/action` query parameters on the action-status path. `accepted` does not imply `confirmed`. Plain vLLM does not supply custom lifecycle endpoints automatically.

Archive import accepts multipart `archive`, `source`, optional `model_id`, `remote_repo`, `name`, `version`, `replace_existing`. ZIP and TAR/gzip/bzip2/xz are supported. Links, traversal and special archive members are rejected. Replacing an installed model requires explicit selection; an extraction failure retains its old files.

Container references prevent model deletion. Removing a reference does not stop another application's container. Aliases resolve through the original string-to-model-ID registry.

Other apps can mount the installer-selected shared directory read-only at `/modelhub`, then read `/modelhub/export/hf/<org>/<repo>/current/...` or `/modelhub/export/ms/...`. For Ollama, mount `ollama/models` at its configured model directory. External downloaders use snapshots and incomplete markers, then publish current only after completion. Automatic scans run every60seconds; `POST /api/v1/models/sync` scans immediately.

The global **About / 关于** button shows the VOS App version, installation profile and each configured image name/tag (without a registry URL), including the separate model-management service and three Ollama instances. Values come from the actual packaged profile, including distinct L4T/Thor Ollama tags; absent metadata is shown as unknown.

Full operation reference: `http://v-scheduler:6005/openapi.json`, host `http://<vos-host>:36005/openapi.json`, and the application API tab.

## Built-in Ollama automatic scheduling

Installing the app authorizes its own three Ollama backends. The controller discovers downloaded models and registers their scoped inference executors and private gateway bindings automatically. No per-model approval, SSH or additional agent is required. Existing Playground and unified `/v1/*` endpoints use these bindings directly.

The initial policy uses a conservative memory estimate of twice the installed model size plus 1 GiB, one shared RAM pool on unified-memory devices, and exclusive use of the node's configured compute budget. This estimate is not a measured peak. Automatic registration preserves the installation context/output settings and explicit inference parameters; automatic mode does not impose a separate smaller context/output limit. The buffered request limit is 32 MiB. When compute is busy, unified requests wait for up to five minutes and honor client cancellation. Calls set `keep_alive=0` to release the model after inference. Normal permit accounting, node drain/revocation and physical resource checks remain active. Runtime discovery reads `/api/show` when tags omit capabilities and excludes rerank models from the QA list.

## Per-model budget calibration

The **Models** page shows a budget state for every model: the automatic estimate, a calibrated budget, direct pass-through (model or upstream), or service pass-through for API-level service rows. Administrators can edit CPU millicores, memory MiB, GPU slots and GPU MiB and compare them with the current automatic estimate before saving. The editor warns that a manual budget replaces the estimator, is not a measured peak, and must fit the node's approved capacity after the resident baseline; invalid or over-capacity values are rejected. "Restore default budget" recalculates the automatic estimate from the current native model size.

For built-in Ollama, a calibrated binding also carries fixed runtime limits for context (`options.num_ctx`), threads, batch, GPU layers and output tokens (`max_tokens`, `options.num_predict`). The UI offers 8192 context, 4 threads, 512 batch, all GPU layers and 4096 output tokens by default. Requests exceeding the approved values are rejected before a permit is acquired; the separate input-byte limit is unchanged. These limits apply only to the calibrated model and do not lower other models' estimates.

Models owned by an application (Image Studio, V-Voice, ML Server, private V-Sub, and other app-provided upstreams) can also be calibrated. Because their executors belong to the owning application, calibration does not replace the executor; it stores an override keyed by upstream URL and native model name in `model-budgets.local.json`, and admission reserves those resources for that model's permits only. This path has no runtime limits and uses `POST /api/v1/model-budgets/calibrate` with `upstream_url` alongside `model_id`; restore needs both fields. The override never changes the owning application's executor or settings, and it only takes effect for requests that acquire a scheduler permit.

"Direct (no limit / not actively scheduled)" removes the executor and runtime limits from the binding. Verified calls are forwarded to the upstream Ollama after the upstream URL check, with no permit, queue, residency accounting or resource admission; V-Scheduler no longer accounts this model's requests. Use it only when another component owns the machine's capacity. A direct binding can be calibrated again or restored to the automatic estimate. API details, defaults and audit events are in [the controller contract](api-contract.md#模型预算校准与单模型放行).

For quick diagnostics, an administrator can also release a single model of any upstream through `PUT /api/v1/model-routing` with `{"model_id":"<upstream_id>/<native>","direct":true}`. The decision is stored in the gateway state file and checked before any scheduler policy, so it also works for app-provided and externally registered models; `direct=false` removes the record and restores normal admission. Upstream-wide direct routing still takes precedence and is managed on the Upstreams page.
