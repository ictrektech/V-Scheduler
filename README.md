# V-Scheduler · 资源调度

统一入口、身份与调度责任见[统一接入指南](docs/application-integration.md)，各 profile 的实际发布与验收范围见[验收记录](docs/validation.md)。

本轮新增：资源页的「推理等待队列」、试聊有界排队和取消、交互/后台优先级与等待老化；模型仓库的「模型接入状态」统一关联权重、推理服务、网关和调度绑定。自带 Ollama 默认保留空闲模型 60 秒，连续调用可复用；切换或到期确认卸载后释放，驻留期间保留完整预算。管理员可设置 0..600 秒，0 保留每次卸载的行为。已验证身份调用自带 Ollama 且开启驻留时，未提供排队头会自动等待最多 30 秒，协调旧客户端并发与模型切换；显式 0、关闭驻留和其他后端仍立即准入或返回 429。使用方式、内外地址和边界见[排队与驻留接入](docs/application-integration.md#inference-queue-residency)。

原仓库的[文件选择下载 API](docs/file-download-api.md)、[网关协议](gateway/docs/api-protocols.md)、[注册与路由](gateway/docs/routing.md)、[鉴权](gateway/docs/auth.md)及其他网关指南已集中到本仓库；各文档标明统一应用入口与历史验证内容。

[HybRAG 后台网关身份](docs/hybrag-gateway-identity.md) 使用同 VOS 可信交接和独立推理凭据；不使用 Scheduler submitter token 代替网关 key，不授予管理权限。新 Voice 输入边界见其接入文档。

[Music / Desensitize / Image Studio 可信绑定](docs/application-auto-binding.md) 按实测运行设置、模型参数和输入限额接入调度；清单包含 Thor MusicGen-small / SD-Turbo、AMD MusicGen-small，以及 AMD / Thor / L4T Desensitize NER/OCR 的独立实测预算。各模型仅在完整运行设置与输入范围匹配时接入，具体真实业务验收见文档。

[V-Sub 私有 worker 可信绑定](docs/v-sub-auto-binding.md) 使用完整 namespace、worker generation 和已实测原生模型模板；L4T 与 Thor 的最终 Root worker 已通过真实字幕、直调/网关、业务取消和 generation 重启验证。

网关源码位于本仓库 `backend/src/router/`，与调度、模型仓库运行在同一个 Rust 进程；统一镜像使用原飞书 `v-scheduler` 列，构建命令见[源码与构建](docs/unified-application.md#source-ownership)。

V-Scheduler 统一安装包包含 V-Router 网关与调度控制器，提供一个界面和文档入口。模型目录、上游、自动发现、试聊、协议适配、用量和原有模型 API 由 Rust 网关统一提供；任务、许可、节点和资源统计由控制器管理。顶部登录身份使用随主题变化的文字颜色。安装后自动发现支持的应用与模型；界面“应用接入”说明本地、云端及 V-Voice 主动接入方式。API 与端口兼容说明见[统一应用说明](docs/unified-application.md)。 网关接口若返回 HTML 等无效 JSON，界面会提示检查服务与访问路径，不会将错误响应写入模型列表，切换语言仍可使用。

模型管理已使用 Rust/Axum 与 Vue/TypeScript 合并，入口为「模型仓库」：本地模型、HF/ModelScope/Ollama 下载、选定权重文件与追加下载、更新、压缩包导入、下载任务、三类 Ollama 与外部推理服务卡片、别名和容器引用均在同一应用内。安装时选择原 Model Hub 共享目录即可接管，保留原模型文件、注册表和任务；三个预热开关默认关闭。Ollama 仍由 `modules/ollama` 构建，打包继续读取原飞书 `ollama_server` 列。

仓库下载请求携带应用 User-Agent，兼容 ModelScope 权重 CDN；下载失败时，任务提供 HTTP 状态、网络、磁盘空间或校验等安全错误码，保留已有模型与可续传文件。

调度页面轮询跟随 VOS 更新后的登录令牌；过期会话会按未认证处理并获取新令牌，无需刷新页面恢复。真实非管理员访问仍返回权限不足。

顶部「关于」在所有页面均可打开，显示当前 VOS App 版本、运行配置，以及 Scheduler（内置 Router、模型仓库、界面）和三个 Ollama 实例共四个容器的镜像名与版本标签。

v0.0.31 修复浏览器 `Origin` / `Accept-Encoding` 透传导致原生推理拒绝或桥接解析失败的问题，已更新 192、229、232 并开展七个兼容应用的真实调用回归；逐 profile 结果和环境限制见[客户端验收记录](docs/validation.md#v0031-installed-client-compatibility)。

v0.0.30 将 Router 与模型仓库合入 Scheduler 的同一 Rust 进程，安装共四个容器；原网关/模型仓库别名、九组端口和共享目录保持兼容。192、229、232 的安装、取消恢复、重启恢复与五分钟稳定性结果见[验证记录](docs/validation.md#v0030-publication-and-installed-acceptance)。192 的 BGE-m3 / 重排序仍可能因现有容量和观测内存余量被拒绝，该环境限制未被绕过。

自带 Ollama 调用中断后，控制器自动向固定后端核验原生加载与进程退出状态；停止已确认就释放残留许可并恢复后续调用。活动调用、停止未确认及其他应用的预约不会被清除。待核验的旧调用占用资源时，接口会明确提示停止核验，而非笼统地提示机器忙。

网关向推理后端转发时移除浏览器 `Origin` 和 `Accept-Encoding`，避免 Ollama 将来自 VOS / V-ChatCut 的服务器调用误判为跨域请求，并确保协议转换可以解析 JSON / SSE。

本地同步推理在资源忙时及时返回 HTTP 429，自动绑定尚未就绪时返回 HTTP 503；网关不再静默等待数分钟而使客户端超时。

试聊沿用现有页面。Router 直接连接自带的三个后端，上游名为 `v-scheduler/ollama-qa`、`v-scheduler/ollama-embedding`、`v-scheduler/ollama-rerank`，不通过 Model Hub 发现。模型管理与调度任务使用独立 API 命名空间。内部别名、外部宿主端口、完整 API 与接管说明见[模型管理接入](docs/model-management.md)及应用内「模型管理 API」。

试聊在等待响应、模型思考和接收正文时显示当前阶段与耗时；请求期间发送按钮变为「停止」，可取消本次调用并保留已收到的正文；停止后由后端核验计算退出再释放资源。思考流与正文分别识别，只有正文作为回答。HTTP 成功后的流错误和没有正文的结束会明确报错，避免空白或一直显示省略号。

自带 Ollama 的完整 SSE `[DONE]` 事件作为同步计算结束证据；客户端在该事件后关闭连接，不再因尚未读取传输 EOF 而留下停止未知的许可。此规则只用于固定来源的自带 Ollama；中途断连、残缺事件及其他后端仍保留原停止确认要求。

## Host alias、内部端口与宿主机端口

以下是所有 profile 共用的默认映射。`<vos-host>` 为安装该应用的 VOS 宿主机 IP 或域名；192、229、232 三台测试机分别使用 `192.168.1.192`、`192.168.1.229`、`192.168.1.232`。内部地址仅在 Docker `vos_default` 网络解析，完整 URL 在地址前加 `http://`。安装时修改端口参数后，以实际安装值为准；应用内「API 文档 → 模型管理 API」显示配置后的地址。

| 功能 | 正式 Docker host alias:内部端口（兼容名另注） | 宿主机 host:外部端口 | 主要路径与用途 |
|---|---|---|---|
| 统一推理、网关管理及统一界面 | `v-scheduler:8080`；兼容：`v-scheduler-gateway:8080`、`v-router:8080`、`v-router-backend:8080` | `<vos-host>:22888` | `/v1/*` 推理；`/api/v1/*` 网关管理；`/` 界面 |
| 调度接管与资源管理 | `v-scheduler:38120`；兼容：`v-scheduler-backend:38120`、`v-scheduler-frontend:38120` | `<vos-host>:38120` | `/api/v1/*` 任务、许可、节点、执行器及统计 |
| 模型仓库管理 | `v-scheduler:6005`；兼容：`v-scheduler-model-hub:6005`、`v-scheduler-model-hub-backend:6005` | `<vos-host>:36005` | `/api/v1/*` 模型、选中文件下载、更新、导入、别名、引用和后端管理；`/openapi.json` |
| Ollama QA 原生管理 | `v-scheduler-ollama-qa:11434` | `<vos-host>:12434` | `/api/*` 模型清单、拉取、详情、加载/卸载及原生推理 |
| Ollama QA 推理网关 | `v-scheduler-ollama-qa:11535` | `<vos-host>:12535` | `/v1/chat/completions` 聊天；`/metrics` 指标 |
| Ollama Embedding 原生管理 | `v-scheduler-ollama-embedding:11434` | `<vos-host>:12435` | `/api/*` 原生模型管理及向量推理 |
| Ollama Embedding 推理网关 | `v-scheduler-ollama-embedding:11535` | `<vos-host>:12536` | `/v1/embeddings` 向量；`/metrics` 指标 |
| Ollama Rerank 原生管理 | `v-scheduler-ollama-rerank:11434` | `<vos-host>:12436` | `/api/*` 原生模型管理及重排推理 |
| Ollama Rerank 推理网关 | `v-scheduler-ollama-rerank:11535` | `<vos-host>:12537` | `/v1/rerank` 重排；`/metrics` 指标 |

可配置的外部端口参数：

- `V_ROUTER_HOST_PORT`：默认 `22888`。
- `SCHEDULER_HOST_PORT`：默认 `38120`。
- `MODEL_HUB_BACKEND_HOST_PORT`：默认 `36005`。
- `OLLAMA_QA_HOST_PORT`：默认 `12434`。
- `OLLAMA_QA_GATEWAY_HOST_PORT`：默认 `12535`。
- `OLLAMA_EMBEDDING_HOST_PORT`：默认 `12435`。
- `OLLAMA_EMBEDDING_GATEWAY_HOST_PORT`：默认 `12536`。
- `OLLAMA_RERANK_HOST_PORT`：默认 `12436`。
- `OLLAMA_RERANK_GATEWAY_HOST_PORT`：默认 `12537`。

应用内「API 文档 → 应用接入 / 模型管理 API」与[统一接入指南](docs/application-integration.md)分别注明统一正式别名和旧客户端兼容别名，端口表不只列旧 Router 的兼容入口。

新应用和已有应用后续适配统一推荐使用 `v-scheduler`，按功能选择 `8080`（网关）、`38120`（调度）、`6005`（模型管理）。统一容器只有这一个正式 host alias，其余七个名称全部作为兼容别名保留；与旧独立 V-Router 共存时，共享的 `v-router` 兼容别名可能解析到旧容器，不应用于传递 V-Scheduler 私有凭据。

同一行的兼容别名指向同一个容器和端口，不产生额外服务或宿主机映射。`host.docker.internal` 是网关容器中的 Docker 宿主机别名（`host-gateway`），用于访问宿主机已发布端口，例如 `host.docker.internal:38120`；它不是新增服务，也没有单独的固定端口。

网关还在同一 `22888` 端口提供 `/scheduler/api/v1/*` 调度代理和 `/model-hub/api/v1/*` 模型管理代理。VOS 内统一入口为 `/app/com.ictrek.v-scheduler/`；模型管理前缀为 `/app/com.ictrek.v-scheduler/model-hub/api/v1/*`，网关 API 前缀为 `/api/com.ictrek.v-scheduler`。三个 Ollama 的 `11535` 是各后端推理/指标入口；跨后端统一模型名和调度入口使用 `v-scheduler:8080/v1/*` 或 `<vos-host>:22888/v1/*`。

这些别名由 V-Scheduler 自己声明；原 Model Hub 的别名、端口及共享目录保持原应用所有权。共享模型目录仍在安装时选择，沿用原 Model Hub 所选公共目录即可。

0.0.20 起，“节点管理”通过 VOS App 发现和管理节点：读取 ModelInfer 已有节点及监控、保存预算规划、管理已受控接入节点的预算与执行器、撤销服务凭据并查看操作记录。用户无需逐台部署 Agent。ModelInfer 远端调度控制接口已预留，验证前保持不可调度；底层固定多节点在线许可协议已通过隔离验收，不等于真实多机部署已经接入。见[节点管理](docs/node-management.md)。

0.0.21 修复同日升级可能沿用旧控制器镜像的问题：所有运行配置均强制检查并拉取当前发布镜像，避免安装版本已更新而后台代码仍停留在旧版。

同步模型后端现在可通过 `POST /api/v1/permits/{id}/verify` 核验并原子认领
V-Router 的同一次预算。配置独立的 `permit_executors` 权限，后端不需要、也不能
复用网关令牌或再预约一份资源。普通与 delegated 子许可均绑定执行令牌、模型、
地址和执行器；参见[应用接入指南](docs/application-integration.md)及
[完整契约](docs/integration-standard.md)。

VOS 安装中的 V-Voice 可由现有发现器主动接入：控制器核验同一 VOS 的应用身份，
仅为匹配已校准硬件与运行设置、且模型文件已就绪的模型建立受限服务身份和执行器，
自动推送持久化的绑定版本。网关与内置 Agent 热更新配置，无需用户分发令牌。
未校准模型继续拒绝，资源不足继续排队或拒绝；见 [V-Voice 主动接入](docs/v-voice-auto-binding.md)。
V-Voice 已验证的 Python 直调若在准入回执返回前超时，协调器会在租约过期后原子回收从未认领的许可，避免一次失败永久占用预算。只处理匹配当前可信 `0.0.21` / `0.0.22` 绑定的直调；已认领、关联任务、委派和未知计算仍保留预约，详见上述主动接入文档。
原生音乐单独匹配已实测的 `execution_settings.music` 和逐模型时长/提示词边界，不能继承语音审批；没有真实生成及停止证据时保持不可调度。

面向 VOS 应用、其他服务、脚本和第三方 worker 的独立资源调度应用。统一管理耗时任务的排队、资源预约、执行许可、取消与故障恢复。

**当前状态：Rust controller、节点后台服务、Vue 管理界面及 Image Studio / V-Router 适配已完成最小版本。tc192、tc229 的实际 VOS 登录与节点资源页面，以及 tc229 的真实业务任务已验证。正式安装包见 [GitHub Releases](https://github.com/ictrektech/V-Scheduler/releases)。完整设计的后续能力不代表当前已支持。**

- 应用名称：V-Scheduler；中文显示名：资源调度。
- VOS app ID：`com.ictrek.v-scheduler`。
- 仓库：`git@github.com:ictrektech/V-Scheduler.git`。
- VAI 子模块路径：`apps/V-Scheduler`。

## 统一配置与接口契约

网关、调度器与模型仓库共用配置/身份说明和[错误契约](docs/api-contract.md)。VOS 根地址可通过 `SCHEDULER_VOS_URL` 提供给统一 Rust 服务，原 userinfo 覆盖保留。统一模型仓库入口复用网关登录；原生受信任内网接口保持兼容。应用 API 错误统一提供 `code`、`request_id` 与 `X-Request-ID`，保留旧 `error.message` / `detail`；前端统一服务路径、令牌轮换和错误解析。成功响应及推理协议保持原样，具体调度拒绝原因不会再被折叠为没有业务码的通用错误。模型下载的终态发布与活动记录清理保持同步，失败后可立即重试。

## 设计与接入标准

**[统一接入指南](docs/application-integration.md)**：模型仓库推理服务卡片、模型网关注册、调度接管与停止确认按同一实例的生命周期衔接；vLLM 主动注册、public/private、是否启用调度、共享模型目录及远端节点也统一在这一份文档维护操作步骤。应用内「API 文档 → 应用接入」也指向该指南。

应用内「模型与网关 API」同时列出正式的 `v-scheduler:8080/v1` 和兼容的 `v-scheduler-gateway:8080/v1`、`v-router:8080/v1`、`v-router-backend:8080/v1`；「调度管理 API」列出控制器直连和新旧网关代理地址。多个 host alias 是统一容器按原服务角色保留的 DNS 名称，不会增加容器；端口区分网关、调度和模型管理入口。

连续调用遇到 `WAIT_STOP_CONFIRMATION` 时，统一网关最多等待八秒重新申请准入，仅在原生停止确认后转发一次推理；其他容量拒绝仍立即返回。`WAIT_MEMORY_HEADROOM` 单独表示可用宿主内存不足，内置本机执行组件在申请许可时重新读取可用 RAM，避免使用卸载前的旧采样。系统安全余量、远端节点验证及未知停止预约保持有效，见[接口与错误契约](docs/api-contract.md#本机内存余量与连续调用)。192 的受限语音/CPU 脱敏配置和 232 的 V-Sub CPU 执行使用匹配硬件、运行参数及最大输入的校准，不能据此批准其他模型或 CUDA 插件。


- [详细设计](docs/design.md)：任务协议、资源预约、公平排队、故障恢复、分阶段实施。
- [网关资源准入](docs/resource-scheduling.md)：统一应用内的逐模型授权、许可交接与停止证据。
- [公开 API 与接入标准](docs/integration-standard.md)：VOS/非 VOS 客户端、服务身份、提交与 worker 接入、兼容性验收。
- [OpenAPI v1](api/openapi-v1.json)：对应当前 MVP 路由的机器可读 HTTP 接口。
- [现有架构证据](docs/architecture-evidence.md)：只读核验的 VOS 能力及限制。
- [VOS 节点管理](docs/node-management.md)：应用发现、已有监控、预算规划与预留控制接口。
- [节点发现与资源管理](docs/node-resources.md)：只读同步 VOS 节点与监控、Agent 真实 GPU 数据、观察与调度权限的边界及配置。
- [参考设计吸收与分阶段实施](docs/design.md#131-参考设计的增量吸收2026-10-14)：飞书调度/状态感知材料的取舍与验收顺序。

## 维护与部署范围

Scheduler、UI、Agent、协议及打包均由本仓库维护。VOS 平台和 vos-cluster-manager 代码不需要修改。

VOS 上只需安装本应用，本机资源采集作为应用内部组件自动初始化并随控制器运行；不需要用户部署项目、运行 Agent 或填写采集凭据。外部计算节点通过所属 VOS 应用管理，用户不需要另外下载或运行 Agent；未经验证的远端控制保持禁用。

应用保留原业务入口，按任务类型选择接入：只提交任务，或注册 worker 提供执行能力。普通请求无需经过调度器；未接入重任务需要固定预算/设备隔离，否则不能保证整机资源不被抢占。

V-Router 不是所有本地算力应用的必经层。自有异步执行器由原应用直接提交 V-Scheduler 任务；通过 V-Router 的本地同步/流式模型由 V-Router 申请短期 permit；已持有任务预约的执行端若再调用 V-Router，须核验父任务与关联许可，避免重复扣预算。云端上游不占本机 GPU。按真实计算入口确定唯一预约责任，详见[设计中的路径判定](docs/design.md#23-v-router-不是所有算力应用的必经层)。

已纳管的其他节点与本 VOS 节点按各自的资源池预约；外部云端模型不申请本地许可。上游位于其他机器并不自动等于云端，接入步骤见[应用接入指南](docs/application-integration.md)和[公开 API 与接入标准](docs/integration-standard.md)。

## 运行实现（应用开发与维护）

核心服务为 Rust Axum/Tokio + SQLx PostgreSQL，界面为 Vue 3 / TypeScript。数据库推荐复用独立 PGV 服务，为 V-Scheduler 创建专用 `v_scheduler` 数据库与普通 owner 角色；不连接 VOS 平台内部数据库，不使用 PGV 超级用户或其他应用角色。只需 controller 连接数据库，Agent 无需本地 PGV。测试使用隔离 PostgreSQL 容器。

```sh
export CARGO_TARGET_DIR="$HOME/dev/build/V-Scheduler/target"
cd backend
cargo build --locked
cd ../frontend
npm ci
npm run build -- --outDir "$HOME/dev/build/V-Scheduler/ui"
cd ..
# 先将 config/*.example.json 复制到受保护文件，替换身份令牌/地址/预算。
# SCHEDULER_DATABASE_URL 通过受保护环境文件注入，不写入仓库。
"$CARGO_TARGET_DIR/debug/v-scheduler" controller --config /secure/controller.json --ui-dir "$HOME/dev/build/V-Scheduler/ui"
"$CARGO_TARGET_DIR/debug/v-scheduler" agent --config /secure/agent.json
```

服务默认监听 `127.0.0.1:38120`；公开 API 位于 `/api/v1`，公开发现 `/api/v1/discovery`，机器规范 `/api/v1/openapi.json`。VOS 内界面复用当前平台登录，由后端通过 `/v1000/user/check` 校验启用的 `system_admin` 身份；不要求用户填写 Scheduler 令牌。独立部署仍支持服务令牌。节点任务排布显示执行、等待顺序、近期结束任务，并提供取消、受控重试和删除；资源面板显示 CPU、内存与 GPU 占用环图、可展开的逐逻辑处理器占用及一分钟采样曲线。Jetson 统一内存只按系统内存计一次；Orin 的 GPU 使用率在 `nvidia-smi` 不提供数值时读取可用的内核负载计数。连接前可打开清楚标注的演示模式，演示操作只修改页面样例。默认跟随 VOS 语言，支持简中/英文/繁中，手动选择保存于应用自己的存储。

数据库迁移只作用于专用数据库。原有节点/执行器/凭据由 controller 配置保留，动态接入由管理员在界面或管理 API 批准并持久化到专用数据库；第一版每节点一条执行通道、FIFO 排队，资源预约和 V-Router 本地在线许可共享预算。云端请求不申请本地许可。VOS 节点发现/应用清单见节点资源文档。

controller 中每个 executor 的 `resources` 是单次执行的承诺成本；节点可选 `resident_baseline`（CPU millicores、RAM/GPU MiB、GPU 槽位）预留已实测的常驻开销，默认全零。运行时预算先扣常驻基线，再计算未释放的任务预约和在线许可。只有经目标机器实测批准的数值才能写入生产配置。Agent 的执行器配置可选 `health_url`（完整、由运维批准的 URL）；探测失败时暂停该执行器的新任务，旧 Agent 或未配置探测的执行器仍按心跳与资源判断。`/nodes` 分别显示 Agent 心跳、执行器探测、VOS 遥测采样时间；`/tasks` 与任务详情返回明确的 `wait_reasons`，包括离线、排空、执行器异常、承诺容量已满及观测压力。失联时现有 suspect 预约不会自动释放。

失败自动重试只针对已确认停止、可重试错误、声明幂等的执行器；`max_attempts` 为总尝试上限（1–10），退避 3/6/12/24/48 秒。人工重试也受上限约束。失联先进入 recovering/suspect，停止核验前不释放。人工删除只软删除已停止终态记录，保留审计、幂等占位与业务输出。

状态上报兼容只提供起止状态的执行器；可选的 `progress.percent` 使用 0–1 小数，`stage`/`message` 用于展示当前阶段与说明。`/events` 保存每个提交状态版本的进度快照，旧事件的进度为 `{}`；调度器仍以状态和 attempt/fence 为准，不能用进度判断是否已完成。

当前支持经管理员批准的动态 worker/执行器登记，以及固定模板的多节点在线许可。仍不支持 HA、跨节点数据复制、多 GPU 设备级分配、异步多节点 gang 调度或任意远程命令。已验证的目标机器及业务范围见验证记录；Cluster Manager 节点清单接入仍需单独配置和验证。镜像及安装包发布另按仓库既有流程执行。

`scripts/e2e.mjs` 是获授权 tc232 测试环境的故障注入工具，使用独立端口/容器/数据目录；需要已安装 Image Studio 镜像和只读模型挂载。构建及测试遵循 VAI 指定的远程构建规则。

镜像构建、版本脚本和 VOS 包流程见 [打包与发布](ictrek.app/README.md)。

已完成的检查及部署边界见 [MVP 验证记录](docs/validation.md)。

首次安装不要求已有 `database.env`：PGV 与 Scheduler 均读取当前 VOS 主机注入的 `VOS_POSTGRES_PASSWORD`，VOS 管理员打开 Scheduler 时自动创建其专用数据库和随机普通角色，再启动控制器。VOS 应用详情会遮蔽密码，因此不从该接口读取凭据。旧版 PGV 若使用不同密码，可由安装人员在 VOS 主机运行 `python3 ictrek.app/scripts/prepare_pgv.py`，从运行中的 PGV 向 Scheduler 专属 App Storage 写入一次性交接文件；成功后自动删除。PGV 未安装或不可达时保留初始化状态，不回退到 VOS 平台数据库。业务应用和 Agent 使用独立服务身份。

VOS 侧导航只有一个 `v-scheduler` 页面；资源任务和 API 文档使用应用内部导航。

VOS 登录接入清单必须声明 `oauth2.client: {}`，与 V-Router 相同；该声明使平台注册客户端并提供已有登录桥接接口。界面不提供单独的令牌登录表单。

Agent 允许空执行器配置：这种模式只上报节点资源，不认领任务，适合先纳管节点、再逐步接入业务。VOS 页面入口带安装版本查询参数，使升级后的 iframe 使用新页面。

Agent 是本应用在节点上运行的资源采集与任务执行后台进程，不是用户需要选择或命名的应用。资源页优先显示 VOS 集群登记名称；未接集群清单时使用该节点 VOS 的 `device_name`，再退回 VOS 上报的主机名。Agent 的 `node_id` 只用于内部协议关联，不作为节点显示名称。每台 VOS 设备安装本应用后自动接入本机采集，内部身份和恢复日志保存在应用私有数据中。

### 网关完整准入与系统资源保护

新版 V-Router 的 VOS 包强制依赖 Scheduler `>=0.0.11`，逐模型批准输入、资源参数与目标 executor。HTTP/SSE、WebSocket 与异步创建均有准入；异步接力和已运行任务子调用必须经数据库核验，不能凭 delegated 声明跳过。`GET /api/v1/permits` 显示模型许可历史，executor `upstream_urls` 绑定实际计算地址。接口以 [OpenAPI](api/openapi-v1.json) 和 [接入标准](docs/integration-standard.md) 为准。

节点 `resident_baseline` 计常驻模型，`safety_reserve` 保护 VOS 余量；默认保留至少 2 GiB/10% RAM、一个 CPU 核和 256 MiB/5% 独立显存，CPU 95% 暂停准入。全部未释放预约与观测余量共同校验，统一内存仅用一个池。未知停止继续占用；关联子许可未停止时父任务不释放、不重试。统一服务与内置采集共同受 1280 MiB/3 CPU 容器限制。模板须按真实模型峰值校准，外部未纳管进程及不守契约的模型服务仍需执行端硬约束。

### 本机采集与 Jetson Thor

安装包按 VOS 硬件能力提供 NVIDIA、CPU、Jetson/L4T 与 Thor/Spark 运行配置，GPU 配置自动声明 NVIDIA runtime。首次启动从宿主机 `/proc/stat`、`/proc/meminfo` 初始化本机节点并启动内部采集；空配置也会出现实际资源，重启保留身份与 journal。没有单独部署采集组件的用户步骤。

每个逻辑处理器分别计算两次实际计数器的差值，点击数量可展开逐核心占用。GPU 类型未知显示未知，不默认当作独立显存。Thor 的 integrated 属性独立于 CUDA 上下文和显存查询；统一内存只按系统 RAM 池预算，NVML 不提供的显存数值仍保留为空，不伪造独立显存池。

控制器启动只读取 CPU/RAM，GPU 探测由后台采集完成；重启保留已批准容量，首次安装的 GPU 容量在首次有效心跳后补齐。`nvidia-smi` 超时后必须杀死并等待回收旧进程，旧进程尚未停止时不启动下一次探测，避免驱动阻塞造成探测进程堆积、容器 OOM 和页面失联。采样停滞时保留旧采样时间并拒绝新计算，不把未知资源当作空闲。

管理页的“负载与可靠性”展示最近 24 小时的首次排队等待、任务执行占用、任务失败率和模型许可占用；“模型调用许可”可筛选、分页查看在线模型的节点、预算、停止或移交依据。统计每 30 秒更新，许可历史每 2.5 秒刷新。

`GET /api/v1/statistics` 按服务 `app_id` 隔离，管理员可查看全部。首次等待只取任务第一次分配；执行占用包含分配与停止核验，重试按各次执行统计。失败率为失败 /（成功 + 失败），取消不计入；许可释放不能代表业务推理成功。没有样本时返回 `null`，P95 至少需要 20 条有效样本。新许可保存首次 `released_at`；旧许可没有实际释放时间时保持未知，不使用租约截止时间推算。`GET /api/v1/permits?state=active|suspect|released&cursor=0&limit=50` 提供按状态分页历史。此统计不自动调整节点选择。

数据库仅使用 PostgreSQL 内置表、事务、JSONB、行锁和聚合功能，不依赖 pgvector、pg_search 或 ParadeDB 扩展。

统计回归测试可在已同步源码的 Linux 测试机运行 `node scripts/e2e-statistics.mjs`：需 Docker、Node 22.18+、已构建的 `~/dev/build/V-Scheduler/cargo/debug/v-scheduler` 和 `~/dev/build/V-Scheduler/ui-verify/dist`，以及迁移前版本的已安装镜像（或 `SCHEDULER_LEGACY_IMAGE`）。测试使用独立临时 PostgreSQL 和内部网络，检查完成后自动清理；`KEEP_TEST_SERVICES=1` 可保留至页面检查结束，向记录的测试 PID 发送 SIGTERM 即清理。

### 任务跟踪与排查

点击任务的业务 ID 打开详情，概览、状态时间线和执行历史每 2.5 秒持续更新，切换页签保留位置。时间线展示数据库中已保存的状态版本、时间和进度快照，可加载更早事件；执行历史区分每次分配、确认停止及资源占用时长。失败任务显示当前是否允许重试，重试仍保留在原任务历史中。排队详情列出完整等待原因及检查建议，不把已等待时长当作开始时间预测。

任务列表支持状态、应用、节点组合筛选，并搜索业务任务 ID、调度任务 ID、应用和执行器。默认加载最近 200 条；存在更早任务时明确标记筛选范围，并提供“加载更早任务”。打开的详情独立查询原任务，不随列表翻页消失。

`GET /api/v1/tasks/{id}/events?cursor=0&limit=50` 返回单个可见任务的事件，按最新在前排列。`next_cursor` 用于查询更早事件，无更早页时为 null。原 `/events` 的正向增量读取契约不变。

内置三个 Ollama 后端和已下载模型由应用自动纳管，可直接使用现有试聊，无需逐模型批准、SSH 或手写策略。统一调用保留 Ollama 的安装配置和请求参数，采用基于模型大小的保守内存估算，默认独占本机已配置的计算预算；资源忙时等待计算资源。详见 [模型管理接入](docs/model-management.md#built-in-ollama-automatic-scheduling)。

自动纳管回归测试：在已同步并完成构建的 Linux 测试机执行 `python3 scripts/e2e-builtin-ollama.py`。测试使用独立 PostgreSQL、内部 Docker 网络与模拟后端，覆盖自动登记、私有凭据、三类路由、流式输出、资源等待、许可释放和重启恢复；真实设备验证仍需单独完成。

192 的已实测 CPU 小模型预设使用固定的本地 Ollama 标签，保留原模型与全局默认值：Qwen2.5 0.5B q2（896 MiB）与 all-minilm 22m（512 MiB），每次计算预算 4.5 核、GPU 为 0，上下文 512、输入最多 16 KiB、生成最多 128 tokens。它们通过同一模型仓库卡片、统一网关和真实 Scheduler 许可管理，取消后须取得原生停止证明；不把该实测预算套用到其他权重、硬件或并发设置。详见[统一接入指南](docs/application-integration.md#calibrated-local-models)。

V-Voice 混合绑定同时包含旧语音审批和新音乐输入边界时，双方显式协商 `mixed_input_limits_version=1`，旧兼容模型由受信任审批单独列出；缺少限制不会自动取得豁免。详见[自动绑定契约](docs/v-voice-auto-binding.md#原生模型输入边界)。

### 启动状态与地址迁移

网关 `/healthz` 为进程存活，`/readyz` 为内置模型目录就绪状态；两者均免鉴权。首次发现返回 `503 MODEL_CATALOG_INITIALIZING`，不会提前给出“没有模型”的成功空列表。管理页面显示自动更新的发现/后端故障提示。控制器准入（含响应正文）的三秒超时使用独立 `SCHEDULER_ADMISSION_TIMEOUT`，资源不足仍保留具体 429 业务码，详见[错误契约](docs/api-contract.md)。

新客户端统一推荐 `v-scheduler`；HybRAG、V-Sub 的受限绑定按能力协商迁移并保留凭据与 revision 单调性。旧版本和显式兼容配置继续按原契约工作，不覆盖公共模型目录、用户模型选择或原生运行设置。

内置 Ollama 权重更新和删除会先核验驻留卸载及未释放预约；有排队、执行或下载任务时返回 409，请在任务结束后重试。自动资源绑定按当前原生模型大小核对；未完成绑定返回 `503 MODEL_BINDING_PENDING`，不会沿用旧权重的预算。旧 Model Hub 或直接操作原生管理端口的写入方，应先停止该模型并确认卸载，再变更共享权重；进程内互斥不覆盖外部写入方。

驻留设置和模型关联管理使用与调度管理相同的 VOS 管理员校验；普通用户只能查看和取消自己的等待请求，不通过用户名推断管理员权限。

### 应用统一生命周期接入

新应用可通过调度 API `POST /integrations` 登记已批准执行器，再按响应间隔调用 `/integrations/heartbeat`；`GET /integrations` 统一查询应用心跳、节点采样和批准预算。节点由 VOS App 内置组件采集，无需用户手工部署 Agent。申请停止使用 `/permits/{id}/stop`，原生执行端确认退出及卸载后通过 `/permits/{id}/stopped` 回执；不因失联或超时释放预约。已有应用自动绑定兼容保留，未实现此协议不宣称已支持统一停止。完整字段、身份、重试、预算审批和异步取消见[统一接入指南](docs/application-integration.md#unified-lifecycle)。

### 执行应用的自动生命周期

支持该协议的 V-Voice（含配乐）、V-Sub、Desensitize、Image Studio 在 VOS 安装及可信绑定后自动登记批准执行器、维护应用心跳。无需用户部署 Agent，节点遥测继续由本应用采集。节点管理的「应用统一接入」同时显示节点新鲜度、预算、同步执行及异步任务；「停止并卸载」或「取消」只有在执行端确认原生退出和模型卸载后才释放预约。已确认回执保存在执行应用私有目录，重启后可重试；未知停止继续占用预约。调用方 V-ChatCut、HybRAG 不重复申报执行端资源。详见[统一流程](docs/application-integration.md#unified-lifecycle)。


### 任务优先级与协作让出

向本应用的推理接口发送请求时，可选传入以下请求头：
`X-VScheduler-Priority: background|interactive|high`、
`X-VScheduler-Preemptible: true|false`、
`X-VScheduler-Allow-Preemption: true|false`、
`X-VScheduler-Force-Admission: true|false`。
默认后台优先级，三个开关均关闭。抢占需要当前任务同意让出且新任务允许抢占；
原生执行退出、模型卸载确认前仍占用预算。同步调用停止后由调用方决定是否重新提交，
不会自动重放。强制尝试须携带当前管理员的 Bearer 身份，由 V-Scheduler 核验，
不突破批准预算、停止屏障或至少 1 GiB 内存余量；凭证不写入任务。
完整流程见 [V-Scheduler 统一接入指南](https://github.com/ictrektech/V-Scheduler/blob/main/docs/application-integration.md)。

异步 Image Studio 可声明 `scheduling.preemptible:true`，原生进程确认退出后
使用新的 attempt/fence 从头重跑；同步请求仅停止，不自动重放。
任务 API 使用 `scheduling` JSON 对象，统一推理入口使用上述请求头。
统一内存节点可显式设置 `safety_reserve.memory_mib: 1024`；未配置的节点继续
使用自动安全余量。强制尝试由管理员授权并留有审计。

Image Studio 声明可重跑能力后，Scheduler 为新的执行策略建立独立的已校准执行器身份，
在空闲时迁移绑定并保留预算、共享目录和凭证；不会覆盖管理员对旧执行器的编辑或禁用。

在已同步的构建机上运行 `python3 scripts/e2e-image-policy-migration.py`，可验证
旧策略升级、空发现期间的管理员禁用保护、显式恢复后的身份迁移及预算/凭证保留。
该隔离协议测试使用临时 PostgreSQL、Node 服务及 Rust 控制器，测试模型声明不是生产校准。
