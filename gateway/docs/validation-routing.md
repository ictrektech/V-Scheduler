> 迁移说明：网关现由 V-Scheduler 统一发布，源码位于 `gateway/`。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# 注册、路由与协议验收（2026-09-11）

验证使用隔离的新版网关、现有 ARM 主机上的实际 Ollama 服务，以及独立 HTTP 测试上游。正式 V-Router 实例未替换。聊天模型为 `qwen3:0.6b`；通过 Ollama 拉取 `nomic-embed-text` 用于 embedding。官方 OpenAI、Anthropic SDK 启用严格响应验证和 `max_retries=0`，不依赖 SDK 重试掩盖错误。

| 范围 | 验证方式与结果 |
| --- | --- |
| 文档与 API 一致性 | `TestEveryDocumentedEndpointIsUsable` 遍历路由表，通过真实 HTTP 测试全部文档端点；管理写入、资源固定上游及拒绝条件单独覆盖。使用测试上游的部分不代表实际模型具备音视频或有状态能力。 |
| 模型列表 | 实际 `/v1/models` 由官方 SDK 严格解析；发现并补齐必需 `created` 字段，未知创建时间为 0；`owned_by` 始终存在。 |
| Chat Completions | 真实模型普通和 SSE 回复非空，流结束正常。 |
| Legacy Completions | 固定主动注册的 Ollama 原生端点，官方 SDK 得到非空文本。 |
| OpenAI Responses → Chat Completions | 真实模型普通、SSE、function call 均通过；SDK 能取得完整最终响应及工具参数。 |
| Anthropic Messages → Chat Completions | 真实模型普通、SSE、tool_use 均通过；SDK 最终消息及 stop_reason 正确。 |
| 两种协议转换细节 | 独立 HTTP 上游验证工具参数分片、调用 ID、usage、结束事件、错误与取消；官方 SDK 严格解析。 |
| Embeddings | 实际 `nomic-embed-text` 返回两条不同输入的两组向量，均为 768 维、数值有限且向量不同。 |
| 主动注册 | 通过 HTTP 注册直连 Ollama 的服务端点，与 Model Hub 发现的 gateway 入口并存。两者是不同接入路径，共用同一推理引擎，不能视为新增独立算力。未部署新的 vLLM 实例。 |
| 自动及指定调用 | 短名实际命中不同接入路径，来源前缀限定路径；指定上游返回对应实例 ID。独立 HTTP 上游测试并发占用、SSE 取消释放、指定上游无切换及满载 429。 |
| 权重与并发 | 单元测试中 2:1 权重分配 60 次为 40:20；50 个并发申请、每后端上限 1 时仅两个成功。指定调用和其他模型范围不干扰轮换。 |
| 注册生命周期 | 验证鉴权、稳定 ID 幂等更新、心跳、到期退出、重注册和注销；另以真实 30 秒租约验证到期行为。 |
| 持久化 | 调度策略在重新发现及网关重启后保留；进行中计数不因发现刷新清零，注册租约不从磁盘复活。 |
| 界面 | Playwright 操作注册来源的“调度”，保存权重和上限；试聊 Enter 指定真实后端成功，显示实际命中 ID；新文档分组可见，无 JavaScript 页面异常。 |

Go 全量测试含 race、vet 通过。ARM Docker 构建也执行全量 Go 测试。

未把图片、音视频、rerank、向量库、批任务、Responses 历史/后台任务或 Anthropic 原生资源功能算作真实模型验收通过。这些能力需要对应原生上游；当前测试模型仅用于聊天和 embedding。两种协议转换是 Responses/Messages 入站转为 Chat Completions，不表示 Anthropic 原生后端可以反向承接全部 OpenAI 接口。

本轮修正的调度问题：以全局累计请求数打破平局，会让指定调用及其他模型范围的历史影响新候选分配。现在先比较上游全局进行中负载，再在请求模型/接口范围内做平滑加权轮换；真实同名模型分配及独立回归均已覆盖。

可重复运行的真实模型验收位于 `gateway/testdata/live_sdk_check.py`：配置 `V_ROUTER_TEST_BASE`（不含 /v1）、`V_ROUTER_TEST_KEY`、`V_ROUTER_TEST_CHAT_MODEL` 和 `V_ROUTER_TEST_EMBED_MODEL`，在已安装 openai/anthropic SDK 的环境执行。工具参数和协议结束事件的确定性验收由 `protocol_sdk_check.py` 与 Go 测试覆盖。

附加循环本机 HTTP 性能检查（不含模型推理，GOMAXPROCS=4，1 KiB 输入、32 并发）通过：Responses SSE 约 3262 请求/秒，Messages SSE 约 3647 请求/秒；相对 Chat SSE 的 P95 增量分别约 5.31 ms 和 4.53 ms。此结果仅用于网关开销对照，不代表真实模型吞吐。
