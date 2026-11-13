# V-Scheduler complete API integration

## Navigation and contract fields

In the application open **API Documentation**. Choose **Model & Gateway API**, **Scheduler Management API**, **Model Management API**, or **Application Integration**. Use the quick index/search and expand an operation to inspect path/query/header parameters, required/default/enum values, nested bodies and every declared response status/content type. OpenAPI download and shared-schema references are supplied in the same view. This is the delivered implementation contract, not an unversioned endpoint list.

The source contracts are [gateway](https://github.com/ictrektech/V-Scheduler/blob/main/api/router-openapi-en.json), [controller](https://github.com/ictrektech/V-Scheduler/blob/main/api/openapi-v1.json), and [weights](https://github.com/ictrektech/V-Scheduler/blob/main/api/model-management.openapi.json). The operation catalogue below identifies all three namespaces; they must not be interchanged. Gateway management and the controller both use `/api/v1` relative to **different** base addresses.

## Canonical addresses and compatibility

| Function | Recommended Docker address | Default host mapping |
| --- | --- | --- |
| Unified inference and gateway management | `http://v-scheduler:8080` | `http://<HOST>:22888` |
| Controller direct API | `http://v-scheduler:38120/api/v1` | `http://<HOST>:38120/api/v1` |
| Controller through unified gateway | `http://v-scheduler:8080/scheduler/api/v1` | `http://<HOST>:22888/scheduler/api/v1` |
| Weight and inference-service management | `http://v-scheduler:6005` | `http://<HOST>:36005` |
| QA Ollama native / compatible API | `v-scheduler-ollama-qa:11434` / `:11535` | `<HOST>:12434` / `:12535` |
| Embedding Ollama native / compatible API | `v-scheduler-ollama-embedding:11434` / `:11535` | `<HOST>:12435` / `:12536` |
| Rerank Ollama native / compatible API | `v-scheduler-ollama-rerank:11434` / `:11535` | `<HOST>:12436` / `:12537` |

All external ports are installation-dependent; copy the actual **API Documentation → Application Integration** address table. Frontend same-origin gateway base is `/api/com.ictrek.v-scheduler`; controller appends `/scheduler/api/v1`. The canonical host is `v-scheduler`. Compatibility names such as `v-router`, `v-scheduler-gateway`, `v-scheduler-backend`, `v-scheduler-frontend` and `v-scheduler-model-hub` point to the appropriate existing unified service/listener; they do not represent separate router/weight containers. Keep old saved clients working, but recommend the canonical address for new integrations. Ollama aliases identify separate execution containers.

## VOS identity, public/private models and inference Keys

An authorized VOS user access token works for discovery and inference. OpenAI clients use `Authorization: Bearer $TOKEN_OR_KEY`; Anthropic accepts `x-api-key: $TOKEN_OR_KEY`. Personal inference Keys act as their owning VOS user with their allowed model scope; model Keys act only for their bound model. They do **not** grant management privileges. An upstream provider credential is stored on the upstream and used only by V-Scheduler to call that provider; clients use Scheduler-issued Keys, not the vendor secret.

Public models accept no Key, a valid Key, or an arbitrary/invalid placeholder Key. An invalid Key gains anonymous public-only access and never gains private access. Private models require an entitled VOS identity, personal Key or valid model Key. Visibility and direct admission are separate controls: making a model public does not approve compute resources. Existing models/upstreams can be edited between public/private using the corresponding owner/admin configuration operation.

Open **Advanced → API Keys** to create/list/revoke personal Keys after installation. No reinstall is required. Key management and model-specific Key assignment require the appropriate VOS owner/admin identity; use that identity for management calls, not an inference Key. Port6005 weight management has no independent API authentication; restrict it to a trusted management network. A gateway Key does not protect direct port6005.

## Create, set, rotate and revoke Keys through the API

```bash
GATEWAY='http://<HOST>:22888'
# VOS_TOKEN is the authorized owner's VOS access token, provided privately.
curl --fail-with-body -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' \
  -d '{"name":"external-agent","expires_in_days":30}' "$GATEWAY/api/v1/api-keys"
```

Creation returns the generated value in `api_key`, with safe record metadata in `key`. Copy the secret privately; list/metadata reads never retrieve the full value or its hash. DELETE `/api/v1/api-keys/{id}` revokes the record immediately and returns a JSON acknowledgement (not204). To rotate a personal Key, create a replacement, change the client and revoke the old one.

For a model-specific Key use PUT `/api/v1/models/access-key` with `upstream_id`, `model` and `access_key`. `access_key:true` generates a new secret and returns it once; a caller-supplied string sets that value without echoing it back; null/false clears the Key and the response's key is null. Generated per-model results are in `model_access_keys`, each with its bound model and record metadata. Never treat an absent `api_key` in a custom-set response as a failure. The creation/reset response is the only place to read a generated secret; later list/read responses contain metadata only. Setting a replacement revokes the old model Key. A model Key for one private model cannot read a different private model.

Model registration can include `access_key:true` or a custom Key policy in the same POST; explicit re-registration can set/change it. Omission preserves an existing binding rather than silently clearing it. Provider `api_key` is an entirely different field and never becomes a client's access Key. See the operation schemas for per-model maps, TTL bounds and ownership checks.

## Add upstreams, edit models and identify capabilities

Open **Models & Upstreams** or the first-page **Configure upstream models** action. One configuration is shared by VOS apps according to permissions. Models are grouped by upstream/application source; source editing, routing/admission and advanced actions belong to the source card, while metadata, visibility, capabilities/endpoints and model overrides remain in model settings. Downloads and selected weight files are in **Weight Management**. A manually added model still has its upstream URL/identity.

POST `/api/v1/upstreams` creates a persistent cloud/manual source (201); use its returned ID for edit/refresh/routing/delete. Provider presets are deduplicated from Models.dev; directory entries describe available integrations, not proof of an account's entitlement. Existing native pi-ai execution handles its provider-specific authentication/OAuth where configured; transparent compatible protocols remain available. Vendor Keys/private OAuth credentials are write-only and are not returned by management reads. Mainland-network restrictions may block foreign provider APIs even when local configuration is correct: test connectivity separately and use an authorized reachable endpoint.

GET `/v1/models` returns canonical inference IDs. GET gateway `/api/v1/models` returns richer per-upstream records: native `id`, canonical `qualified`, `upstream_id`, capabilities/endpoints and routing metadata. Use `qualified` for normal inference. To pin an upstream explicitly, use its native model ID with `X-VRouter-Upstream: <upstream_id>`. Different upstream prefixes retain separate same-name models; do not reverse a hostname/IP or reconstruct a qualified ID yourself. Old stored IDs are accepted for compatibility while new display/save uses canonical IDs.

PATCH gateway `/api/v1/models` edits metadata; explicit capability/endpoints override detection. Ollama uses `/api/show` for completion/vision/tools/thinking/embedding/rerank/decision where actually supported, and retains manual overrides until re-identification is requested. A plain upstream model list may omit capabilities; do not declare tools/vision merely because a model is conversational. Explicit dedicated endpoints take precedence over chat defaults. Jev uses `decision`/`systemone`; OpenAI Decisions uses `decision`/`decisions` and is not a chat model.

## Registration and scheduling policies

POST `/api/v1/registrations` registers/discovers a service. Registration is **not** budget approval. `service_id` identifies that instance consistently across its weight-management card, gateway route and executor binding. Automatic discovered/registered sources heartbeat; persistent manually configured upstreams do not require a lease heartbeat.

The administrator-only `scheduler` policy accepts:

| mode | Meaning |
| --- | --- |
| omitted | Managed service; needs an approved resource binding before compute |
| direct | Same upstream `routing.direct` setting; bypass permits, budgets, route concurrency and circuit-breaker checks; ownership, pause and protocol checks remain |
| cloud | Explicit remote independently managed compute; no local reservation, but ordinary gateway routing controls still apply |
| permit | Approved `executor_id`, `completion:"synchronous"`, matching per-model resource budget |
| delegated | Native application owns asynchronous completion and confirmed stopping |

PUT `/api/v1/upstreams/{id}/routing` replaces the whole routing configuration, including `weight,max_inflight,disabled,direct`. Encode the upstream ID as one URL path segment (for example, `v-scheduler%2Follama-qa`). Model-level PUT `/api/v1/model-routing` uses internal `upstream_id/native_model_id` plus `direct`; setting false removes the model override but cannot override an upstream-wide direct setting. A normal heartbeat does not overwrite an operator's routing change. Explicit re-registration with a policy is a configuration change. A missing binding is a specific503 error, not an automatic grant of zero budget.

```json
{"service_id":"my-infer","protocol":"chat_completions","base_url":"http://my-infer:8000/v1","visibility":"private","models":[{"id":"chat-a","capabilities":["chat","completion"],"endpoints":["chat_completions"]}],"scheduler":{"mode":"cloud"},"heartbeat_timeout_seconds":90}
```

Send the saved full snapshot to `/api/v1/registrations/{returned_id}/heartbeat` at the returned heartbeat interval. It renews health or restores an entry after restart. Use explicit re-registration for model/metadata/visibility/policy changes. `healthy:false` hides a service without releasing active reservations. Default heartbeat timeout90seconds, accepted30–3600; `ttl_seconds` is the compatibility alias. A missing heartbeat removes unavailable discovery entries; it does not prove a running computation has stopped.

## Nodes, resources, application heartbeats and budgets

Use the controller base, not gateway management. VOS apps include the reporting/execution components: users must not manually deploy agents on each machine.

1. Administrator POST `/enrollments` for `node_id`, issuing a one-time invitation.
2. The embedded node component POSTs `/workers/register` using the invitation Bearer and `protocol_version:1,node_id,display_name,instance_id,boot_id,session_id,worker_token`.
3. Administrator POST `/nodes/{id}/approve` for actual capacity, resident baseline and safety reserve.
4. Node worker POST `/workers/{id}/executors` with the execution definitions, per-execution `resources` and reachable `upstream_urls`; administrator approves the current executor definition.
5. Node worker sends `/workers/{id}/heartbeat` with the matching session and real `report.observed_at`, CPU/RAM/GPU/executor-health telemetry.
6. Worker or administrator GET `/workers/{id}/resources` to read approved capacity, resident baseline, reservations, remaining budget, safety reserve and fresh telemetry.

`remaining_budget` is approved capacity minus resident baseline/unreleased reservations; actual available RAM is `report.memory_available_mib`. Independent GPU memory uses `report.gpus[].free_mib`; unified memory shares host RAM and must not be counted twice. Stale telemetry sets `telemetry_fresh=false,report=null`. Resource visibility is not a guaranteed permit. Reusing an already resident model shares its weight memory; per-request temporary/KV/workspace budgets still depend on the actual request and concurrency.

An application service identity registers its approved executors through POST `/integrations`:

```json
{"protocol_version":1,"revision":0,"executors":["approved-executor-id"],"stop_protocol":"permit-native-exit-v1"}
```

Save the returned revision and heartbeat at `heartbeat_after_seconds` using POST `/integrations/heartbeat` with that revision. Heartbeat includes pending stop commands. Registration grants no arbitrary budget or control URL. Stale application identity pauses new admission; recovery resumes only if the approved definitions still match. GET `/integrations` is self-scoped for an application and administrator-wide for an administrator. Conflicting revisions or changing executor sets with active/unreleased work return409; read state before retry. DELETE `/integrations` refuses while active work/stop requests remain. Node telemetry, application heartbeat and model discovery heartbeat serve different purposes and cannot substitute for each other.

## Queue priority, permits and confirmed stopping

Scheduler prioritizes **tasks/requests**, not permanent model importance. Administrator direct/force behavior has the documented authority boundary; ordinary applications can request allowed task priority and queue options. Priority ordering does not permit unconfirmed reclamation: cooperative preemption asks the lower-priority execution to stop/unload, preserves its resumable task state where supported, and only admits higher-priority work after native confirmation frees resources. A lower-priority task may resume/retry later according to its contract.

Synchronous/streaming approved executors use permits, renewed while computing; asynchronous apps retain their business job/output API and submit durable tasks/controlled executions. Do not reserve the same compute twice. POST `/permits/{id}/stop` requests stopping and does **not** immediately release resources. The registered executor sends `/permits/{id}/stopped` with the matching identity/execution fence only after confirming exit and unloading. Lost heartbeat, timeout or stream disconnect is not proof that resources were released. Use the returned command/revision/lease values, not a copied constant; replay must remain idempotent. Reserved ModelInfer control returns501 pending validation and does not execute a remote stop.

## Weight and inference-service management

Use the installed shared model directory selected during installation; no copying or redownloading is required to adopt prior Model Hub weights. Port6005 serves weight management and inference-service cards, not OpenAI chat. Enumerate models/files, preview selection, submit pull/update/append requests, and poll download tasks through the declared API. A selected-file download is not proof that every repository file is present; ready applies to the selected scope. Verify the required weight file(s) and actual runtime readiness separately.

Use the same instance/model mapping for a weight-management service card, gateway registration and approved executor. Load/unload, task cancellation and model deletion are different operations with different side effects. A successful HTTP submission is not a terminal task result. VOS owns the built-in three Ollama execution containers and their shared paths; external clients should normally use the unified inference gateway rather than bypass resource admission at a raw Ollama port. Direct maintenance addresses are for the appropriate trusted management workflow.

## OpenAI, Anthropic, Auto and native decision calls

OpenAI Chat Completions and Anthropic Messages can call the same compatible model even if the upstream only exposes the other chat protocol. Use the exact catalogue ID in either request. Tool/media support, context/output limits and configured upstream tool parsing still apply. Scheduler admission cannot increase an8192-token upstream context limit or enable missing vLLM tool parsing.

```bash
MODEL='<canonical id from /v1/models>'
curl --fail-with-body -H "Authorization: Bearer $SCHEDULER_KEY" -H 'Content-Type: application/json' \
  -d "{\"model\":\"$MODEL\",\"messages\":[{\"role\":\"user\",\"content\":\"Hello\"}],\"stream\":false}" \
  "$GATEWAY/v1/chat/completions"
curl --fail-with-body -H "x-api-key: $SCHEDULER_KEY" -H 'anthropic-version: 2023-06-01' -H 'Content-Type: application/json' \
  -d "{\"model\":\"$MODEL\",\"max_tokens\":128,\"messages\":[{\"role\":\"user\",\"content\":\"Hello\"}],\"stream\":false}" \
  "$GATEWAY/v1/messages"
```

For a public model omit identity or provide an arbitrary placeholder; for a private model use its allowed Key/VOS identity. Never expose a private owner token in a public curl example. Set `stream:true` and use a real SSE parser for streaming; OpenAI `[DONE]`, Anthropic `message_stop` and Responses terminal events differ. Usage-only, tool and reasoning deltas may contain no ordinary answer text. Read terminal error/incomplete states even after HTTP200.

`auto` is an always-discoverable virtual chat model, usable with both `/v1/chat/completions` and `/v1/messages`. Configure it in the UI or administrator GET/PUT `/api/v1/auto-routing`; inspect `/candidates` and use POST `/api/v1/auto-routing/test`. Local rules are the default; request type, context size, chat classifier, Jev/System One and OpenAI Decisions are selectable strategies/classifiers. Candidate visibility/admission still apply; an empty explicit list uses eligible visible chat models, while no eligible candidate returns503. The legacy enabled flag does not hide Auto.

Native OpenAI Decisions uses `/v1/decisions` with `model,input,questions[]` and `answers[]`; Jev uses `/v1/systemone` with `model,state,questions` and name-keyed answers. Choice task/complexity/confidence mapping is documented by the corresponding schema; OpenAI bridge validates choice/confidence and named answers, while media or unsupported question types must use native Decisions. These are classifiers, not ordinary chat models. Preserve their native usage/probability/confidence fields and explicit capability/endpoints.

## Errors, retry and diagnostics

Use HTTP status plus the gateway's `code,category,msg,data,request_id,error` fields where returned. Native upstream errors may retain another JSON shape or be non-JSON; controller errors and weight errors follow their own schemas. Missing binding/budget is503 with a specific cause, resource/queue saturation is429, permission/identity errors401/403, state/revision conflict409, and reserved operations501. Read the documented response for each operation; do not treat these namespaces as one universal envelope.

Transient VOS login expiry is automatically renewed where the app bridge supports it; user-facing screens retain useful state while technical detail goes to **Advanced → Diagnostic Logs**. An expired VOS token is not an upstream model capability error. Writes and model/task submissions have operation-specific idempotency: after a timeout query current state before repeating a destructive or expensive action. Do not infer cancellation or freed resources from a single successful retry. Mainland-network restrictions can independently block foreign upstreams; local management success is not a remote connectivity test.


## Gateway — base http://v-scheduler:8080

| Method | Path | Operation |
| --- | --- | --- |
| GET | `/api/v1/apidoc` | API documentation data |
| GET | `/api/v1/auth-probe` | Authentication diagnostics |
| GET | `/api/v1/auth/me` | Verify caller identity |
| GET | `/api/v1/models` | Model list with metadata |
| PATCH | `/api/v1/models` | Update model metadata |
| GET | `/api/v1/openapi.json` | OpenAPI 3.1 contract |
| POST | `/api/v1/registrations` | Register or update an inference backend |
| DELETE | `/api/v1/registrations/{id}` | Unregister an inference backend |
| POST | `/api/v1/registrations/{id}/heartbeat` | Renew a registration lease |
| POST | `/api/v1/scenes/detect` | Detect video scenes synchronously |
| GET | `/api/v1/status` | Gateway status |
| GET | `/api/v1/upstreams` | Upstream list |
| POST | `/api/v1/upstreams` | Add a cloud or manual upstream |
| POST | `/api/v1/upstreams/probe` | Probe a cloud/manual upstream |
| DELETE | `/api/v1/upstreams/{id}` | Delete a cloud or manual upstream |
| PUT | `/api/v1/upstreams/{id}` | Update a cloud or manual upstream |
| POST | `/api/v1/upstreams/{id}/refresh` | Refresh upstream models |
| PUT | `/api/v1/upstreams/{id}/routing` | Configure upstream scheduling |
| GET | `/api/v1/usage` | Usage statistics |
| GET | `/api/v1/video-structure/status` | Video structure discovery status |
| POST | `/api/v2/scenes/detect/jobs` | Submit a scene detection job |
| GET | `/api/v2/scenes/detect/jobs/{job_id}` | Query a scene detection job |
| POST | `/api/v2/video/describe/jobs` | Submit a scene description job |
| GET | `/api/v2/video/describe/jobs/{job_id}` | Query a scene description job |
| POST | `/api/v2/video/structure/jobs` | Submit a video structure job |
| DELETE | `/api/v2/video/structure/jobs/{job_id}` | Release a video structure artifact |
| GET | `/api/v2/video/structure/jobs/{job_id}` | Query a video structure job |
| GET | `/api/v2/video/structure/jobs/{job_id}/artifact` | Download a video structure artifact |
| GET | `/healthz` | Health check |
| DELETE | `/v1/assistants` | assistants · pinned upstream forwarding |
| GET | `/v1/assistants` | assistants · pinned upstream forwarding |
| PATCH | `/v1/assistants` | assistants · pinned upstream forwarding |
| POST | `/v1/assistants` | assistants · pinned upstream forwarding |
| DELETE | `/v1/assistants/{resource_path}` | assistants · pinned upstream forwarding |
| GET | `/v1/assistants/{resource_path}` | assistants · pinned upstream forwarding |
| PATCH | `/v1/assistants/{resource_path}` | assistants · pinned upstream forwarding |
| POST | `/v1/assistants/{resource_path}` | assistants · pinned upstream forwarding |
| DELETE | `/v1/audio` | audio · pinned upstream forwarding |
| GET | `/v1/audio` | audio · pinned upstream forwarding |
| PATCH | `/v1/audio` | audio · pinned upstream forwarding |
| POST | `/v1/audio` | audio · pinned upstream forwarding |
| POST | `/v1/audio/music/generations` | Music generation (OpenAI-style extension) |
| GET | `/v1/audio/speakers` | Speaker library |
| POST | `/v1/audio/speakers/identify` | Speaker identification 1:N |
| POST | `/v1/audio/speakers/verify` | Speaker verification 1:1 |
| POST | `/v1/audio/speech` | Text-to-speech |
| POST | `/v1/audio/streaming/transcriptions` | Streaming ASR (HTTP chunked upload) |
| GET | `/v1/audio/streaming/ws` | Streaming ASR (realtime WebSocket) |
| POST | `/v1/audio/transcriptions` | ASR transcription (hotword text, files and boost scores) |
| POST | `/v1/audio/translations` | Audio translation |
| GET | `/v1/audio/voices` | TTS voice list |
| DELETE | `/v1/audio/{resource_path}` | audio · pinned upstream forwarding |
| GET | `/v1/audio/{resource_path}` | audio · pinned upstream forwarding |
| PATCH | `/v1/audio/{resource_path}` | audio · pinned upstream forwarding |
| POST | `/v1/audio/{resource_path}` | audio · pinned upstream forwarding |
| DELETE | `/v1/batches` | batches · pinned upstream forwarding |
| GET | `/v1/batches` | batches · pinned upstream forwarding |
| PATCH | `/v1/batches` | batches · pinned upstream forwarding |
| POST | `/v1/batches` | batches · pinned upstream forwarding |
| DELETE | `/v1/batches/{resource_path}` | batches · pinned upstream forwarding |
| GET | `/v1/batches/{resource_path}` | batches · pinned upstream forwarding |
| PATCH | `/v1/batches/{resource_path}` | batches · pinned upstream forwarding |
| POST | `/v1/batches/{resource_path}` | batches · pinned upstream forwarding |
| DELETE | `/v1/chat/completions` | chat/completions · pinned upstream forwarding |
| GET | `/v1/chat/completions` | chat/completions · pinned upstream forwarding |
| PATCH | `/v1/chat/completions` | chat/completions · pinned upstream forwarding |
| POST | `/v1/chat/completions` | Chat completions (SSE streaming) |
| DELETE | `/v1/chat/completions/{resource_path}` | chat/completions · pinned upstream forwarding |
| GET | `/v1/chat/completions/{resource_path}` | chat/completions · pinned upstream forwarding |
| PATCH | `/v1/chat/completions/{resource_path}` | chat/completions · pinned upstream forwarding |
| POST | `/v1/chat/completions/{resource_path}` | chat/completions · pinned upstream forwarding |
| POST | `/v1/completions` | Text completions |
| DELETE | `/v1/containers` | containers · pinned upstream forwarding |
| GET | `/v1/containers` | containers · pinned upstream forwarding |
| PATCH | `/v1/containers` | containers · pinned upstream forwarding |
| POST | `/v1/containers` | containers · pinned upstream forwarding |
| DELETE | `/v1/containers/{resource_path}` | containers · pinned upstream forwarding |
| GET | `/v1/containers/{resource_path}` | containers · pinned upstream forwarding |
| PATCH | `/v1/containers/{resource_path}` | containers · pinned upstream forwarding |
| POST | `/v1/containers/{resource_path}` | containers · pinned upstream forwarding |
| DELETE | `/v1/conversations` | conversations · pinned upstream forwarding |
| GET | `/v1/conversations` | conversations · pinned upstream forwarding |
| PATCH | `/v1/conversations` | conversations · pinned upstream forwarding |
| POST | `/v1/conversations` | conversations · pinned upstream forwarding |
| DELETE | `/v1/conversations/{resource_path}` | conversations · pinned upstream forwarding |
| GET | `/v1/conversations/{resource_path}` | conversations · pinned upstream forwarding |
| PATCH | `/v1/conversations/{resource_path}` | conversations · pinned upstream forwarding |
| POST | `/v1/conversations/{resource_path}` | conversations · pinned upstream forwarding |
| POST | `/v1/embeddings` | Text embeddings |
| DELETE | `/v1/files` | files · pinned upstream forwarding |
| GET | `/v1/files` | files · pinned upstream forwarding |
| PATCH | `/v1/files` | files · pinned upstream forwarding |
| POST | `/v1/files` | files · pinned upstream forwarding |
| DELETE | `/v1/files/{resource_path}` | files · pinned upstream forwarding |
| GET | `/v1/files/{resource_path}` | files · pinned upstream forwarding |
| PATCH | `/v1/files/{resource_path}` | files · pinned upstream forwarding |
| POST | `/v1/files/{resource_path}` | files · pinned upstream forwarding |
| DELETE | `/v1/fine_tuning` | fine_tuning · pinned upstream forwarding |
| GET | `/v1/fine_tuning` | fine_tuning · pinned upstream forwarding |
| PATCH | `/v1/fine_tuning` | fine_tuning · pinned upstream forwarding |
| POST | `/v1/fine_tuning` | fine_tuning · pinned upstream forwarding |
| DELETE | `/v1/fine_tuning/{resource_path}` | fine_tuning · pinned upstream forwarding |
| GET | `/v1/fine_tuning/{resource_path}` | fine_tuning · pinned upstream forwarding |
| PATCH | `/v1/fine_tuning/{resource_path}` | fine_tuning · pinned upstream forwarding |
| POST | `/v1/fine_tuning/{resource_path}` | fine_tuning · pinned upstream forwarding |
| DELETE | `/v1/images` | images · pinned upstream forwarding |
| GET | `/v1/images` | images · pinned upstream forwarding |
| PATCH | `/v1/images` | images · pinned upstream forwarding |
| POST | `/v1/images` | images · pinned upstream forwarding |
| POST | `/v1/images/edits` | Image editing |
| POST | `/v1/images/generations` | Image generation |
| POST | `/v1/images/variations` | Image variations |
| DELETE | `/v1/images/{resource_path}` | images · pinned upstream forwarding |
| GET | `/v1/images/{resource_path}` | images · pinned upstream forwarding |
| PATCH | `/v1/images/{resource_path}` | images · pinned upstream forwarding |
| POST | `/v1/images/{resource_path}` | images · pinned upstream forwarding |
| DELETE | `/v1/messages` | messages · pinned upstream forwarding |
| GET | `/v1/messages` | messages · pinned upstream forwarding |
| PATCH | `/v1/messages` | messages · pinned upstream forwarding |
| POST | `/v1/messages` | Anthropic Messages |
| DELETE | `/v1/messages/{resource_path}` | messages · pinned upstream forwarding |
| GET | `/v1/messages/{resource_path}` | messages · pinned upstream forwarding |
| PATCH | `/v1/messages/{resource_path}` | messages · pinned upstream forwarding |
| POST | `/v1/messages/{resource_path}` | messages · pinned upstream forwarding |
| POST | `/v1/ml/predictions` | ML Server image inference extension |
| POST | `/v1/ml/video/structure` | ML Server video structure task |
| GET | `/v1/ml/video/structure/tasks/{task_id}` | Query an ML Server video structure task |
| DELETE | `/v1/models` | models · pinned upstream forwarding |
| GET | `/v1/models` | Model list |
| PATCH | `/v1/models` | models · pinned upstream forwarding |
| POST | `/v1/models` | models · pinned upstream forwarding |
| DELETE | `/v1/models/{resource_path}` | models · pinned upstream forwarding |
| GET | `/v1/models/{resource_path}` | models · pinned upstream forwarding |
| PATCH | `/v1/models/{resource_path}` | models · pinned upstream forwarding |
| POST | `/v1/models/{resource_path}` | models · pinned upstream forwarding |
| POST | `/v1/moderations` | Content moderation |
| DELETE | `/v1/realtime` | realtime · pinned upstream forwarding |
| GET | `/v1/realtime` | realtime · pinned upstream forwarding |
| PATCH | `/v1/realtime` | realtime · pinned upstream forwarding |
| POST | `/v1/realtime` | realtime · pinned upstream forwarding |
| DELETE | `/v1/realtime/{resource_path}` | realtime · pinned upstream forwarding |
| GET | `/v1/realtime/{resource_path}` | realtime · pinned upstream forwarding |
| PATCH | `/v1/realtime/{resource_path}` | realtime · pinned upstream forwarding |
| POST | `/v1/realtime/{resource_path}` | realtime · pinned upstream forwarding |
| POST | `/v1/rerank` | Reranking (compatible extension, not an OpenAI standard) |
| DELETE | `/v1/responses` | responses · pinned upstream forwarding |
| GET | `/v1/responses` | responses · pinned upstream forwarding |
| PATCH | `/v1/responses` | responses · pinned upstream forwarding |
| POST | `/v1/responses` | Responses multimodal output |
| POST | `/v1/responses/compact` | Responses context compaction |
| DELETE | `/v1/responses/{resource_path}` | responses · pinned upstream forwarding |
| GET | `/v1/responses/{resource_path}` | responses · pinned upstream forwarding |
| PATCH | `/v1/responses/{resource_path}` | responses · pinned upstream forwarding |
| POST | `/v1/responses/{resource_path}` | responses · pinned upstream forwarding |
| DELETE | `/v1/skills` | skills · pinned upstream forwarding |
| GET | `/v1/skills` | skills · pinned upstream forwarding |
| PATCH | `/v1/skills` | skills · pinned upstream forwarding |
| POST | `/v1/skills` | skills · pinned upstream forwarding |
| DELETE | `/v1/skills/{resource_path}` | skills · pinned upstream forwarding |
| GET | `/v1/skills/{resource_path}` | skills · pinned upstream forwarding |
| PATCH | `/v1/skills/{resource_path}` | skills · pinned upstream forwarding |
| POST | `/v1/skills/{resource_path}` | skills · pinned upstream forwarding |
| DELETE | `/v1/threads` | threads · pinned upstream forwarding |
| GET | `/v1/threads` | threads · pinned upstream forwarding |
| PATCH | `/v1/threads` | threads · pinned upstream forwarding |
| POST | `/v1/threads` | threads · pinned upstream forwarding |
| DELETE | `/v1/threads/{resource_path}` | threads · pinned upstream forwarding |
| GET | `/v1/threads/{resource_path}` | threads · pinned upstream forwarding |
| PATCH | `/v1/threads/{resource_path}` | threads · pinned upstream forwarding |
| POST | `/v1/threads/{resource_path}` | threads · pinned upstream forwarding |
| DELETE | `/v1/uploads` | uploads · pinned upstream forwarding |
| GET | `/v1/uploads` | uploads · pinned upstream forwarding |
| PATCH | `/v1/uploads` | uploads · pinned upstream forwarding |
| POST | `/v1/uploads` | uploads · pinned upstream forwarding |
| DELETE | `/v1/uploads/{resource_path}` | uploads · pinned upstream forwarding |
| GET | `/v1/uploads/{resource_path}` | uploads · pinned upstream forwarding |
| PATCH | `/v1/uploads/{resource_path}` | uploads · pinned upstream forwarding |
| POST | `/v1/uploads/{resource_path}` | uploads · pinned upstream forwarding |
| DELETE | `/v1/vector_stores` | vector_stores · pinned upstream forwarding |
| GET | `/v1/vector_stores` | vector_stores · pinned upstream forwarding |
| PATCH | `/v1/vector_stores` | vector_stores · pinned upstream forwarding |
| POST | `/v1/vector_stores` | vector_stores · pinned upstream forwarding |
| DELETE | `/v1/vector_stores/{resource_path}` | vector_stores · pinned upstream forwarding |
| GET | `/v1/vector_stores/{resource_path}` | vector_stores · pinned upstream forwarding |
| PATCH | `/v1/vector_stores/{resource_path}` | vector_stores · pinned upstream forwarding |
| POST | `/v1/vector_stores/{resource_path}` | vector_stores · pinned upstream forwarding |
| DELETE | `/v1/videos` | videos · pinned upstream forwarding |
| GET | `/v1/videos` | videos · pinned upstream forwarding |
| PATCH | `/v1/videos` | videos · pinned upstream forwarding |
| POST | `/v1/videos` | videos · pinned upstream forwarding |
| POST | `/v1/videos/generations` | Video generation |
| DELETE | `/v1/videos/{resource_path}` | videos · pinned upstream forwarding |
| GET | `/v1/videos/{resource_path}` | videos · pinned upstream forwarding |
| PATCH | `/v1/videos/{resource_path}` | videos · pinned upstream forwarding |
| POST | `/v1/videos/{resource_path}` | videos · pinned upstream forwarding |
| GET | `/readyz` | Model catalog readiness |
| GET | `/api/v1/inference-queue` | Inference waiting queue |
| DELETE | `/api/v1/inference-queue/{id}` | Cancel waiting inference |
| GET | `/api/v1/model-residency` | Model residency and unloading policy |
| PUT | `/api/v1/model-residency` | Set idle retention duration |
| GET | `/api/v1/model-lifecycle` | Weights, services, routes and scheduler linkage |
| POST | `/v1/decisions` | OpenAI Decisions |
| POST | `/v1/systemone` | Jev / System One typed decisions |
| GET | `/api/v1/auto-routing` | Auto configuration |
| PUT | `/api/v1/auto-routing` | Set Auto routing |
| POST | `/api/v1/auto-routing/test` | Test Auto routing |
| GET | `/api/v1/auto-routing/candidates` | Auto candidate catalog |
| GET | `/api/v1/provider-catalog` | Models.dev provider catalog |
| GET | `/api/v1/provider-catalog/{provider}` | Models.dev provider catalog |
| GET | `/api/v1/api-keys` | My API keys |
| POST | `/api/v1/api-keys` | Create a personal API key |
| DELETE | `/api/v1/api-keys/{id}` | Revoke an API key |
| PUT | `/api/v1/models/access-key` | Set, rotate or revoke a model access key |
| PUT | `/api/v1/model-routing` | Set per-model direct admission |


## Controller — base http://v-scheduler:38120/api/v1

| Method | Path | Operation |
| --- | --- | --- |
| GET | `/discovery` | Discover capabilities |
| GET | `/openapi.json` | Read API specification |
| GET | `/executors` | Available executors |
| POST | `/tasks` | Submit a task |
| GET | `/tasks` | List tasks |
| GET | `/tasks/{id}` | Get task details |
| DELETE | `/tasks/{id}` | Delete task record |
| POST | `/tasks/{id}/cancel` | Cancel task |
| POST | `/tasks/{id}/retry` | Retry task manually |
| GET | `/tasks/{id}/attempts` | Attempt history |
| GET | `/events` | Incremental events |
| GET | `/nodes` | Nodes and placements |
| POST | `/nodes/{id}/drain` | Drain a node |
| GET | `/reservations` | Unreleased reservations |
| POST | `/permits` | Acquire online permit |
| GET | `/permits` | List scoped permit history |
| POST | `/permits/{id}/verify` | Claim or verify a bound backend execution permit |
| POST | `/permits/{id}/renew` | Renew online permit |
| POST | `/permits/{id}/release` | Release online permit |
| POST | `/permits/{id}/confirm-stopped` | Confirm stopped execution |
| GET | `/workers/{id}/resources` | Read one node's resource snapshot |
| POST | `/workers/{id}/heartbeat` | Worker heartbeat |
| POST | `/workers/{id}/commands/claim` | Claim execution commands |
| POST | `/attempts/{id}/renew` | Renew execution lease |
| POST | `/attempts/{id}/report` | Report execution status |
| POST | `/permits/{id}/delegate` | Transfer asynchronous ownership |
| POST | `/delegations/verify` | Acquire linked permit |
| GET | `/statistics` | Operational statistics |
| GET | `/tasks/{id}/events` | Task status timeline |
| GET | `/management` | Read node management |
| GET | `/model-budgets` | List built-in model budgets |
| POST | `/model-budgets/calibrate` | Calibrate or directly pass a built-in model budget |
| POST | `/model-budgets/restore` | Restore the automatic built-in model budget |
| POST | `/enrollments` | Create enrollment invitation |
| POST | `/enrollments/{id}/revoke` | Revoke enrollment invitation |
| POST | `/workers/register` | Register external worker |
| POST | `/nodes/{id}/approve` | Approve node and budget |
| POST | `/nodes/{id}` | Update node budget |
| POST | `/nodes/{id}/revoke` | Revoke external node |
| POST | `/workers/{id}/executors` | Propose executors |
| POST | `/executors/{id}` | Approve or disable executor |
| POST | `/service-identities` | Create scoped service identity |
| POST | `/service-identities/{id}/revoke` | Revoke scoped service identity |
| POST | `/node-sources/modelinfer/{id}` | Save VOS App node budget plan |
| POST | `/node-sources/modelinfer/{id}/control` | Reserved VOS App remote control contract |
| POST | `/inference-waiters` | Internal bounded admission priority hints |
| POST | `/permits/{id}/reuse-resident` | Rotate a reserved built-in resident execution proof |
| POST | `/permits/inspect` | Inspect current binding and execution-node eligibility without reserving resources |
| GET | `/integrations` | Application integration status |
| POST | `/integrations` | Register application lifecycle |
| DELETE | `/integrations` | Unregister idle application lifecycle |
| POST | `/integrations/heartbeat` | Application heartbeat and stop commands |
| POST | `/permits/{id}/stop` | Request model execution stop |
| POST | `/permits/{id}/stopped` | Confirm native exit and unloading |
| POST | `/permits/{id}/park-resident` | Park an owned Ollama model after native compute is confirmed idle |
| POST | `/permits/wait` | Wait for native inference admission without starting inference |
| POST | `/integrations/executions` | start_controlled_execution |


## Weights — base http://v-scheduler:6005

| Method | Path | Operation |
| --- | --- | --- |
| GET | `/api/v1/models` | List local models; status=ready / all / pulling / failed / deleting |
| DELETE | `/api/v1/models` | Safely delete an unreferenced model; model_id query |
| POST | `/api/v1/models/pull` | Download full model, exact selected files or append files |
| POST | `/api/v1/models/update` | Update installed model; preserve old snapshot on failure |
| POST | `/api/v1/models/check-update` | Check one model version |
| POST | `/api/v1/models/check-updates` | Check all installed model versions |
| POST | `/api/v1/models/sync` | Scan shared model storage |
| POST | `/api/v1/models/cleanup` | Clean inactive download staging |
| GET | `/api/v1/models/repository-files` | List remote HF/MS repository files without downloading weights |
| GET | `/api/v1/models/files` | List actual files in the current local snapshot |
| POST | `/api/v1/models/import-archive` | Import zip/tar archive with multipart form |
| GET | `/api/v1/tasks` | Latest download task per model |
| GET | `/api/v1/tasks/detail/{id}` | Download task progress, speed, ETA and failure |
| DELETE | `/api/v1/tasks/{id}` | Delete task record without deleting model |
| POST | `/api/v1/tasks/{id}/cancel` | Cancel owned download and restore old model |
| POST | `/api/v1/tasks/{id}/retry` | Retry failed download with original file selection |
| GET | `/api/v1/aliases` | List model aliases |
| POST | `/api/v1/aliases` | Create model alias |
| DELETE | `/api/v1/aliases/{id}` | Delete model alias |
| GET | `/api/v1/containers` | List model container references |
| POST | `/api/v1/containers/bind` | Declare container model reference |
| DELETE | `/api/v1/containers/unbind` | Remove container reference |
| GET | `/api/v1/inference-services` | List external inference services; include_builtin optional |
| POST | `/api/v1/inference-services/register` | Register Ollama/OpenAI/custom management contract |
| GET | `/api/v1/inference-services/{id}` | Inspect service health, models, live metrics and slots |
| DELETE | `/api/v1/inference-services/{id}` | Delete service registration |
| POST | `/api/v1/inference-services/{id}/models/start` | Start model and confirm running state |
| POST | `/api/v1/inference-services/{id}/models/stop` | Stop model and confirm stopped state |
| GET | `/api/v1/ollama/runtime` | Inspect three builtin Ollama instances |
| POST | `/api/v1/ollama/runtime/start` | Load model in qa/embedding/rerank instance |
| POST | `/api/v1/ollama/runtime/stop` | Unload model |
| GET | `/api/v1/integration` | Configured internal endpoints and host port mappings |
| GET | `/api/v1/version` | Installed VOS app version, profile and every service image/tag |

## Model domains and per-step Auto routing

See [Model domains](model-domains.md#model-domains) for complete UI workflows, fields, permissions, responses, errors and examples. UI and API share one reusable domain catalog. Auto domains restrict specialist candidates and optionally allow General fallback. Each fresh step reranks by domain, complexity and existing capability/resource constraints. Unfinished tool turns retain their model and require the same session ID.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/v1/model-domains` | Read built-in/custom categories; returns domains array |
| POST | `/api/v1/model-domains` | VOS user creates a shared category; duplicates reuse the existing ID |
| PATCH | `/api/v1/models/domains` | Atomically replace domains of 1–200 models; administrators may include auto/auto |

Single-model changes use domains in PATCH `/api/v1/models`. Registration accepts top-level domain defaults and models[].domains overrides. GET/PUT `/api/v1/auto-routing` also configures Auto domains using the latest revision. POST `/api/v1/auto-routing/test` exposes this step’s classification and selection reasons. Omission preserves stored domains, [] resets General, and unknown IDs/null are rejected. The linked guide details custom category IDs, batch atomicity, classifier confidence fallback and tool-session boundaries.

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/api/v1/model-speed` | [Model speed trends: generation speed, first-token wait and per-call data](model-speed.md) |
