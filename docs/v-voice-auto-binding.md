# V-Voice 主动接入

统一包内的 Go 网关复用 V-Voice 发现器，将同一容器网络中的标准服务声明和实际就绪
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
模型自报的权重大小不授予资源；声明支持但文件未就绪的模型也不会建立绑定。
同步模型使用原生 ID，创作兼容 API 的 ID 带 `audiocpp-`，异步创作使用裸 ID。

管理员可在控制器 `/config/voice-approvals.json` 保存本节点的完整校准清单以覆盖内置值，
文件必须为普通文件、0600，最大 256 KiB。它不包含服务令牌，字段格式与内置清单一致。
校准应在独立测试副本中，通过本机 Scheduler 的独立受限身份和真实资源账本执行，
同时约束 CPU、内存及输入范围；记录装载、推理、联合模型、取消及停止后的峰值与证据。
未校准的硬件、运行设置或模型继续拒绝，不从默认预算静默放行。

当前内置清单覆盖 NVIDIA Thor 统一内存、CUDA、4 线程与推理并发设置 2：
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
