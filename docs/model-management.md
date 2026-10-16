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
| Model management | `v-scheduler-model-hub:6005` | `<vos-host>:36005` | `/api/v1` |
| Ollama QA native management | `v-scheduler-ollama-qa:11434` | `<vos-host>:12434` | `/api` |
| Ollama QA metered inference | `v-scheduler-ollama-qa:11535` | `<vos-host>:12535` | `/v1` |
| Ollama embedding native management | `v-scheduler-ollama-embedding:11434` | `<vos-host>:12435` | `/api` |
| Ollama embedding metered inference | `v-scheduler-ollama-embedding:11535` | `<vos-host>:12536` | `/v1` |
| Ollama rerank native management | `v-scheduler-ollama-rerank:11434` | `<vos-host>:12436` | `/api` |
| Ollama rerank metered inference | `v-scheduler-ollama-rerank:11535` | `<vos-host>:12537` | `/v1` |

Model management moves old internal5005/host35005 to6005/36005, plus1000. Original Model Hub did not publish the three Ollama ports; the new role-specific host range starts at11434+1000 and11535+1000. Stable aliases are unique to V-Scheduler; the old `model-hub-*` aliases remain owned by Model Hub.

VOS browser model-management prefix: `/app/com.ictrek.v-scheduler/model-hub`. API gateway alternative: `/api/com.ictrek.v-scheduler/model-hub`. Append `/api/v1/...` to either. Opening the unified UI directly at host22888 uses `/model-hub/api/v1/...`. Scheduler APIs remain `/scheduler/api/v1/...` on host22888. Model download tasks and scheduler execution tasks are different resources.

Model management preserves Model Hub's anonymous API contract, including writes. HTTPS does not authenticate these API calls. Keep native management and model-management ports on a trusted network. Scheduler and unified inference authentication remain unchanged; downloading a model does not grant an execution approval.

## Download exact weights

```sh
BASE=http://v-scheduler-model-hub:6005
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

Full operation reference: `http://v-scheduler-model-hub:6005/openapi.json`, host `http://<vos-host>:36005/openapi.json`, and the application API tab.
