> 迁移说明：网关现由 V-Scheduler 统一发布，Rust 源码位于 `backend/src/router/`；文中 Go 测试命令仅为历史记录。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# 注册与统一路由

试聊和 `/v1/*` 模型推理接口共用调度器。上游来源可以是手动配置、Model Hub 发现或后端主动注册；来源负责提供后端/模型快照，不单独决定分流。后续发现适配器应接入同一 Registry，并使用稳定 ID，保留调度配置及正在执行的请求计数。

## API 如何选择

| 请求 | 行为 |
| --- | --- |
| `model=qwen3:0.6b`，无选择头 | 在所有来源的同名模型中指定来源，包括已配置的云端上游 |
| `model=model-hub/qwen3:0.6b` | 仅 Model Hub 发现的候选 |
| `model=manual/qwen3:0.6b` | 仅手动添加的候选，即使这些后端也运行在本机 |
| `model=com.ictrek.ai/deepseek-chat` | 仅 `ai.ictrek.com` 云端算力 API 候选；其他云端 API 使用同样的域名反转前缀 |
| `model=registered/qwen3:0.6b` | 仅主动注册的候选 |
| `X-VRouter-Upstream: registered-vllm-1` | 严格指定该实例；模型填上游原生名，允许原生接口使用未列出的模型或供应商默认模型 |

**短名不再采用 local > manual > cloud 的固定优先级。** 需要限制来源、避免调用已配置的收费上游时，使用上述前缀或指定实例。相同名称意味着调用方接受候选模型等价；网关不会判断不同量化、版本或供应商模型是否等价。

自动选择步骤：

1. 匹配模型名或带前缀的名称，排除不健康、租约过期及暂停的后端。
2. 根据模型能力、声明协议和服务角色排除已知不兼容后端。Messages 仅选择 Anthropic 或 Chat Completions 转换后端；Anthropic 原生不自动提供 OpenAI 接口。Model Hub/Ollama 优先读取 native `/api/tags` 中每个模型的 `capabilities`，同时用 generate/embedding 服务角色限定实际入口；注册后端可声明 `capabilities` 或逐模型 `model_capabilities`。云端和手动上游从模型列表读取能力；能力缺失或不是 V-Router 已知能力词时视为未知，不提前拦截所有接口，由实际上游返回具体不支持原因。
3. 标记为“直接放行”的上游跳过并发上限、熔断、模型预算与调度准入，并在存在可用候选时优先选择；该上游下的所有模型都继承这一状态。多个直接放行候选按实例 ID 确定顺序。
4. 其余候选排除已达到 `max_inflight` 者，选择 `进行中请求 / weight` 最小者。
5. 负载相同时，在当前模型名与接口范围内按权重平滑轮换，再以实例 ID 确定初始平局顺序。同权重的空闲后端轮流接收请求；指定调用和其他模型范围的历史不影响轮换。

每次选择和占用递增在同一锁内完成。普通响应、错误、客户端取消及 SSE 结束后释放占用；SSE 期间不能提前释放。指定实例的推理请求也占用相同计数。计数只覆盖当前 V-Router 进程转发的推理请求，不是后端 GPU 槽位，不包含绕过网关的调用，也不在多个网关副本间共享。

返回头：`X-VRouter-Upstream` 是实际实例 ID，`X-VRouter-Routing` 是 `least-inflight` 或 `pinned`。用量记录保存同一实例 ID。所有候选满载返回 **429** 和 `Retry-After: 1`；无健康候选返回 **503**；未知模型/指定实例返回 **404**；只有已知不兼容候选返回 **400**。当前不排队，不自动重试或切换已经发出的请求，避免重复生成与计费。

文件、批任务、历史 Responses 等有状态资源仍要求固定实例；不能轮询不同上游读取同一个资源 ID。它们不参与模型自动调度。

```python
import os
from openai import OpenAI
from anthropic import Anthropic
key = os.environ['V_ROUTER_TOKEN']
oa = OpenAI(base_url='http://<vos-host>:22888/v1', api_key=key)
# 指定来源
reply = oa.chat.completions.create(model='qwen3:0.6b', messages=[{'role':'user','content':'你好'}])
# 严格指定，沿用相同协议转换能力
reply = oa.responses.create(model='qwen3:0.6b', input='你好', store=False,
    extra_headers={'X-VRouter-Upstream':'registered-vllm-1'})
an = Anthropic(base_url='http://<vos-host>:22888', api_key=key)
reply = an.messages.create(model='registered/qwen3:0.6b', max_tokens=128,
    messages=[{'role':'user','content':'你好'}])
```


## 云端算力 API

管理页“添加云端算力 API”默认填入 `https://ai.ictrek.com/v1`，注册和购买入口指向 `https://ai.ictrek.com`。该服务按 OpenAI 兼容方式提供 `/v1/models`，需要 Bearer API Key；未带 Key 会返回认证错误。其他云端服务也可以选择 `openai`、`chat_completions`、`anthropic`、`offline_asr` 或 `image_studio` 协议。

云端模型前缀由 `base_url` 的 host 反转生成，避免不同云端同名模型冲突：

| Base URL host | 模型前缀示例 |
| --- | --- |
| `ai.ictrek.com` | `com.ictrek.ai/<model>` |
| `api.example.com` | `com.example.api/<model>` |

“厂商预配置”可选 OpenRouter、DeepSeek、阿里云百炼（国内/国际）、Z.ai、BigModel、Moonshot Kimi、MiniMax、Xiaomi MiMo、OpenAI、Anthropic、xAI 和 OpenCode Zen/Go，自动填入名称、Base URL 与协议，密钥仍需自行填写。注册模型服务对话框提供同一组预配置，但外部厂商通常应作为上游添加；注册服务需要在租约过期前发送心跳。

添加前可以测试模型列表；测试成功后对话框会列出探测到的模型并默认全选，可以用“全选/清空”调整后只添加勾选的模型。保存请求的 `models` 字段接受模型 ID 字符串或对象数组（`{"id":"...","kind":"...","capabilities":[...],"endpoints":[...]}`）；省略该字段时保持旧行为，登记全部探测结果。编辑已保存的上游时会列出已有模型；重新“刷新”只保留已登记的模型，不会静默加入新模型。填写“联通测试模型”时会再发一次轻量对话请求。测试失败不会强制阻止保存，勾选“模型列表探测失败也允许保存”后可以先记录上游，让真实业务调用返回上游的具体错误。模型列表一旦刷新成功，云端模型会同时出现在 `/v1/models` 和 `/api/v1/models`。

保存后可在“模型”页继续修改单个模型的元数据（仅外部 API 上游，owner 或管理员）：

```http
PATCH /api/v1/models
Content-Type: application/json

{"model":"openrouter/openai/gpt-5.6-sol","upstream_id":"openrouter","kind":"chat","capabilities":["completion","tools"],"endpoints":["/chat/completions"]}
```

`kind` 可省略或留空，由能力和接口推断；`capabilities`、`endpoints` 也按探测值处理。直接放行的上游下的模型会显示“直接放行（上游）”，不再单独提供预算校准。

## 配置权重和并发限制

上游页的“调度”支持全部来源，等价 API：

```http
PUT /api/v1/upstreams/registered-vllm-1/routing
Authorization: Bearer <VOS token 或网关 Key>
Content-Type: application/json

{"weight":2,"max_inflight":4,"disabled":false,"direct":false}
```

默认权重 1，范围 1..1000；默认并发上限 0 表示不限；默认不暂停。`direct=true` 表示“直接放行”：跳过 `max_inflight`、熔断、模型预算和调度准入，并在多个候选可用时优先直连该上游；该上游下的所有模型都会自动进入直接放行状态，不需要逐模型设置，`disabled`（暂停）和租约过期仍然生效。外部云端 API 通常应直接放行，本地推理后端则由调度控制并发；由 V-Scheduler 或应用托管（`source` 非 `api`）的上游只能由管理员开启直接放行，外部 API 上游由 owner 或管理员配置。PUT 替换完整策略，省略字段回到默认。策略保存在原有状态文件，升级旧文件自动采用默认值；Model Hub 刷新和同 ID 后端重新注册不清零策略或占用。`GET /api/v1/upstreams` 返回 `routing`、`inflight`、`model_ids`，已保存凭据不回显。只有 owner 和管理员能修改调度配置。

暂停和注销只阻止新模型推理请求，不中止已在执行的请求。指定实例不会绕过暂停；直接放行的指定实例也不再受并发上限限制。有状态资源读写不纳入推理并发限制。

## 连接失败熔断与恢复

上游连续 3 次传输层连接失败（拨号失败、连接拒绝、超时等；上游返回的 4xx/5xx 不算，能应答就说明存活）会自动进入冷却，退出自动调度候选：首次 30 秒，之后按指数退避，上限 5 分钟。冷却结束后的下一个请求就是恢复探测，成功即复位，失败则按退避再次冷却。该机制只影响自动选择；显式 `X-VRouter-Upstream` 指定仍然放行，可用于人工验证故障实例。客户端取消的请求不计入。熔断状态在内存中，网关重启后清零；发现刷新替换上游快照或注册服务心跳续期也会复位。

管理 API 和界面对每个上游给出 `status` 状态分档：`healthy`、`paused`（调度暂停）、`lease_expired`（租约过期）、`circuit_open`（熔断中，含连续失败次数和预计恢复时间）、`auth_failed`（模型探测 401/403）、`discovery_failed`（发现或探测失败）、`no_models`（可达但没有模型）、`full`（达到并发上限）。

## 上游所有权与可见范围

手动和云端上游在创建时记录 owner（VOS `sub` 或 `gateway-admin`），并支持 `visibility`：

| 可见范围 | 行为 |
| --- | --- |
| `public`（默认，共享） | 所有调用者可见可用；管理操作仅限 owner 和管理员 |
| `private` | 模型只对 owner 同一 VOS 身份和管理员可见、可调用 |

修改、刷新、删除和调度配置仅 owner 与管理员可操作：他人访问 private 上游返回 404，修改他人 public 上游返回 403。owner 不可转移。存量上游 owner 为空，升级后保持共享且所有登录用户可管理（行为不变，不丢配置）；如果既有的共享上游要收紧，重新保存或在升级前规划 owner。

## 供应商资源归属

经 V-Router 创建的供应商资源（文件、批任务、视频任务、向量库等固定上游资源，含 `POST /v1/videos/generations` 返回的任务 ID）会记录创建者并持久化到 App Storage。其他用户随后对这些资源 ID 的读取、取消、删除、下载返回 404，owner 和管理员不受限。两条边界：列表接口仍返回上游共享账户的全部资源（共享供应商账户的固有限制）；在本功能上线前或绕过 V-Router 创建的资源没有归属记录，保持原有共享行为。删除上游会一并清除其归属记录。

## 后端主动注册

注册接口需要现有 VOS 身份或网关 Key。注册者可以选择 `visibility`：

| 可见范围 | 行为 |
| --- | --- |
| `public`（默认） | 模型进入所有调用者的 `/v1/models` 和 `/api/v1/models`；内网匿名推理在未强制鉴权时也可调用。 |
| `private` | 模型只对注册者同一 VOS 身份和管理员可见；其他用户、匿名调用和其他应用的 token 不会在模型列表看到，也不能路由到该上游。 |

管理员可以在注册时附带 `scheduler` 策略决定该服务是否由 V-Scheduler 调度。默认不附带，服务按本机模型调度策略处理；`{"mode":"direct"}` 表示直接放行，跳过调度准入、排队和模型预算，等价于外部上游。其他模式（`cloud`、`permit`、`delegated`）用于受调度管理的本地后端。直接放行绑定会记录当时的 `base_url`，地址变化后必须重新注册；界面仅在管理员登录时显示“调度方式”。

其他 VOS 应用不需要手动配置这些注册服务：应用以自己的 OIDC/Fastpath token 调用 `/api/com.ictrek.v-scheduler/v1/models` 即可看到该用户可用的 public 模型和自己的 private 模型；需要能力筛选时调用 `/api/com.ictrek.v-scheduler/api/v1/models`，读取 `kind`、`capabilities`、`endpoints`、`upstream_kind` 和 `model_prefix`。应用侧不要保存云端 Key、上游地址或模型清单。

vLLM 原生不会自行调用本接口，需由其启动脚本、服务管理进程或配套注册程序调用。每个实例有稳定且不同的 `service_id`；重启沿用同一 ID，副本不要共用 ID。同一个 `service_id` 只能由原 owner 或管理员更新。Base URL 必须是从 V-Router 容器可达的地址，包含 `/v1`。public/private 选择、同账户所有权、sidecar 生命周期、多副本和 tc229 验收步骤见 [vLLM 主动注册指南](vllm-active-registration.md)。

模型 API 注册会探测上游 `/models`：

```bash
curl http://<vos-host>:22888/api/v1/registrations \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "service_id":"vllm-node-1",
    "visibility":"public",
    "base_url":"http://<backend>:8000/v1",
    "protocol":"chat_completions",
    "model_kind":"chat",
    "ttl_seconds":90
  }'
```

单模型注册不依赖 `/models`，但必须声明可访问接口：

```bash
curl http://<vos-host>:22888/api/v1/registrations \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "service_id":"embed-node-1",
    "visibility":"private",
    "base_url":"http://<backend>:8000/v1",
    "protocol":"openai",
    "model":"bge-m3",
    "endpoints":["embeddings"],
    "capabilities":["embedding"],
    "ttl_seconds":90
  }'
```

`protocol` 取值与手动上游一致。`offline_asr` 用于把 OpenAI 兼容 `/v1/audio/transcriptions` 的 multipart `file` 转成离线语音服务的 `POST /transcribe?lang=<language>` 原始音频调用；同账户 private 注册、心跳恢复、Compose 配置和 tc229 验收步骤见 [ASR 主动注册开发指南](asr-active-registration.md)。`image_studio` 用于把 `/v1/images/generations`、`/v1/images/edits`、`/v1/videos/generations` 转成 Image Studio 的 `/api/v1/generate` 后台任务；V-Router 自动发现同 VOS 的 Image Studio，并只暴露 `status=ready` 的模型和变体，例如 `image-studio/sd-turbo`、`image-studio/sd15@q8_0`。`endpoints` 决定 V-Router 允许哪些 `/v1/*` 接口进入该模型；常用值包括 `chat_completions`、`responses`、`messages`、`completions`、`embeddings`、`images/generations`、`images/edits`、`videos/generations`、`audio/speech`、`audio/transcriptions`、`moderations`、`rerank`。`capabilities` 和 `model_capabilities` 用于模型能力兜底：`completion`/`chat`/`tools`/`thinking` 进入对话类接口，`embedding`/`embeddings` 进入 Embeddings；`model_kind` 可为 chat、embedding、unknown，仅作为旧客户端兜底。上游需要鉴权时，可提交该上游的 `api_key`（与网关注册凭据分开）。

管理界面也支持同样两种方式：选择“模型 API”时从 `/models` 拉取列表并注册整个 API，模型默认按上游协议可用接口尝试调用；点击“测试”拿到模型列表后，可以在“从探测列表选择模型”里选一个模型，界面会切到“单个模型”，再用“模型能力预设”按这个模型的真实能力选择 Chat、Responses、Messages、Completions、Embedding、Rerank、图片生成/编辑/变体、视频生成、ASR、语音翻译、TTS、Moderation 或自定义接口。单个模型不要声明它实际不支持的接口。

模型 API 模式下也可用 `model_endpoints` 和 `model_capabilities` 逐模型覆盖：

```json
{
  "service_id": "mixed-api",
  "base_url": "http://backend:8000/v1",
  "model_endpoints": {
    "qwen3:8b": ["chat_completions", "responses"],
    "bge-m3": ["embeddings"]
  },
  "model_capabilities": {
    "qwen3:8b": ["completion", "tools"],
    "bge-m3": ["embedding"]
  }
}
```

租约默认 90 秒，允许 30..3600 秒。心跳返回 404 时应重新注册；模型目录、地址、协议、可见范围或上游密钥变化时也应重新注册。心跳是后端的存活声明，不代替推理健康检查；发送方须先确认服务可用。注册快照和租约不落盘，网关重启后后端重新登记；调度策略按稳定实例 ID 恢复。租约过期立即退出模型列表和新请求选择，已开始的请求继续完成。

```bash
curl -X POST http://<vos-host>:22888/api/v1/registrations/registered-vllm-node-1/heartbeat \
  -H "Authorization: Bearer $VOS_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"ttl_seconds":90}'
```

未来新增发现机制时，应映射为稳定来源/实例、协议、模型和健康快照，使用同一个调度器。当前未增加局域网扫描或新的发现协议。
