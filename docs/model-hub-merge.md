# Model Hub integration acceptance

The Model Hub implementation is ported to Rust/Axum and Vue/TypeScript. The Ollama implementation remains in the Ollama submodule; packaging resolves `ollama_server` from its original platform sheets. No Python or React service is shipped.

## Compatibility boundary

Installer `MODEL_HUB_SHARED_MODELS_PATH`, default `/data/vos_workspace/model_hub`, retains hint `[com.ictrek.model.storage]`. Both applications mount it at `/data`. Preserve `registry/{models,aliases,containers,inference_services}.json`, `shared/tasks/*.json`, Ollama blobs/manifests and HF/MS export/snapshots/current. Registry locks use the same Linux flock files under `shared/locks`. New downloads stage separately and publish current only after all selected files pass validation. Existing files are never moved on startup. Switching back to Model Hub uses the same directory and registry schema. Do not run competing downloads for the same model across apps; fresh external task records block a second writer.

## Feature checklist

- [x] Local models: status filters, refresh/sync, file list, access path, size, safe delete, update checks and batch checks.
- [x] Downloads: Ollama/HF/MS, remote file metadata, searchable selection, full repo or exact file list, append preserving old files, retry/resume, cancellation, progress/speed/ETA, persistent task history, cleanup.
- [x] Archive import: multipart zip/tar, safe extraction, optional repository/version, replace guard, preserved old version on failure.
- [x] Runtime: three builtin Ollamas, downloaded/running/failed models, metrics, slots and phases, start/stop; external Ollama/OpenAI/custom registration/contracts/cards/actions.
- [x] Chat test: reuse the existing V-Scheduler playground; no duplicate Model Hub chat page/API. Router directly connects its own three Ollamas as v-scheduler/ollama-{qa,embedding,rerank}, without querying Model Hub.
- [x] Aliases and container references: preserved data, management APIs and discoverable UI.
- [x] API guide: exact file download/append examples, task lifecycle, file mount layouts, external service contract, complete operation list.
- [x] Installation: every old Model Hub parameter, preloads false, native platform Ollama images, stable unique DNS aliases, all necessary host ports.
- [ ] Release validation: Rust tests, existing scheduler tests, Vue build, fixture API/download/import/takeover/runtime checks, native AMD/ARM images, Feishu readback, one complete release and App Store publication.

## Endpoint plan

Model management internal `v-scheduler-model-hub:6005`, host default `36005` (old Model Hub 5005/35005 plus1000). Preserve original `/api/v1/...` model APIs on this listener. Embedded UI uses `/app/com.ictrek.v-scheduler/model-hub/api/v1/...`, keeping scheduler task APIs separate. Unified inference `v-router:8080` / host22888. Scheduler `v-scheduler:38120` / configurable host38120. Ollama DNS `v-scheduler-ollama-{qa,embedding,rerank}`; native11434 and gateway11535, each role gets distinct configurable host mappings. Original Model Hub did not expose the three Ollamas on host ports; use a documented consecutive range in the new app instead of claiming nonexistent old mappings.

## Verification before publication

Native AMD and ARM Rust suites: 34 tests each. Vue: 10 tests and production build. Packaging/dependency contracts: 14 tests. Isolated API lifecycle: 46 checks on each native runtime image, including unchanged legacy inode/bytes, selected downloads, resume, rollback, runtime confirmation and version metadata. Browser checks cover the unified API documentation entry, removal of the duplicate model-library guide tab, model/file selection, runtime cards, all three locales, mobile width, existing Playground streaming and global About beside language on API and model-library pages. Both runtime images pass ZIP/TAR/gzip/bzip2/xz imports, flat model IDs, config-only archives and 3 MiB multipart uploads. AMD/ARM scheduler and gateway images use the fixed 20261016 tags; original Ollama images remain unchanged. Registry manifests and four Feishu component cells are checked before the single package release.
