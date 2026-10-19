# V-Voice 主动接入

统一包内的 Rust 网关复用 V-Voice 发现器，将同一容器网络中的标准服务声明和实际就绪
模型写入网关私有数据目录。Rust 控制器核验 VOS 签发给
`com.ictrek.v-scheduler` 的应用 context 身份，并确认 `com.ictrek.v-voice`
在本 VOS 已安装。授权只发给固定的 `http://v-voice-backend:7070`，不向发现结果中的
任意 URL 发送凭据，也不使用浏览器令牌。

控制器为 V-Voice 建立自己的 submitter、同步 `model.infer` 和异步
`voice.generate` 执行器；网关使用另一独立 submitter。V-Voice 的
`permit_executors` 仅允许核验相应网关执行器。网关转发的同步调用复用同一笔预算，
V-Voice 直接调用则申请自己的许可。ASR 与声纹组合必须落在同一个已批准联合预算中。

## 资源批准

[内置校准清单](../config/voice-calibrations.json) 只包含经过实测的固定预算。
匹配项包括架构、实际 GPU 名称及内存类型、provider、CPU 模型覆盖、线程数与并发设置。
存在 `application_versions` 时，还须匹配控制器从 VOS 安装详情核验的应用版本；缺少或
不匹配时不采用该记录，不能用客户端自报版本代替。修改 provider 选择或内存回收实现后，
旧版本的测量不会自动授权新运行时。CPU 覆盖配置也须核对实际原生工厂使用的 provider。
192 的 `0.0.20` 混合执行记录预约 2.25 GiB RAM 和 GPU 槽；CUDA 分配不完全计入
容器内存，不能仅凭 cgroup 硬限制调低预约。修复 CPU provider 与卸载回收后的
`0.0.21` 记录仅覆盖 Kokoro 和 SenseVoice，使用 1.5 GiB RAM、4.5 CPU、GPU 0，
要求 `CPU_MODELS` 为 `kokoro-multilang,paraformer-zh,sensevoice-int8`，但不批准未实测的
Paraformer。该记录不等于 Voice 新版本已经发布或已在当前 VOS 安装。
模型自报的权重大小不授予资源；声明支持但文件未就绪的模型也不会建立绑定。
同步模型使用原生 ID，创作兼容 API 的 ID 带 `audiocpp-`，异步创作使用裸 ID。

管理员可在控制器 `/config/voice-approvals.json` 保存本节点的完整校准清单以覆盖内置值，
文件必须为普通文件、0600，最大 256 KiB。它不包含服务令牌，字段格式与内置清单一致。
校准应在独立测试副本中，通过本机 Scheduler 的独立受限身份和真实资源账本执行，
同时约束 CPU、内存及输入范围；记录装载、推理、联合模型、取消及停止后的峰值与证据。
未校准的硬件、运行设置或模型继续拒绝，不从默认预算静默放行。

已发布 `0.0.26` 的清单覆盖 NVIDIA Thor 统一内存、CUDA、4 线程与推理并发设置 2：
同步包括 SenseVoice INT8、Whisper Tiny、Qwen3-ASR 0.6B、CAM++ 声纹和原生 Kokoro；
异步包括 Kokoro Q8 与 VoxCPM2 Q8。每次执行预约 4 CPU、8 GiB 主存、1 个 GPU 槽位
及 4 GiB GPU 预算。校准副本受 4 CPU/8 GiB 容器限制，原生联合调用峰值 2419 MiB，
异步生成峰值 3768 MiB，并验证了运行中取消。创作模型的同步兼容入口、音乐及编曲
尚未获得本清单授权；目录可见或文件已安装不会改变这一点。

节点遥测、执行器健康与资源余量仍由原有准入逻辑判断。发现成功、绑定成功均不代表
当前有空闲资源，也不代表所有目录模型已经获准运行。

## 持久化与重试

- 控制器 `/config/voice-binding.json` 保存自己的受限凭据、执行器和 revision。
- 网关 `/data/voice-scheduler.local.json` 只保存网关自己的身份及逐模型策略，每次请求
  读取原子快照；原有其他上游的 `scheduler.local.json` 保持兼容。
- V-Voice 通过已有 `POST /api/v1/scheduler/binding` 接收自己的身份，先核验执行器再
  原子写入本应用的私有文件。浏览器和 discovery 均不会取得服务令牌。
- 安装、服务重启、模型就绪变化和 context 凭据轮换后定期重试。同内容保留同一 revision，
  内容变化递增，重启不重置。内置 Agent 热更新执行器，继续使用原来的节点身份、
  会话和执行日志；`start/inspect/cancel` 协议不变。
- 提高校准清单顶层 `credential_epoch` 可轮换受限凭据；不允许倒退。存在活动或未知
  许可、资源预约时等待，保留旧凭据以便续租、核验和停止。切换期间暂停新的准入；
  不因超时或配置轮换释放未知计算的预算。

这些都是统一应用内部的可信应用适配，并未增加通用的公开动态注册接口。

## 验收

需要在真实节点分别验证 V-Voice 界面、原生兼容 API、网关调用及异步创作任务：
每次同步推理恰好一笔许可，完成且确认停止后释放；异步任务通过 Agent
`start/inspect/cancel`，输出保留在发起用户的私有工作区。还需验证重试幂等、
凭据轮换、错误身份、未校准模型、忙碌资源、过期遥测及停止未知均无法绕过准入。
界面显示“已发现”不能替代这些调用证据。

## 原生模型输入边界

新清单增加 192 Orin / CUDA 与 232 RTX 5880 宿主 / CPU provider 的原生 Kokoro、SenseVoice 实测记录，其他模型或组合不能借用这些记录。

新记录要求 discovery 声明 `input_limits_version=1`，并为每个就绪且获批准的模型下发 `binding.model_limits`。旧客户端没有该能力时不匹配这些新记录；原有 Thor 兼容记录保留既有行为。新客户端在直接 API 与网关认领路径实际模型装载前校验边界。

| 模型 | 已测上限 |
|---|---|
| `kokoro-multilang` | 512 字符；速度固定 1.0 |
| `sensevoice-int8` | 20 秒；源采样率最多 24000 Hz；无 hotwords；输入最多 1 MiB |

192 的候选预算为 4500m CPU、4 GiB 系统内存及独占 GPU 槽，统一内存不另计显存。232 当前实际 provider 为 CPU，保守预算 24 CPU、4 GiB RAM、无 GPU 槽，覆盖观测到的进程启动峰值约 19.33 核及后续原生调用。内核不提供 cgroup `memory.peak` 时，清单明确区分采样 `memory.current`、宿主可用内存变化及进程高水位。

Voice 绑定使用管理 API 当前保存的节点预算，不能继续按初始配置文件中的旧预算判断。最终 L4T 客户端已完成真实 VOS 可信绑定及直调越界拒绝验证（`actual-binding/76ca4ed70ff045048b59aa8b759a7d52`），没有新增计算许可或装载模型。当前 192 可用内存不足以同时满足 4 GiB 预算和宿主保留量，正向调用保持容量拒绝；232 受原 Model Hub QA CPU 占用限制。Thor 最终客户端通过真实 TTS / ASR 直调与网关输出、四份许可完整释放（`actual-binding/b2f4b99d014748daac078a7d39668a1d`），该项使用保留的 Thor 兼容契约，不能代替 L4T / AMD 新输入模板的正向输出验收。
