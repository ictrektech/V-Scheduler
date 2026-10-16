> 迁移说明：网关现由 V-Scheduler 统一发布，Rust 源码位于 `backend/src/router/`；文中 Go 测试命令仅为历史记录。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# V-Router 当前设计与后续计划

更新日期：2026-09-18
状态：以当前 Go 实现为准；运行时端点以 `/api/v1/openapi.json` 为准

## 1. 产品定位

V-Router 是 VOS 内统一的 AI 模型入口。它聚合 Model Hub、本地或宿主机推理服务、Image Studio，以及用户手动配置的外部 API，向 VOS 应用和局域网客户端提供统一接口。

V-Router 负责：

- 发现、注册和展示模型；
- 根据模型、协议、能力和调度状态选择上游；
- 统一 OpenAI 兼容入口，并提供 Anthropic Messages 入口；
- 在边界明确时执行协议转换或异步任务适配；
- 验证 VOS 用户身份，控制 private 模型可见性；
- 记录请求、延迟、错误和上游实际报告的 token 用量；
- 提供上游、模型、调度和用量管理界面。

V-Router 不负责：

- 下载模型、管理显存或启动 Ollama/vLLM；这些属于 Model Hub 或对应推理应用；
- 推测模型能力，或把上游不存在的接口伪装成可用；
- 本地估算供应商未报告的 token；
- 自动重试已经发出的生成请求；
- 作为任意 URL、任意脚本或任意供应商接口的通用反向代理。

## 2. 实现原则

1. **Go 单进程**：后端、协议转换、注册表、调度和用量记录都在 Go 网关内完成，不引入额外代理运行时。
2. **标准接口优先**：上游优先接入 OpenAI、Anthropic 或已有明确契约；特殊供应商使用配置化、边界明确的适配器。
3. **上游报告为准**：模型、能力、token 和错误尽量保留上游事实；未知即未知，不猜测。
4. **一次生成只发一个上游**：请求发送后不自动切换上游，避免重复执行、重复扣费和工具副作用。
5. **身份贯穿调用链**：VOS OIDC token 用于用户识别、private 模型可见性和用量归属，但不会转发给模型供应商。
6. **本地状态可恢复**：持久配置和用量写入 VOS App Storage；主动注册使用租约，网关重启后由服务重新注册。

## 3. 当前架构

```text
VOS 应用 / 局域网客户端
  │ OpenAI SDK、Anthropic SDK 或 HTTP
  ▼
V-Router Go 网关
  ├─ 身份校验：VOS OIDC / 共享网关 Key
  ├─ Registry：模型、上游、来源、能力、可见范围
  ├─ Router：模型解析、协议筛选、权重、并发限制
  ├─ Adapters：协议转换、离线 ASR、Image Studio、异步任务
  ├─ Proxy：JSON、multipart、SSE、二进制、WebSocket
  └─ Usage：按用户追加 JSONL 并聚合查询
        │
        ├─ Model Hub 发现的 Ollama/vLLM 服务
        ├─ 主动注册的 ASR、vLLM 或其他模型服务
        ├─ Image Studio 自动发现
        └─ 手动配置的本地或外部 API
```

交付形态由两个容器组成：

- `v-router-backend`：Go 网关，默认容器端口 `8080`；
- `v-router-frontend`：静态管理页面，经 VOS/Traefik 暴露。

两者均加入外部网络 `vos_default`。VOS 内其他容器使用 `http://v-router:8080`，浏览器同域应用使用 `/api/com.ictrek.v-scheduler`，局域网客户端使用宿主机映射端口，默认 `22888`。

## 4. 模型来源与注册表

所有来源最终转换为相同的上游和模型记录，再进入同一个路由器。

### 4.1 Model Hub

V-Router 轮询 Model Hub 的 inference-services 列表，读取服务角色、模型列表、运行状态和 `gateway_url`。Model Hub 仍负责模型下载、启动、槽位和推理服务生命周期；V-Router 不绕过其 gateway 直接访问底层 Ollama 端口。

发现入口可以是容器网络 `http://model-hub-backend:5005` 或宿主机映射 `http://<vos-host>:35005`。发现入口只用于读取服务列表，推理仍使用 Model Hub 返回且从 V-Router 可达的 `gateway_url`。

### 4.2 主动注册

不由 Model Hub 管理的服务使用 `/api/v1/registrations` 注册、心跳和注销。稳定 `service_id` 生成 `registered-<service_id>`；每个真实副本使用不同 ID，同一副本重启沿用原 ID。注册租约不落盘，过期或网关重启后服务必须重新注册。

注册支持：

- `public`：所有有权访问 V-Router 的调用者可见；
- `private`：仅注册者同一 OIDC `sub` 和管理员可见。

具体接入见 [ASR 主动注册开发指南](asr-active-registration.md) 和 [vLLM 主动注册指南](vllm-active-registration.md)。

### 4.3 Image Studio

V-Router 从 `V_ROUTER_IMAGE_STUDIO_URL` 读取 Image Studio 模型和路由元数据，只暴露已就绪模型及变体。调用方继续使用标准图片/视频生成接口，V-Router 适配 Image Studio 后台任务。

### 4.4 手动上游

管理界面和 `/api/v1/upstreams` 支持配置本地或外部 API。上游密钥保存于 VOS App Storage，管理 API 不回显明文。支持模型列表探测、单模型声明、逐模型 endpoint/capability 覆盖，以及权重、并发上限和暂停状态。

通用异步图片上游使用“创建任务 + 查询任务”的字段映射配置，只覆盖 `images/generations`。它不是任意请求脚本，不执行用户提供的代码，也不会在“检查配置”时创建付费任务。

## 5. 模型命名与调度

聚合模型名始终带来源前缀，例如：

```text
model-hub/qwen3:8b
registered/qwen3-8b
image-studio/sd-turbo
ai.kie.api/nano-banana-2
```

路由顺序：

1. 解析模型名或 `X-VRouter-Upstream`；
2. 按调用接口、上游协议、模型能力和用户可见范围过滤；
3. 排除暂停、过期、不可用或已达 `max_inflight` 的候选；
4. 根据“进行中请求 / 权重”选择实例；
5. 固定该实例完成本次请求。

固定上游时请求使用上游原生模型名，且不会绕过暂停、并发限制、身份或 private 可见性检查。网关不在失败后把生成请求转发到另一个上游；安全的故障处理只发生在请求发送前。

## 6. 协议与适配边界

### 6.1 OpenAI 与资源接口

统一入口覆盖 Chat Completions、Completions、Responses、Embeddings、Images、Audio、Videos、Moderations 和 Rerank。Files、Uploads、Batches、Vector Stores、Assistants、Threads、Conversations、Realtime、Containers 等资源接口要求固定上游。

模型请求必须带 `model`。资源创建、查询、取消和下载必须持续携带相同的 `X-VRouter-Upstream`，V-Router 不根据资源 ID 猜测供应商。

### 6.2 Anthropic

`protocol=anthropic` 的上游原生接收 Messages 及已登记的 Anthropic 资源请求。Anthropic SDK 使用的 `x-api-key` 可以承载 VOS token 或网关 Key；供应商密钥由 V-Router 保存并在上游请求中设置。

### 6.3 Go 进程内转换

`protocol=chat_completions` 时，V-Router 可以把已定义范围内的 Responses 或 Anthropic Messages 请求转换到 Chat Completions。当前支持文本、图片输入、工具调用/结果、结构化输出和 SSE 的已测试子集。

转换是无状态的，不实现服务端 conversation、background、`store=true`、托管工具、Anthropic thinking/缓存控制或上游不存在的资源生命周期。不可映射的字段返回明确错误，不静默丢弃。详细契约见 [api-protocols.md](api-protocols.md)，性能基线见 [protocol-performance.md](protocol-performance.md)。

### 6.4 专用适配器

- `offline_asr`：把 OpenAI multipart ASR 请求转换为上游 `POST /transcribe` 原始音频调用；
- `image_studio`：把标准图片/视频请求转换为 Image Studio 任务；
- `async_task`：按受限字段映射创建并轮询图片任务。

只有多个上游能共享稳定契约时才增加通用适配器。单一供应商的临时字段优先通过现有配置映射解决，不为每个 API 网站复制一套硬编码逻辑。

## 7. 身份、权限与数据

### 7.1 鉴权

- VOS OIDC access token：经 VOS userinfo 验证，身份以稳定 `sub` 为准；
- 共享网关 Key：作为 `gateway-admin`；
- 匿名：仅在未强制 Key 时允许普通推理，不能调用管理 API。

无效或过期凭证不会降级成匿名。管理 API 始终要求有效身份。当前所有已登录用户可以使用管理 API；主动注册的 private 模型已经按 owner 隔离，但手动上游配置和供应商资源仍是共享管理范围。

### 7.2 上游凭证

调用方的 OIDC token、Cookie、网关 Key 和路由选择头不会转发给模型供应商。V-Router 只向上游发送该上游保存的 API Key。日志、管理响应和错误不得回显凭证。

### 7.3 持久化

持久配置写入 VOS App Storage。用量按 VOS 用户分别写入 `data/usage/users/*.jsonl`，升级和普通卸载重装保留；仅用户明确选择删除应用数据时清除。

主动注册快照和进行中请求计数是进程内状态。网关重启后注册服务重新登记，调度配置按稳定上游 ID 恢复，不从磁盘复活旧计数或过期租约。

## 8. 用量与可观测性

每次请求记录用户、接口、模型、实际上游、延迟、状态和上游报告的 usage。当前解析 OpenAI Chat/Responses、Anthropic Messages、Ollama 原生计数和 OpenAI 兼容标准 usage。

管理界面按今天、本周、本月和全部范围，分别按模型、上游和用户汇总输入、输出、合计 token、请求数、失败数和延迟。

图片、视频、部分语音服务通常不报告 token；这种请求仍记录请求、失败和延迟，token 显示为“—”。V-Router 不按提示词长度、本地 tokenizer 或文件大小估算 token，也不把估算值混入供应商账单数据。

## 9. VOS 交付约束

- 应用 ID 为 `com.ictrek.v-router`；
- profile 必须由用户至少选择一个，并声明对应宿主机 caps；
- backend/frontend 均加入 `vos_default` 并提供稳定 aliases；
- 配置、用量和密钥使用 `VOS_APP_STORAGE_PATH`；
- AMD64、ARM64 镜像按现有构建脚本和飞书表发布；
- 发布顺序是子仓库代码、平台镜像、飞书记录、`update_version.sh`、终态 CI、Release/商店包、父仓库 gitlink；
- CI 或 tag 成功不代替真实 VOS 的 OIDC、模型列表和推理验证。

## 10. 接下来要做的事情

以下按优先级排序。未列入本节的早期设想不视为当前计划；已完成的事项从本节移入各阶段的完成记录，避免计划与实现脱节。

### 已完成：P0 统一接入（2026-09 关闭）

1. ASR 主动注册：V-Router 提供 `/api/v1/registrations` 注册、TTL/3 心跳、404 重注册和退出注销，`offline_asr` 协议转换与同账户 private 隔离均已实现；接入步骤见 [ASR 主动注册开发指南](asr-active-registration.md)。V-Voice 走发现元数据路线（`v-voice/` 前缀、热词与时间戳透传，0.0.28/0.0.29 发布），不需要为 V-Voice 另建注册器。
2. vLLM 主动注册：public/private 生命周期与多副本稳定 `service_id` 已实现，tc229 跨主机验证已记录，见 [vLLM 主动注册指南](vllm-active-registration.md)。
3. V-Voice 自动发现和调用已在 tc229、tc192、tc232 三台 VOS 上完成真实验证。
4. V-Music 自动发现、`v-music/` 命名空间和 `/v1/audio/music/generations` OpenAI 风格扩展已接入；只暴露 discovery 中 ready 的音乐模型。
5. ML Server 自动发现、`ml-server/` 命名空间、OpenAI `/v1/embeddings` 文本向量适配，以及图片推理/视频结构化 `/v1/ml/*` 扩展已接入；内部 callback 和运维路径不对外转发。
6. 已接入 V-Router 的应用：V-ChatCut（模型路由）、Image Studio（云端模型经 V-Router 调用）、V-Cine、V-Voice、V-Music、ML Server。

### 已完成：P1 可靠性与多用户边界（2026-09-18）

1. **有界熔断与恢复探测**：上游连续 3 次连接失败（拨号、连接拒绝、超时等传输层错误，不含上游返回的 4xx/5xx）自动进入冷却，首次 30 秒、指数退避、上限 5 分钟；冷却期结束后的下一个请求即为恢复探测，成功即复位。只影响自动调度选择，显式指定上游不绕过暂停和并发上限但可用于手动探测。熔断状态不落盘，网关重启后清零。
2. **上游状态分档**：管理 API 和界面对每个上游区分 健康、已暂停、租约过期、熔断中、鉴权失败、发现失败、无模型、满载 八种状态并给出原因，替代原来的"健康/异常"两态。
3. **手动/云端上游所有权**：创建时记录 owner（VOS `sub` 或 `gateway-admin`），可选 `visibility=public`（共享，默认）或 `private`（仅 owner 和管理员可见可用）。修改、刷新、删除和调度配置仅 owner 与管理员可操作；存量上游 owner 为空，升级后保持所有人可管理、不丢配置。
4. **供应商资源归属**：经 V-Router 创建的文件、批任务、视频任务、向量库等资源 ID 记录创建者；其他用户访问这些资源返回 404，owner 和管理员不受限。归属记录持久化到 VOS App Storage。列表接口仍返回上游共享账户的全部资源（共享供应商账户的固有限制），创建于本功能之前或绕过 V-Router 创建的资源不受限。
5. 跨账户授权测试覆盖 private 模型、上游管理和资源访问三类用例。

### P1（剩余）：固定回归

把多副本、权重、并发限制、取消释放和网关重启恢复纳入发布前固定回归清单，与 `gateway/testdata/live_sdk_check.py` 一起形成可重复执行的真实模型验收。

### P2：完善用量和诊断

目标：保持“只展示真实上游数据”，同时让无 token 的多模态请求也能被分析。

1. 保留 token 的“已报告/未报告”状态，不把 0 与缺失混为一谈。
2. 对上游明确报告的图片张数、音频时长、视频时长或请求单位设计独立字段，不折算成虚构 token。
3. 增加按用户、模型、上游和接口导出的脱敏诊断摘要。
4. 为用量迁移、损坏尾行、并发追加、升级保留和删除数据建立恢复测试。

验收：文本 token 与多模态单位分栏；上游未报告时界面不报错；数据生命周期符合 VOS 约定。

## 11. 明确不做

当前路线不包含以下事项，除非以后出现新的、经过确认的产品需求：

- 引入第二层模型代理或额外协议转换运行时；
- 增加 Python/Node 协议转换服务；
- 在 V-Router 内实现模型下载、GPU 调度或推理引擎生命周期；
- 对未报告 usage 的响应做本地 token 猜测；
- 自动重试生成、工具调用、图片或视频任务；
- 允许用户上传任意转换脚本或配置任意代理路径。

新增方向应先更新本节和“接下来要做的事情”，再开始实现，避免设计文档重新积累已废弃方案。
