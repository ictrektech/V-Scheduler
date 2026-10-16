# vLLM 主动注册

该指南已合并到同一仓库的 [V-Scheduler 统一接入指南](../../docs/application-integration.md#vllm-registration)。V-Router 作为统一应用的网关组件运行，无需为接入另装独立 V-Router。

统一指南包含网关地址与鉴权、模型 API 探测和显式注册、public/private、稳定 service_id、多副本、心跳、重注册与注销，以及是否启用 Scheduler 调度的完整配置。

- [启用同步/流式计算调度](../../docs/application-integration.md#scheduled-vllm)
- [仅路由外部独立资源](../../docs/application-integration.md#external-vllm)
- [同步许可与异步业务任务](../../docs/application-integration.md#application-contract)
- [上线验收与当前能力边界](../../docs/application-integration.md#acceptance)

旧安装命令和历史版本验证可从 Git 历史查看；新接入统一按上述指南执行。
