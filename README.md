# V-Scheduler · 资源调度

面向 VOS 应用、其他服务、脚本和第三方 worker 的独立资源调度应用。统一管理耗时任务的排队、资源预约、执行许可、取消与故障恢复。

**当前状态：Rust controller / Agent、Vue 管理界面及 Image Studio / V-Router 适配已完成最小版本，并通过独立环境验证。正式安装包见 [GitHub Releases](https://github.com/ictrektech/V-Scheduler/releases)。首次安装需要预先准备专用 PostgreSQL 数据库、受保护的配置与节点 Agent；目标 VOS 的只读认证和多节点部署仍待验证。完整设计的后续能力不代表当前已支持。**

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

服务默认监听 `127.0.0.1:38120`；公开 API 位于 `/api/v1`，公开发现 `/api/v1/discovery`，机器规范 `/api/v1/openapi.json`。VOS 内界面复用当前平台登录，由后端通过 `/v1000/user/check` 校验启用的 `system_admin` 身份；不要求用户填写 Scheduler 令牌。独立部署仍支持服务令牌。节点任务排布显示执行、等待顺序、近期结束任务，并提供取消、受控重试和删除；连接前可打开清楚标注的演示模式，演示操作只修改页面样例。默认跟随 VOS 语言，支持简中/英文/繁中，手动选择保存于应用自己的存储。

数据库迁移只作用于专用数据库。节点/执行器/凭据由 controller 配置批准；第一版每节点一条执行通道、FIFO 排队，资源预约和 V-Router 本地在线许可共享预算。云端请求不申请本地许可。VOS 节点发现/应用清单见节点资源文档。

失败自动重试只针对已确认停止、可重试错误、声明幂等的执行器；`max_attempts` 为总尝试上限（1–10），退避 3/6/12/24/48 秒。人工重试也受上限约束。失联先进入 recovering/suspect，停止核验前不释放。人工删除只软删除已停止终态记录，保留审计、幂等占位与业务输出。

目前不支持动态 worker/执行器注册、OIDC 管理登录、HA、跨节点数据复制、多 GPU 设备级分配或任意远程命令。现有测试以 tc232 上的独立服务与真实 CUDA Image Studio 为范围；尚未完成目标 VOS 版本的只读认证和多节点部署验证。镜像及安装包发布另按仓库既有流程执行。

`scripts/e2e.mjs` 是获授权 tc232 测试环境的故障注入工具，使用独立端口/容器/数据目录；需要已安装 Image Studio 镜像和只读模型挂载。构建及测试遵循 VAI 指定的远程构建规则。

镜像构建、版本脚本和 VOS 包流程见 [打包与发布](ictrek.app/README.md)。

已完成的检查及部署边界见 [MVP 验证记录](docs/validation.md)。

首次安装不再要求已有 `database.env` 才能创建容器：配置缺失时先提供初始化界面，VOS 管理员身份通过校验后为现有 PGV 创建专用数据库和随机普通角色，再启动调度控制器。PGV 未安装、不健康或凭据无法发现时会明确失败，保持初始化界面，不回滚安装或改用 VOS 平台数据库。PGV 自定义凭据未被 VOS 保存的旧实例，需要运维在受保护的 App Storage `config/pgv-bootstrap.json` 中交接一次 PGV 凭据（键 `POSTGRES_SUPERUSER`、`POSTGRES_PASSWORD`、`POSTGRES_DB`）；初始化成功后删除该文件。业务应用和 Agent 仍使用独立服务身份，不共享 VOS 用户或数据库管理凭据。

VOS 侧导航只有一个 `v-scheduler` 页面；资源任务和 API 文档使用应用内部导航。

VOS 登录接入清单必须声明 `oauth2.client: {}`，与 V-Router 相同；该声明使平台注册客户端并提供已有登录桥接接口。界面不提供单独的令牌登录表单。
