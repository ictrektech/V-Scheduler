# vLLM 与外部算力接入

操作步骤已合并到 [V-Scheduler 统一接入指南](application-integration.md)。

- [模型主动注册、public/private、心跳与注销](application-integration.md#vllm-registration)
- [交由 Scheduler 调度](application-integration.md#scheduled-vllm)
- [只路由外部独立资源](application-integration.md#external-vllm)
- [通过 VOS App 管理远端节点](application-integration.md#remote-nodes)
- [上线验收](application-integration.md#acceptance)

注册不等于纳管算力；远端 URL 和 public 可见性不自动豁免调度。本文保留旧文档地址，后续只维护统一指南中的操作步骤。
