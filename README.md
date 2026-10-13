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

## 维护与部署范围

Scheduler、UI、Agent、协议及打包均由本仓库维护。VOS 平台和 vos-cluster-manager 代码不需要修改。

VOS 上通过独立应用包安装 controller/node 模式，再经现有 projects 接口部署本应用自己的 `resource-agent` 子项目。外部环境使用独立容器或 Agent 程序，经同一公开协议接入，无需 VOS 安装。

应用保留原业务入口，按任务类型选择接入：只提交任务，或注册 worker 提供执行能力。普通请求无需经过调度器；未接入重任务需要固定预算/设备隔离，否则不能保证整机资源不被抢占。

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

服务默认监听 `127.0.0.1:38120`；公开 API 位于 `/api/v1`，公开发现 `/api/v1/discovery`，机器规范 `/api/v1/openapi.json`。VOS 内界面复用当前平台登录，由后端通过 `/v1000/user/check` 校验启用的 `system_admin` 身份；不要求用户填写 Scheduler 令牌。独立部署仍支持服务令牌。节点任务排布显示执行、等待顺序、近期结束任务，并提供取消、受控重试和删除；资源面板显示 CPU、内存与 GPU 占用环图、逻辑处理器数量及一分钟采样曲线。Jetson 统一内存只按系统内存计一次；Orin 的 GPU 使用率在 `nvidia-smi` 不提供数值时读取可用的内核负载计数。连接前可打开清楚标注的演示模式，演示操作只修改页面样例。默认跟随 VOS 语言，支持简中/英文/繁中，手动选择保存于应用自己的存储。

数据库迁移只作用于专用数据库。节点/执行器/凭据由 controller 配置批准；第一版每节点一条执行通道、FIFO 排队，资源预约和 V-Router 本地在线许可共享预算。云端请求不申请本地许可。VOS 节点发现/应用清单见节点资源文档。

失败自动重试只针对已确认停止、可重试错误、声明幂等的执行器；`max_attempts` 为总尝试上限（1–10），退避 3/6/12/24/48 秒。人工重试也受上限约束。失联先进入 recovering/suspect，停止核验前不释放。人工删除只软删除已停止终态记录，保留审计、幂等占位与业务输出。

目前不支持动态 worker/执行器注册、HA、跨节点数据复制、多 GPU 设备级分配或任意远程命令。已验证的目标机器及业务范围见验证记录；Cluster Manager 节点清单接入仍需单独配置和验证。镜像及安装包发布另按仓库既有流程执行。

`scripts/e2e.mjs` 是获授权 tc232 测试环境的故障注入工具，使用独立端口/容器/数据目录；需要已安装 Image Studio 镜像和只读模型挂载。构建及测试遵循 VAI 指定的远程构建规则。

镜像构建、版本脚本和 VOS 包流程见 [打包与发布](ictrek.app/README.md)。

已完成的检查及部署边界见 [MVP 验证记录](docs/validation.md)。

首次安装不要求已有 `database.env`：PGV 与 Scheduler 均读取当前 VOS 主机注入的 `VOS_POSTGRES_PASSWORD`，VOS 管理员打开 Scheduler 时自动创建其专用数据库和随机普通角色，再启动控制器。VOS 应用详情会遮蔽密码，因此不从该接口读取凭据。旧版 PGV 若使用不同密码，可由安装人员在 VOS 主机运行 `python3 ictrek.app/scripts/prepare_pgv.py`，从运行中的 PGV 向 Scheduler 专属 App Storage 写入一次性交接文件；成功后自动删除。PGV 未安装或不可达时保留初始化状态，不回退到 VOS 平台数据库。业务应用和 Agent 使用独立服务身份。

VOS 侧导航只有一个 `v-scheduler` 页面；资源任务和 API 文档使用应用内部导航。

VOS 登录接入清单必须声明 `oauth2.client: {}`，与 V-Router 相同；该声明使平台注册客户端并提供已有登录桥接接口。界面不提供单独的令牌登录表单。

Agent 允许空执行器配置：这种模式只上报节点资源，不认领任务，适合先纳管节点、再逐步接入业务。VOS 页面入口带安装版本查询参数，使升级后的 iframe 使用新页面。

Agent 是本应用在节点上运行的资源采集与任务执行后台进程，不是用户需要选择或命名的应用。资源页优先显示 VOS 集群登记名称；未接集群清单时使用该节点 VOS 的 `device_name`，再退回 VOS 上报的主机名。Agent 的 `node_id` 只用于内部协议关联，不作为节点显示名称。新节点接入仍由管理员部署后台进程和批准资源预算，普通用户无需配置 Agent。
