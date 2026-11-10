# API reference / API 完整参考

This index is generated from the shipped OpenAPI documents. Each operation links to the complete request, response and authentication contract. In the app, open **API Documentation → Model & Gateway API / Scheduler Management API / Model Management API** and search by path. The Application Integration tab explains the end-to-end workflow.

本索引来自随镜像交付的 OpenAPI。每个接口链接到完整的请求、响应及鉴权契约。应用内进入 **API 文档 → 模型与网关 API / 调度管理 API / 模型管理 API**，可按路径搜索；「应用接入」提供完整流程。

- [Authentication, visibility and keys / 身份、可见范围与 Key](#identity)
- [Registration and direct admission / 注册与直接放行](#registration)
- [Gateway API / 网关接口](#gateway)
- [Controller API / 调度接口](#controller)
- [Weights API / 权重管理接口](#weights)
- [Errors / 错误处理](#errors)

<a id="identity"></a>
## Authentication and permissions / 身份与权限

Use `http://v-scheduler:8080` inside VOS or `http://<vos-host>:22888` externally (actual host port is configurable). OpenAI uses `Authorization: Bearer $KEY`; Anthropic accepts `x-api-key: $KEY`. `$KEY` may be an authorized VOS user token, personal inference key or model-scoped key. Public models accept no key or any key; invalid keys grant public-only access. Private models require an authorized identity. Provider `api_key` is never the caller's V-Scheduler access key.

VOS 内使用 `http://v-scheduler:8080`，外部使用 `http://<vos-host>:22888`（以实际安装映射为准）。OpenAI 使用 Bearer，Anthropic 支持 x-api-key。可使用 VOS token、个人推理 Key 或模型 Key；公开模型可不带或任意填写 Key，无效 Key 只获得匿名公开权限。上游厂商 api_key 仅供访问上游。

Inference keys grant model discovery and inference only. Creating/revoking keys, managing upstreams and changing model settings require the appropriate VOS owner/admin identity. Scheduler policies, model-level direct routing and Auto configuration require administrator authority. Controller app/worker tokens have their own scopes. Port 6005 weight management does not add independent API authentication: restrict it to a trusted management network; a gateway inference key does not protect this port.

推理 Key 只能发现和调用模型，不能管理配置或签发 Key。模型与上游修改需 VOS 所有者或管理员；注册调度策略、模型级放行和 Auto 配置需管理员。调度器应用/worker 凭据单独限权。6005 权重管理端口无独立 API 鉴权，须限制在可信网络，不能用推理 Key 保护它。

<a id="registration"></a>
## Registration modes / 注册调度模式

Registration is model discovery, not resource approval. POST `/api/v1/registrations` accepts an administrator-only `scheduler` policy:

| mode | Behavior / 行为 |
| --- | --- |
| omitted / 省略 | Managed resource admission; requires a valid approved resource binding / 受管准入，须有获批资源绑定 |
| `direct` | Same setting as upstream routing.direct: bypass permits, budgets, routing concurrency and breaker checks / 与上游 routing.direct 同一设置，跳过许可、预算、路由并发及熔断 |
| `cloud` | Declare remote cloud compute; no local resource reservation / 声明远程云计算，不占本地预约 |
| `permit` | Approved executor_id + completion=synchronous and matching model budget / 已批准执行器、同步完成与模型预算 |
| `delegated` | Application owns asynchronous completion and confirmed stopping / 应用负责异步完成与确认停止 |

```sh
GATEWAY=http://v-scheduler:8080
curl -fsS "$GATEWAY/api/v1/registrations" -H "Authorization: Bearer $VOS_TOKEN" -H 'Content-Type: application/json' -d '{"service_id":"my-infer","base_url":"http://my-infer:8000/v1","protocol":"chat_completions","visibility":"private","models":[{"id":"chat-a","capabilities":["chat","completion"],"endpoints":["chat_completions"]}],"scheduler": {"mode": "direct"},"heartbeat_timeout_seconds":90}'
```

Save the same full payload locally as `registration.json`; POST it to `/api/v1/registrations/registered-my-infer/heartbeat` at the returned `heartbeat_after_seconds`. Existing entries only renew health/timeout; a full snapshot restores a missing service after restart. Re-register the same service_id to change metadata, visibility, scheduling or registration keys. `healthy=false` hides a service without releasing active resource reservations. Timeout defaults to 90 seconds, allowed 30–3600; `ttl_seconds` is a compatibility alias.

将完整请求保存为 registration.json，按响应 heartbeat_after_seconds 向心跳接口发送。普通心跳仅续期，完整快照可恢复丢失的注册；修改元数据、权限、调度策略或注册 Key 请重新 POST 相同 service_id。healthy=false 隐藏服务，不释放正在执行的预约。默认超时 90 秒，范围 30–3600 秒。

`PUT /api/v1/upstreams/{id}/routing` is a full routing replacement: `{ "weight":1, "max_inflight":0, "disabled":false, "direct":true }`. Registration `scheduler.mode=direct` is normalized into this same `routing.direct` flag; it is an input alias, not a second independent setting. Both bypass resource admission, budgets, routing concurrency and breaker checks. Pause, ownership and protocol capability checks still apply. Normal heartbeats do not overwrite an operator change; explicit re-registration with a scheduler policy is a new configuration change. `PUT /api/v1/model-routing` takes `{ "model_id":"registered-my-infer/chat-a", "direct":true }`; here model_id is **internal upstream_id/native model ID**, not the public catalog ID. Setting false removes only the model override; an upstream-wide direct setting still applies. Inference uses the full ID from `/v1/models` and accepts older saved IDs for compatibility.

上游路由 PUT 为完整替换，其 direct 还跳过路由并发和熔断。模型级接口使用内部 upstream_id/原生模型 ID，false 只删除模型覆盖，不会取消上游放行。正常调用仍使用模型目录返回的完整 id。

Managed services enroll/register a node, report heartbeat, request executor resources, obtain node/executor approval, then receive a per-model binding. Query `/workers/{id}/resources` for approved capacity, reservations, resident memory, remaining budget and telemetry; this is not a permit guarantee. Submit tasks or obtain permits before computation, renew while running, and release only after confirmed stop. See [unified integration](application-integration.md) for full JSON examples, priority/queue controls and cancellation.

受管服务完成节点登记、心跳、执行器预算申请与审批，再取得模型绑定。资源查询区分批准容量、预约、驻留、剩余预算及遥测，不保证取得许可。计算前申请任务/许可，运行期间续期，确认停止后释放。完整节点 JSON、优先级排队和停止见统一接入指南。

<a id="gateway"></a>
## Gateway / 模型与网关

Base: `http://v-scheduler:8080`. [Complete OpenAPI](../api/router-openapi-en.json)

| Method | Path | Purpose / 用途 |
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

<a id="controller"></a>
## Controller / 调度管理

Base: `http://v-scheduler:38120/api/v1`. [Complete OpenAPI](../api/openapi-v1.json)

| Method | Path | Purpose / 用途 |
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
| POST | `/integrations/executions` | 启动已注册应用计算 |

<a id="weights"></a>
## Weights / 权重管理

Base: `http://v-scheduler:6005`. [Complete OpenAPI](../api/model-management.openapi.json)

| Method | Path | Purpose / 用途 |
| --- | --- | --- |
| GET | `/api/v1/models` | List local models; status=ready/all/pulling/failed/deleting |
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

<a id="errors"></a>
## Errors / 错误与恢复

| Status/code | Action / 处理 |
| --- | --- |
| 401 / 403 | Check VOS identity, owner/admin role and key scope / 检查身份、所有者、管理员与 Key 范围 |
| 409 | Refresh state/revision or confirm stop before retry / 刷新配置版本或等待确认停止 |
| 429 | Honor Retry-After; request a bounded queue wait / 按提示重试或使用有界队列等待 |
| SCHEDULER_BINDING_MISSING | Approve/bind node, executor and model resources / 完成节点、执行器和模型预算绑定 |
| SCHEDULER_BINDING_INVALID | Inspect integration status and rebind / 检查接入状态并重建有效绑定 |
| MODEL_RESOURCE_POLICY_NOT_APPROVED | Approve this model's budget and refresh binding / 批准该模型预算并刷新绑定 |
| SCHEDULER_ADMISSION_TIMEOUT | Inspect controller/database health; no upstream computation started / 检查控制器和数据库状态，请求尚未送上游 |
| Upstream 400 context length | Reduce input/output or adjust upstream context; direct admission cannot increase it / 缩短上下文或修改上游限制，直接放行不会扩大上下文 |

All business codes and envelopes: [error contract](../api/error-contract.json). Preserve `request_id` when collecting diagnostics; do not log tokens or secrets. Network restrictions on some mainland-China test hosts may prevent access to overseas providers; verify connectivity from the host running V-Scheduler, not only from your browser.

全部业务码及错误包结构见错误契约。排查保留 request_id，不记录 token/Key。部分中国大陆测试机可能因网络限制无法连接境外 API，应从实际运行 V-Scheduler 的主机检查连通性。

## Model key response behavior / 模型 Key 返回值

access_key=true 自动生成时，仅本次响应的 model_access_keys[].api_key 返回完整 Key；请立即保存。字符串表示设置调用方已有的自定义 Key，成功响应只返回模型与 Key 元数据，不回显 api_key。null 表示撤销，返回 key:null。省略 access_key 的注册请求保留已有 Key，不返回原 Key。

Only access_key=true returns the generated secret in model_access_keys[].api_key, once in that response; save it immediately. A string sets a caller-supplied key and returns model/key metadata without an api_key echo. null revokes and returns key:null. Registration with access_key omitted preserves the existing key and never returns its secret.

GET /api/v1/api-keys 只能查看本人 Key 的 id、name、prefix、models、active 和时间等元数据，不返回明文或 hash。遗失自动生成的 Key 后无法找回，请重新生成或设置替换；该模型旧 Key 随即失效，不影响其他模型及个人 Key。操作需要所有者或管理员的 VOS token，不能使用推理 Key。响应 model 使用内部 upstream_id/native_model_id，与请求可使用的公开完整模型 id 含义不同。

GET /api/v1/api-keys lists only the current owner’s metadata such as id, name, prefix, models, active and timestamps, never secrets or hashes. A lost generated key cannot be recovered: generate or set a replacement. The old key stops authorizing this model; other models and personal keys are unaffected. Use the owner/admin VOS token, not an inference key. Response model is internal upstream_id/native_model_id; the request may use the public qualified model id.

```text
# PUT /api/v1/models/access-key
# Authorization: Bearer $VOS_TOKEN (owner or administrator)
# Generate a new model access key:
{
  "upstream_id": "deepseek",
  "model": "api.deepseek.com/deepseek-flash",
  "access_key": true
}
# HTTP 200 — example placeholders, not usable secrets:
{
  "model_access_keys": [
    {
      "model": "deepseek/deepseek-flash",
      "key": {
        "id": "<key-id>",
        "name": "Model: deepseek/deepseek-flash",
        "prefix": "vs-key-<prefix>",
        "models": [
          "deepseek/deepseek-flash"
        ],
        "created_at": "<UTC timestamp>",
        "expires_at": null,
        "revoked_at": null,
        "active": true
      },
      "api_key": "vs-key-<complete-generated-key-returned-once>"
    }
  ]
}
# Set your own key (16..512 printable non-space ASCII characters):
{
  "upstream_id": "deepseek",
  "model": "api.deepseek.com/deepseek-flash",
  "access_key": "<your-own-strong-model-key>"
}
# HTTP 200 — metadata only; no api_key field:
{
  "model_access_keys": [
    {
      "model": "deepseek/deepseek-flash",
      "key": {
        "id": "<key-id>",
        "name": "Model: deepseek/deepseek-flash",
        "prefix": "custom-<hash-prefix>",
        "models": [
          "deepseek/deepseek-flash"
        ],
        "created_at": "<UTC timestamp>",
        "expires_at": null,
        "revoked_at": null,
        "active": true
      }
    }
  ]
}
# Revoke:
{
  "upstream_id": "deepseek",
  "model": "api.deepseek.com/deepseek-flash",
  "access_key": null
}
# HTTP 200:
{
  "model_access_keys": [
    {
      "model": "deepseek/deepseek-flash",
      "key": null
    }
  ]
}
# Query owner metadata (never retrieves complete keys):
curl -fsS "$GATEWAY/api/v1/api-keys" -H "Authorization: Bearer $VOS_TOKEN"
# Model registration uses the same response entries for access_key/model_access_keys.
```

普通心跳和重启后的完整快照恢复不会覆盖已保存的放行设置，也不会重新生成或撤销模型 Key。需要修改设置或轮换 Key 时，明确调用配置 API；恢复响应不返回原 Key。

Ordinary heartbeats and full-snapshot recovery after restart preserve saved direct settings and model keys; they do not regenerate or revoke keys. Use an explicit configuration/key API to change settings or rotate keys. Recovery never returns the existing secret.

## Response contracts / 响应字段与状态

Gateway-owned management operations use named response schemas in [English OpenAPI](../api/router-openapi-en.json) and [中文 OpenAPI](../api/router-openapi-zh-CN.json). The in-app **Model & gateway API** operations expand request parameters, nested fields, array items, nullable/alternative branches and every response status. **Scheduler API** also renders its operation contracts. API fields remain their original JSON spelling in both languages.

网关管理接口不是统一透传正文：`GET /api/v1/models` 返回 `{models:[...]}`，上游列表返回 `{upstreams:[...]}`，修改模型元数据返回 `{model:{...}}`，上游新增成功返回 **201 + 上游对象**；删除上游/注册返回 **204，无正文**。刷新或修改上游返回上游对象，不额外套 `data`。创建个人 Key 返回 `{api_key,key}`，查询与撤销只返回元数据；模型 Key 的一次性返回见前节。注册心跳既可能返回 online、removed，也可能在恢复时返回完整注册结果；客户端应按 `status` 和 `registration_required` 分支处理。

These schemas describe successful gateway-generated JSON, not a new envelope around inference or upstream resources. OpenAI/Anthropic streaming, audio/files, asynchronous native resources and errors keep their documented protocols. Inspect HTTP status and Content-Type before choosing JSON, SSE or binary handling. A 200 response to a dry-run Auto test can have an empty `resolved_model` and `reason=no_eligible_candidate`; inspect the decision rather than treating transport success as an executable selection.

以上 Schema 是网关自己的成功 JSON 契约，不会改变推理协议、原生资源接口或错误信封。Auto 测试成功返回 HTTP 200 时，仍可能 `resolved_model` 为空且 `reason=no_eligible_candidate`，需要读取分类和候选诊断，不能据 HTTP 成功推断已可执行。队列取消的 `execution_started` 可能为 null，表示取消先于请求入队被记录，不代表已收到原生停止证明。

`python3 scripts/check_api_documentation.py` checks route coverage, bilingual request/response/component parity, and success schemas for every management operation, including the distinct 201/204 cases. Runtime acceptance must additionally validate actual payloads against the schemas; route counts alone do not establish contract correctness.

## Protocol payloads and native resources / 推理协议与原生资源

The in-app operation details now include path/query/header parameters, required flags, nested request fields, every declared response status/media type and shared schemas. OpenAI Chat Completions/Completions/Responses, Anthropic Messages, embeddings, reranking, Decisions/System One, images and audio document their common request/response fields. Expand SSE responses for framing and terminal events; an HTTP 200 stream can still fail, and usage-only chunks can have no text. Tool deltas, reasoning deltas and ordinary text are distinct. Context size, supported tools/media, output format and usage details remain constrained by the actual model/upstream; bypassing admission does not change these limits.

应用内展开接口可查看路径/查询/Header、必填标记、嵌套请求、各状态和媒体类型及共享结构。OpenAI、Anthropic、向量、重排、Decisions/System One、图像与音频均说明公共请求/返回字段。SSE 需检查终态，HTTP200后仍可报错；纯 usage 片段可能没有正文，工具参数增量、思考增量、普通文字应分别处理。直接放行不改变模型上下文、工具解析或媒体支持。

Additional `/v1/*` resource routes are transparent native forwarding. Their availability, methods, request/response fields and success codes are owned by the selected upstream/version; a routed path does not promise that upstream supports all methods. Their contract uses `default` responses to preserve native JSON, files and SSE instead of falsely promising HTTP200 or a Scheduler wrapper. Read the native provider/resource OpenAPI for those extensions. For managed API operations the response data is described specifically; a reserved node-control operation returns 501 and is not documented as successful execution.

额外 `/v1/*` 资源是透明原生转发，是否可用、方法、字段和成功码由所选上游及版本决定，不因网关有路由就保证全部方法可用。此类契约使用 default 保留原生JSON/文件/SSE，不虚构HTTP200或统一调度包络，扩展字段应查所属协议。自有管理接口则提供具体数据结构；预留节点控制返回501，不能当作已执行。

## 模型适用领域与 Auto 每步选型

模型注册、编辑和批量设置领域的完整字段、权限、返回值和错误处理，见[模型适用领域](model-domains.md)。界面和 API 使用同一个共享分类目录，Auto 配置中的 domains 允许限定专业池及通用回退；每次新步骤按领域、复杂度及现有能力/资源条件重新选型，未完成工具续轮保留模型并要求同一会话 ID。

| 方法 | 路径 | 功能 |
| --- | --- | --- |
| GET | `/api/v1/model-domains` | 读取内置/自定义共享分类；返回 domains 数组 |
| POST | `/api/v1/model-domains` | VOS 用户新增共享分类；同名返回已有 ID |
| PATCH | `/api/v1/models/domains` | 原子替换 1–200 个模型的领域；支持管理员设置 auto/auto |

单个设置沿用 PATCH `/api/v1/models` 的 domains 字段；注册使用顶层 domains 默认值和 models[].domains 覆盖。Auto 也可通过 GET/PUT `/api/v1/auto-routing` 设置 domains（先读取最新 revision），POST `/api/v1/auto-routing/test` 查看本步骤分类及选择原因。省略 domains 保留已有值，[] 重置通用，未知 ID/null 拒绝。详见链接中的中英文示例及限制。
