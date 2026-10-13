# V-Scheduler · 资源调度

面向 VOS 应用、其他服务、脚本和第三方 worker 的独立资源调度应用。统一管理耗时任务的排队、资源预约、执行许可、取消与故障恢复。

**当前状态：设计与 API 草案阶段。尚无可运行服务、已发布镜像或 VOS 安装包；文档中的接口均未上线。**

- 应用名称：V-Scheduler；中文显示名：资源调度。
- 拟定 VOS app ID：`com.ictrek.v-scheduler`。
- 仓库：`git@github.com:ictrektech/V-Scheduler.git`。
- VAI 子模块路径：`apps/V-Scheduler`。

## 设计与接入标准

- [详细设计](docs/design.md)：任务协议、资源预约、公平排队、故障恢复、分阶段实施。
- [公开 API 与接入标准](docs/integration-standard.md)：VOS/非 VOS 客户端、服务身份、提交与 worker 接入、兼容性验收。
- [OpenAPI v1 草案](api/openapi-v1.json)：机器可读 HTTP 接口，状态为 draft。
- [现有架构证据](docs/architecture-evidence.md)：只读核验的 VOS 能力及限制。

## 维护与部署范围

Scheduler、UI、Agent、协议及打包均由本仓库维护。VOS 平台和 vos-cluster-manager 代码不需要修改。

VOS 上计划通过独立应用包安装 controller/node 模式，再经现有 projects 接口部署本应用自己的 `resource-agent` 子项目。外部环境使用独立容器或 Agent 程序，经同一公开协议接入，无需 VOS 安装。

应用保留原业务入口，按任务类型选择接入：只提交任务，或注册 worker 提供执行能力。普通请求无需经过调度器；未接入重任务需要固定预算/设备隔离，否则不能保证整机资源不被抢占。

尚未创建运行实现，当前没有启动、构建或发布命令。实施顺序和验收条件见详细设计；不得把接口草案当成已支持功能。
