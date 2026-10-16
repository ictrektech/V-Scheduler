# HybRAG 后台网关身份（开发中）

本功能尚未随已安装的 `0.0.26` 发布，客户端热绑定、正式镜像和端到端验收仍在进行。它解决后台索引没有浏览器 OIDC、也不应复用网关管理员密钥的问题；不会创建模型执行器或替代调度准入。

控制器核验自己的 VOS 应用 context 身份，再向同一 VOS 确认 `com.ictrek.hybrag` 已安装。仅访问固定 `http://hybrag-app:8080/api/v1/scheduler/gateway-binding`，不接受发现结果指定的回调地址，不跟随重定向。GET 必须声明 `protocol_version=1`、固定 app/endpoint、`gateway-inference-v1` 能力与实际后端模式。`model-hub` 模式不激活本身份。

POST 使用 Scheduler 的 VOS context Bearer；交接包含版本、单调递增的 `binding_revision`、固定 app、`url=http://v-router:8080/v1`、独立随机 token 和 `scope=gateway-inference-v1`。客户端核验 VOS 身份后以 0600 原子持久化，并在实际发起内置模型请求时读取，以支持后台任务和已有客户端热更新。普通用户 API、日志、模型共享目录、DB 和内置模型 YAML 不应保存或返回该 token；旧 Model Hub 模式与用户自建模型保留其原凭据。

网关只授予以下路由，原有逐模型 Scheduler 策略继续生效：

| 方法 | 路径 | 用途 |
|---|---|---|
| GET | `/v1/models` | 已发布模型列表 |
| POST | `/v1/chat/completions` | QA、图片理解 |
| POST | `/v1/embeddings` | 文档与查询向量 |
| POST | `/v1/rerank` | 检索重排 |

该身份不是管理员，不可调用管理接口、Scheduler 控制接口或其他推理端点。网关逐请求读取 `/data/app-identities/hybrag.json` 的私有摘要快照，验证精确 app/scope、普通文件、0600、大小限制及 token SHA-256；不缓存服务身份，所以撤销快照即时影响后续请求。这里只约束服务凭据的权限，不改变原有 OIDC、显式网关 key 或内部匿名请求的既有配置。

控制器私有状态保存在 `/config/gateway-bindings/hybrag.json`，网关卷内只保存摘要。重启和交接重试复用持久凭据及 revision；明确卸载或切回 Model Hub 模式后停用摘要，再次启用时更新凭据与 revision。临时的 VOS/网络错误不会伪装为已卸载。客户端回执必须确认配置及对应 revision，失败后重试。

这不涉及 V-Sub 的每用户动态 namespace/root broker；该协议须独立实现和验收。
