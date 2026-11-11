### 模型速度趋势：生成速度和首 Token 等待

打开「模型与上游」，展开来源，在模型所在行点「速度趋势」。横轴是每次调用的完成时间，两个纵轴独立显示生成速度（token/s）和首 Token 等待（ms）。悬停、用键盘聚焦或轻触图上的点，即可看到数值、单位和完成时间，展开「逐次调用数据」看明细；关闭再打开会重新读取。Auto 的明细还显示实际模型及上游，样本也计入对应实际模型的趋势。

- 首 Token 等待：从网关开始向上游发出请求到收到首个带输出的流式片段。输出包括文本、推理内容和工具调用输出；角色声明、空片段、心跳、用量以及结束标志不算输出。
- 生成速度：`(上游报告的输出 token 数 − 1) ÷ (末个输出片段时间 − 首个输出片段时间，单位秒)`。从分子扣掉首个 token，不计入首 Token 等待。片段可能包含多个 token，因此这是网关观测的近似值，受网络和缓冲影响，并非模型内部解码计时。两个指标均排除 Scheduler 的排队和许可释放时间。
- 正常结束且观察到输出的流式调用才进入图表。缺少输出 token 数、输出不超过 1 token、或全部输出在同一批次到达时，生成速度留空，但仍可记录首 Token 等待。OpenAI 兼容上游支持时，调用端可传 `stream_options: {"include_usage":true}`；内置 Ollama 的流式 Chat Completions 试聊会请求该字段。
- 非流式响应、失败或中断的流，以及没有独立计时的旧记录不补成 0；历史用量仍照常保留。每个模型与上游组合最多返回最近 200 次可测量调用，从旧到新排列。已写入原用量日志的样本在重启后保留。

### 读取趋势 API

`GET /api/v1/model-speed?upstream_id=UPSTREAM_ID&model=MODEL_ID` 无请求体。两个查询参数均为非空字符串，需 URL 编码，无默认值；`upstream_id` 取自 `GET /api/v1/models` 的 `upstream_id`，`model` 可用 `id` 或 `qualified`。Auto 用 `upstream_id=auto&model=auto`。不支持分页、排序参数或额外筛选；重复参数取首值。VOS 应用内同源前缀为 `/api/com.ictrek.v-scheduler`，容器直连为 `http://v-scheduler:8080`，宿主机直连默认 `http://VOS_HOST:22888`。使用 VOS 应用登录得到的访问令牌（`Authorization: Bearer`），或启用校验后配置的网关管理 Key（Bearer 或 `X-API-Key`）；个人、模型及应用推理 Key 无权读取。只返回当前身份可见模型的数据；Auto 隐藏不可见的实际目标，不返回调用者、请求或响应正文。

```sh
curl -fsS -G "$GATEWAY/api/v1/model-speed" \
  -H "Authorization: Bearer $VOS_TOKEN" \
  --data-urlencode 'upstream_id=my-upstream' --data-urlencode 'model=my-model'
```

200 返回 `application/json`，`Cache-Control: private, no-store`。响应必有 `limit`（整数 200）和 `points`（数组，可为空）。每个样本必含下表字段：

| 字段 | 类型 / 单位 | 含义 |
| --- | --- | --- |
| time | RFC3339 UTC 字符串 | 网关确认流正常结束的时间 |
| upstream_id | 字符串 | 实际上游 ID |
| model | 字符串 | 实际上游的原生模型 ID |
| output_tokens | 正整数或 null | 上游报告的输出 token；没有有效数据时为 null |
| generation_ms | 正数或 null，毫秒 | 末个与首个输出片段的时间差；同一批次无法测量时为 null |
| tokens_per_second | 正数或 null，token/s | 按上述公式计算；缺计数、计数小于 2 或无法测量生成时段时为 null |
| ttft_ms | 非负数，毫秒 | 首个输出片段等待时间 |

成功示例：`{"limit":200,"points":[{"time":"2026-11-11T08:00:00Z","upstream_id":"my-upstream","model":"my-model","output_tokens":41,"generation_ms":800,"tokens_per_second":50,"ttft_ms":320}]}`。没有样本时仍为 200：`{"limit":200,"points":[]}`。

错误为 JSON：顶层 `code`（稳定错误码）、`category="v-scheduler"`、`version="1"`、`data={}`、`msg`（诊断文字）、`request_id`（请求标识，与响应头 `X-Request-ID` 相同）；`error` 含 `code`、`message`（同 msg）、`type`（400 为 invalid_request_error、401 为 authentication_error、403 为 permission_error、其他为 api_error）。401 另带 `WWW-Authenticate: Bearer`。 400 `INVALID_REQUEST` 补齐查询参数；401 `UNAUTHENTICATED` 重新登录或核对网关 Key；403 `FORBIDDEN` 换用管理身份；404 `NOT_FOUND` 核对模型/上游及可见性；405 `METHOD_NOT_ALLOWED` 改用 GET；503 `TEMPORARILY_UNAVAILABLE`（身份服务暂不可用）待身份服务恢复后重试。应用内可在「API 文档 → 模型与网关 API → 模型速度趋势」展开完整契约。既有 `GET /api/v1/usage` 的 recent 条目可附加 `ttft_ms`、`generation_ms`、`speed_completed_at`，不可测量时为 null，升级前历史记录可省略。

---

### Model speed trends: generation speed and time to first token

Open Models & Upstreams, expand a source and choose Speed trends on a model row. Both charts use request completion time on the horizontal axis. Their vertical axes show generation speed (tokens/s) and time to first token (ms) separately. Hover over, focus, or tap a point to see its value, unit and completion time. Expand Per-call data for the table. Reopen to refresh. The Auto view lists the resolved model and upstream; the same point also appears under the resolved model.

- Time to first token: from gateway dispatch to the first streamed output fragment. Text, reasoning and tool output count; role declarations, empty fragments, heartbeats, usage and termination markers do not.
- Generation speed: `(upstream-reported output tokens − 1) ÷ (last output fragment time − first output fragment time, in seconds)`. It excludes the first token and the wait for it. This is a gateway-observed estimate, not engine decode timing; network buffering and batching can affect it. Both metrics exclude Scheduler queueing and permit release.
- Only successfully completed streams with observed output appear. If usage is missing, the output count is at most 1, or all output arrives in one batch, the TPS point is absent while first-token wait can still be shown. For compatible OpenAI-style upstreams, callers can request `stream_options: {"include_usage":true}`. Streaming Chat Completions in the built-in Ollama Playground requests it.
- Non-streaming responses, failed/cancelled streams, and older records without the required timestamps are not plotted as zero. Existing usage history is preserved. The latest 200 measurable calls per model/upstream are returned from oldest to newest. Samples written to the existing usage log survive restart.

### Trend API

`GET /api/v1/model-speed?upstream_id=UPSTREAM_ID&model=MODEL_ID` takes no request body. Both query parameters are required nonempty URL-encoded strings without defaults. Use `upstream_id` from `GET /api/v1/models` and its `id` or `qualified` as `model`. For Auto, use `upstream_id=auto&model=auto`. Duplicate parameters use the first value. No pagination, custom sorting, or additional filters. Use the VOS same-origin prefix `/api/com.ictrek.v-scheduler`, internal base `http://v-scheduler:8080`, or host base `http://VOS_HOST:22888` by default. Authenticate using a VOS app login token (Authorization: Bearer) or the configured management gateway Key when key checking is enabled (Bearer or X-API-Key). Personal, model and application inference keys cannot read this endpoint. Results are scoped to visible models; Auto omits invisible resolved targets. No caller identity or request/response content is returned.

```sh
curl -fsS -G "$GATEWAY/api/v1/model-speed" \
  -H "Authorization: Bearer $VOS_TOKEN" \
  --data-urlencode 'upstream_id=my-upstream' --data-urlencode 'model=my-model'
```

200 returns `application/json` and `Cache-Control: private, no-store`. The response always has `limit` (integer 200) and `points` (possibly empty array). Each point has all these fields:

| Field | Type / unit | Meaning |
| --- | --- | --- |
| time | RFC3339 UTC string | Time at which the gateway recognized successful stream termination |
| upstream_id | string | Actual upstream ID |
| model | string | Actual native model ID |
| output_tokens | positive integer or null | Upstream-reported output tokens; null when unavailable |
| generation_ms | positive number or null, ms | Last-to-first output fragment interval; null when output arrives in one batch |
| tokens_per_second | positive number or null, tokens/s | Estimated generation rate; null if usage is missing, count is below 2, or the interval is unmeasurable |
| ttft_ms | nonnegative number, ms | Wait for the first output fragment |

Success: `{"limit":200,"points":[{"time":"2026-11-11T08:00:00Z","upstream_id":"my-upstream","model":"my-model","output_tokens":41,"generation_ms":800,"tokens_per_second":50,"ttft_ms":320}]}`. No samples is still 200: `{"limit":200,"points":[]}`.

Errors are JSON with top-level `code` (stable code), `category="v-scheduler"`, `version="1"`, `data={}`, `msg` (diagnostic text), and `request_id` (also sent in `X-Request-ID`). The nested `error` has `code`, `message` (same as msg), and `type`: invalid_request_error (400), authentication_error (401), permission_error (403), or api_error (other statuses). HTTP 401 also sets `WWW-Authenticate: Bearer`.  400 `INVALID_REQUEST` (supply both query parameters), 401 `UNAUTHENTICATED` (sign in or check the gateway Key), 403 `FORBIDDEN` (use a management identity), 404 `NOT_FOUND` (check the model, upstream and visibility), 405 `METHOD_NOT_ALLOWED` (use GET), 503 `TEMPORARILY_UNAVAILABLE` (identity service unavailable; retry after it recovers). Expand Model speed trends under API Documentation → Model and gateway API for the contract. The existing `GET /api/v1/usage` recent records can also carry `ttft_ms`, `generation_ms` and `speed_completed_at`, null when unmeasurable and optionally absent from old records.
