# V-Scheduler · 资源调度

面向 VOS 应用、其他服务、脚本和第三方 worker 的独立资源调度应用。统一管理耗时任务的排队、资源预约、执行许可、取消与故障恢复。

**当前状态：Rust controller、节点后台服务、Vue 管理界面及 Image Studio / V-Router 适配已完成最小版本。tc192、tc229 的实际 VOS 登录与节点资源页面，以及 tc229 的真实业务任务已验证。正式安装包见 [GitHub Releases](https://github.com/ictrektech/V-Scheduler/releases)。完整设计的后续能力不代表当前已支持。**

- 应用名称：V-Scheduler；中文显示名：资源调度。
- VOS app ID：`com.ictrek.v-scheduler`。
- 仓库：`git@github.com:ictrektech/V-Scheduler.git`。
- VAI 子模块路径：`apps/V-Scheduler`。

## 设计与接入标准

- [详细设计](docs/design.md)：任务协议、资源预约、公平排队、故障恢复、分阶段实施。
- [公开 API 与接入标准](docs/integration-standard.md)：VOS/非 VOS 客户端、服务身份、提交与 worker 接入、兼容性验收。
- [OpenAPI v1](api/openapi-v1.json)：对应当前 MVP 路由的机器可读 HTTP 接口。
- [现有架构证据](docs/architecture-evidence.md)：只读核验的 VOS 能力及限制。
- [节点发现与资源管理](docs/node-resources.md)：只读同步 VOS 节点与监控、Agent 真实 GPU 数据、观察与调度权限的边界及配置。
- [参考设计吸收与分阶段实施](docs/design.md#131-参考设计的增量吸收2026-10-14)：飞书调度/状态感知材料的取舍与验收顺序。

## 维护与部署范围

Scheduler、UI、Agent、协议及打包均由本仓库维护。VOS 平台和 vos-cluster-manager 代码不需要修改。

VOS 上只需安装本应用，本机资源采集作为应用内部组件自动初始化并随控制器运行；不需要用户部署项目、运行 Agent 或填写采集凭据。外部环境使用独立容器或 Agent 程序，经同一公开协议接入，无需 VOS 安装。

应用保留原业务入口，按任务类型选择接入：只提交任务，或注册 worker 提供执行能力。普通请求无需经过调度器；未接入重任务需要固定预算/设备隔离，否则不能保证整机资源不被抢占。

V-Router 不是所有本地算力应用的必经层。自有异步执行器由原应用直接提交 V-Scheduler 任务；通过 V-Router 的本地同步/流式模型由 V-Router 申请短期 permit；已持有任务预约的执行端若再调用 V-Router，须核验父任务与关联许可，避免重复扣预算。云端上游不占本机 GPU。按真实计算入口确定唯一预约责任，详见[设计中的路径判定](docs/design.md#23-v-router-不是所有算力应用的必经层)。

已纳管的其他节点与本 VOS 节点按各自的资源池预约；外部云端模型不申请本地许可。上游位于其他机器并不自动等于云端，接入步骤见 [VAI 总览](../../docs/vos-compute-access.md)和[公开 API 与接入标准](docs/integration-standard.md)。

## 运行实现

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

数据库迁移只作用于专用数据库。节点/执行器/凭据由 controller 配置批准；第一版每节点一条执行通道、FIFO 排队，资源预约和 V-Router 本地在线许可共享预算。云端请求不申请本地许可。VOS 节点发现/应用清单见节点资源文档。

controller 中每个 executor 的 `resources` 是单次执行的承诺成本；节点可选 `resident_baseline`（CPU millicores、RAM/GPU MiB、GPU 槽位）预留已实测的常驻开销，默认全零。运行时预算先扣常驻基线，再计算未释放的任务预约和在线许可。只有经目标机器实测批准的数值才能写入生产配置。Agent 的执行器配置可选 `health_url`（完整、由运维批准的 URL）；探测失败时暂停该执行器的新任务，旧 Agent 或未配置探测的执行器仍按心跳与资源判断。`/nodes` 分别显示 Agent 心跳、执行器探测、VOS 遥测采样时间；`/tasks` 与任务详情返回明确的 `wait_reasons`，包括离线、排空、执行器异常、承诺容量已满及观测压力。失联时现有 suspect 预约不会自动释放。

失败自动重试只针对已确认停止、可重试错误、声明幂等的执行器；`max_attempts` 为总尝试上限（1–10），退避 3/6/12/24/48 秒。人工重试也受上限约束。失联先进入 recovering/suspect，停止核验前不释放。人工删除只软删除已停止终态记录，保留审计、幂等占位与业务输出。

状态上报兼容只提供起止状态的执行器；可选的 `progress.percent` 使用 0–1 小数，`stage`/`message` 用于展示当前阶段与说明。`/events` 保存每个提交状态版本的进度快照，旧事件的进度为 `{}`；调度器仍以状态和 attempt/fence 为准，不能用进度判断是否已完成。

目前不支持动态 worker/执行器注册、HA、跨节点数据复制、多 GPU 设备级分配或任意远程命令。已验证的目标机器及业务范围见验证记录；Cluster Manager 节点清单接入仍需单独配置和验证。镜像及安装包发布另按仓库既有流程执行。

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

节点 `resident_baseline` 计常驻模型，`safety_reserve` 保护 VOS 余量；默认保留至少 2 GiB/10% RAM、一个 CPU 核和 256 MiB/5% 独立显存，CPU 95% 暂停准入。全部未释放预约与观测余量共同校验，统一内存仅用一个池。未知停止继续占用；关联子许可未停止时父任务不释放、不重试。控制器与内置采集共同受 512 MiB/1 CPU 容器限制。模板须按真实模型峰值校准，外部未纳管进程及不守契约的模型服务仍需执行端硬约束。

### 本机采集与 Jetson Thor

安装包按 VOS 硬件能力提供 NVIDIA、CPU、Jetson/L4T 与 Thor/Spark 运行配置，GPU 配置自动声明 NVIDIA runtime。首次启动从宿主机 `/proc/stat`、`/proc/meminfo` 初始化本机节点并启动内部采集；空配置也会出现实际资源，重启保留身份与 journal。没有单独部署采集组件的用户步骤。

每个逻辑处理器分别计算两次实际计数器的差值，点击数量可展开逐核心占用。GPU 类型未知显示未知，不默认当作独立显存。Thor 的 integrated 属性独立于 CUDA 上下文和显存查询；统一内存只按系统 RAM 池预算，NVML 不提供的显存数值仍保留为空，不伪造独立显存池。
