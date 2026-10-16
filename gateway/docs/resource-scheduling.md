> 迁移说明：网关现由 V-Scheduler 统一发布，源码位于 `gateway/`。当前安装、调度和端口以[统一应用](../../docs/unified-application.md)、[应用接入](../../docs/application-integration.md)及[端口表](../../README.md#host-alias内部端口与宿主机端口)为准。本文保留网关协议及历史验证细节；独立 V-Router 的安装命令和版本记录属于旧部署。

# V-Router 网关资源准入

VOS 安装包、统一界面及接入文档现由 V-Scheduler 维护。V-Router 在其中继续作为 Go 网关组件运行，保留原模型 API、自动发现和协议适配；不再要求另装独立 V-Router。

请使用新版文档：

- [统一应用、地址与保留数据迁移](https://github.com/ictrektech/V-Scheduler/blob/main/docs/unified-application.md)
- [网关资源准入指南](https://github.com/ictrektech/V-Scheduler/blob/main/docs/resource-scheduling.md)：逐模型预算、同步许可、异步交接、停止核验与重启恢复。
- [应用接入指南](https://github.com/ictrektech/V-Scheduler/blob/main/docs/application-integration.md)、[接入标准](https://github.com/ictrektech/V-Scheduler/blob/main/docs/integration-standard.md)及 [OpenAPI](https://github.com/ictrektech/V-Scheduler/blob/main/api/openapi-v1.json)。

旧网关地址和单独运行的网关后端继续兼容；独立开发进程未开启强制准入不代表生产计算已受调度。升级安装前须先卸载旧 V-Router 并保留数据，不能让两个网关同时写同一目录或占用 `22888`。
