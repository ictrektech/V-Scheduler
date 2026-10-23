# 兼容应用接入与发布验证（2026-10-23）

本轮验证实际安装的 VOS 应用、原生执行进程和资源账本。V-Music 已退役，不参与适配、发布或安装。以下“通过”仅覆盖列明的场景，不代表所有应用页面与全部模型均已验证。

## 发布与安装

| 应用 | 发布版本 | 192 | 229 | 232 |
| --- | --- | --- | --- | --- |
| V-Scheduler | 0.0.73 | l4t | thor-spark | amd |
| V-Voice | 0.0.29 | l4t | thor | amd |
| V-Sub | 0.0.14 | l4t | thor | amd-cuda |
| Desensitize | 0.0.53 | l4t | thor-spark | amd |
| Image Studio | 0.0.76 | l4t | thor | amd-cuda |
| HybRAG | 0.1.75 | arm | arm | amd |
| V-ChatCut | 0.0.30 | l4t | thor-spark | amd-with-cuda |
| V-Harness | 0.1.9 | 未安装，不新增安装 | arm | amd |

升级使用 VOS 应用升级接口，保留原 profile、配置和公共模型目录。镜像发布覆盖受影响的全部平台；未修改的后端基础镜像继续复用。三台均核验了安装版本、运行镜像 ID/digest，以及 V-Voice、V-Sub、Desensitize 和 Image Studio 的运行源码 SHA-256 与交付源码一致。

发布记录：[V-Scheduler](https://github.com/ictrektech/V-Scheduler/actions/runs/39893006195)、[V-Voice](https://github.com/ictrektech/V-yovoice/actions/runs/39900010952)、[V-Sub](https://github.com/ictrektech/V-Sub/actions/runs/39897320231)、[Desensitize](https://github.com/ictrektech/desensitize/actions/runs/39897272544)、[Image Studio](https://github.com/ictrektech/image-studio/actions/runs/39897809150)。均已完成 GitHub Release 与 VOS App Store 发布。

## 本轮行为变化

- V-Voice、Desensitize、V-Sub 在取得本地执行锁之前进入控制器统一准入队列，默认等待 30 秒。请求可携带任务优先级、协作抢占意图和等待时限；等待期间不预约权重、不启动或重放推理。
- 同一已批准原生进程使用一条执行通道。Ollama 同一后端的已驻留模型复用完整权重预约，不能把每个请求都算作新加载一份权重。
- 其他应用申请预算不再无条件卸载空闲 Ollama 模型。只有确实需要资源时才请求卸载，确认原生进程停止后释放预算。
- Image Studio 的短期 lease 心跳采用原子替换，避免磁盘持久化延迟阻塞续期；执行状态和原生停止回执仍持久化。非用户取消造成的 lease 失效标为可重试故障，重试仍受停止证明和 attempt fence 约束。

## 实际功能验证

| 场景 | 192 | 229 | 232 |
| --- | --- | --- | --- |
| V-Voice：真实 WAV 合成与非空语音识别，直连及统一网关 | 通过 | 通过 | 通过 |
| Desensitize：NER 实际替换、OCR 实际脱敏 | 通过 | 通过 | 通过 |
| Image Studio：原生生成有效 PNG，取消后确认 sd-cli 退出 | 通过 | 通过 | 通过 |
| Image Studio：请求 high 优先级实际进入调度任务，取消后释放 | 通过 | 通过 | 通过 |
| V-Sub：已校准 CPU Whisper，直连和网关识别 | 通过 | 通过 | 通过 |
| V-Sub：超限输入及未校准参数拒绝、取消/统一停止、worker 重启后重新绑定 | 通过 | 通过 | 通过 |
| 四个执行应用：自动注册、持续心跳、节点遥测、预算匹配 | 通过 | 通过 | 通过 |
| V-Voice / Desensitize：停止意图不提前释放，确认原生退出与卸载后释放，再次调用正常 | 通过 | 通过 | 通过 |
| HybRAG：新建独立文档、摘要、向量检索、重排、事实问答与清理 | 低内存组合通过 | 通过 | 通过 |
| V-ChatCut：真实登录、模型列表、普通/SSE 回答、共享驻留及空闲卸载释放 | 通过 | 通过 | 通过 |
| HybRAG：关闭模型放行后，QA / embedding / rerank 各生成并释放一份实际许可 | 完整 RAG 中验证 | 完整 RAG 中验证 | 通过 |
| V-Harness：实际租户代理、普通/SSE 回答、错误身份拒绝及管理面隔离 | 未安装 | 通过 | 通过 |

V-Sub 的验证使用实际已批准的 CPU 参数，临时切换测试设置后恢复原设置；不能据此宣称任意 GPU 参数都获准。改变运行后端、线程、VAD 或输入上限仍须匹配实测校准。

192 的完整 RAG 使用共享目录中的 Qwen3.5 0.8B、已校准的小型 embedding 和 BGE reranker。0.8B 在 CPU、8K 上下文下实测整机可用内存峰值下降约 2322 MiB，预算为 2816 MiB；准入保留至少 1024 MiB 主机余量，关闭相关模型放行后，完整 RAG 产生 7 次实际许可，均已完成或核验为空闲驻留，无 suspect 许可，原生计算空闲；成功问答后仍有约 2913 MiB 可用。临时知识库、会话及模型接入记录已清理，下载权重留在共享目录；用户原有模型默认项未替换。2B 模型的独立恢复调用也已通过，不能将此等同于任意内存压力下都应放行。

232 关闭三个模型放行后的独立 QA、embedding、rerank 实际调用已通过，各产生并释放一份许可。随后完整新文档上传、解析、摘要、向量检索、重排和事实问答也通过，产生 3 次实际调度许可，均已完成或核验为空闲驻留；测试知识库与会话已删除，原放行设置已恢复。收尾检查确认无 suspect 许可、三个原生后端计算槽空闲、网关等待队列为空，四个执行应用心跳新鲜且无遗留执行任务。

232 先前解析停在 pending 的原因是另一非 VOS 测试实例连接了正式应用相同的 Redis 地址、数据库 0 和认证配置，却使用另一套业务数据库，抢走了正式应用的解析任务。经用户授权，连续三次确认任务队列无运行任务、测试服务无 HTTP 连接及近期请求后，已停止并清除该测试容器；共享镜像、模型和正式应用数据保留。清理后 Redis 仅有正式 HybRAG 的 6 个在线 worker，上述完整链路复测通过。独立测试实例应使用独立的 Redis 和业务数据库连接，避免复制正式队列配置。

## 契约与回归检查

三台均确认正式 alias `v-scheduler` 与兼容 alias 共存，三个统一服务内部端口与宿主机映射保留；应用“关于”返回实际安装版本及四个镜像的 `名字:tag`。控制器 discovery 与 OpenAPI 均公布 `permits/wait` 和统一生命周期能力。验证实际调度时临时关闭三个测试模型原有的“放行”选项，结束后恢复操作者原设置，避免将免调度调用误算为资源准入成功。完整接入流程见 [统一接入指南](../application-integration.md)，应用内“应用接入”同步说明该契约。

控制器与前端回归包括 ARM 130 项（另 1 项忽略）、AMD 128 项（另 1 项忽略）、前端 25 项及构建；独立中央队列场景 11 项、共享驻留场景 53 项、完整调度回归 49 项。应用专项检查包括 V-Sub 26 项、Desensitize 22 项及 6 个子场景、V-Voice 修正后的 38 项与发布 CI 全量检查、Image Studio 15 项及 3 个子场景。Image Studio 的磁盘延迟 lease 失效场景先复现失败、修正后通过。
