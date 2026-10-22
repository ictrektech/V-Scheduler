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

`0.0.22` 至 `0.0.27` 的调度交接使用正式 `http://v-scheduler:8080/scheduler/api/v1`；`0.0.21` 保留原 `v-router` 固定兼容契约。两者仍是同一个统一容器，不共享其他应用的身份。

## 资源批准

[内置校准清单](../config/voice-calibrations.json) 只包含经过实测的固定预算。
匹配项包括架构、实际 GPU 名称及内存类型、provider、CPU 模型覆盖、线程数与并发设置。
存在 `application_versions` 时，还须匹配控制器从 VOS 安装详情核验的应用版本；缺少或
不匹配时不采用该记录，不能用客户端自报版本代替。修改 provider 选择或内存回收实现后，
旧版本的测量不会自动授权新运行时。CPU 覆盖配置也须核对实际原生工厂使用的 provider。
192 的 `0.0.20` 混合执行记录预约 2.25 GiB RAM 和 GPU 槽；CUDA 分配不完全计入
容器内存，不能仅凭 cgroup 硬限制调低预约。修复 CPU provider 与卸载回收后的
`0.0.21` 记录仅覆盖 Kokoro 和 SenseVoice，使用 1.75 GiB RAM、4.5 CPU、GPU 0，
要求 `CPU_MODELS` 为 `kokoro-multilang,paraformer-zh,sensevoice-int8`，但不批准未实测的
Paraformer。`0.0.22` 仅迁移安装地址，复用已核验的 `0.0.21` 六个原生镜像及匹配的校准，不能据此扩大模型或硬件范围。
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
异步生成峰值 3768 MiB，并验证了运行中取消。创作模型的同步兼容入口及编曲
尚未获得这条语音审批授权；目录可见或文件已安装不会改变这一点。

节点遥测、执行器健康与资源余量仍由原有准入逻辑判断。发现成功、绑定成功均不代表
当前有空闲资源，也不代表所有目录模型已经获准运行。

原生音乐仍走 V-Voice 的同一组直接调用、网关许可认领和原生停止契约，但不能借用
语音模型的审批。声明为 `kind=music` 的同步模型要求审批中的 `music` 与 discovery 的
`execution_settings.music` 完全一致，并具备输入限制协议 1；控制器只在实际批准了音乐
模型时下发这组执行设置。线程、步数或其他音乐设置变化后，旧音乐审批不再匹配，
不影响仍匹配的语音模型。

每个音乐模型的 `model_limits` 必须完整包含 `duration_seconds`、`prompt_characters`、
`negative_prompt_characters`；前两项为正，负提示允许 0。网关同时使用该模型实测的时长
上限，V-Voice 在计算前核验三项边界。未完成真实权重、候选镜像、输入范围及原生停止
校准的模型不添加默认审批；目录可见或 adapter 导入成功不能替代真实生成与取消验收。

### Thor 原生音乐审批

应用版本 `0.0.21`、aarch64 NVIDIA Thor 统一内存、CUDA 的四项独立审批如下。每项均实测 30 秒、512 字符提示、正常生成和模型加载后的取消；Stable Audio 负提示上限 256 字符，其余为 0。每次只预约一个执行器，CPU 4500m、GPU 槽 1；统一内存不再重复预约独立显存。

| 模型 | 观测内存峰值 MiB | 预约内存 MiB |
|---|---:|---:|
| MusicGen Small | 4985 | 6400 |
| Stable Audio 3 Small Music | 5269 | 6656 |
| Stable Audio 3 Medium | 11255 | 14080 |
| ACE-Step 1.5 Turbo | 9930 | 12544 |

运行设置固定为 PyTorch 音乐运行时 v1、线程 4、interop 1、并发作业 1、常驻模型 1、步数 8。峰值取同一统一内存池的 cgroup、宿主可用内存下降和原生 CUDA allocator 观测最大值，预算增加至少 1 GiB 或 25% 余量后向上取整。审批不适用于其他架构、应用版本或运行设置；信号检查确认采样、时长、有限数值及非静音，不表示已做主观音质评估。

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

同一绑定同时包含新输入边界和旧兼容审批时，客户端必须额外声明 `mixed_input_limits_version=1`。Scheduler 仅把确实来自旧审批的模型列入 `binding.legacy_unbounded_models`，并同时下发混合协议版本；音乐模型始终要求完整的 `model_limits`。未声明该能力的客户端保持原绑定，不能发送只有部分模型限制的混合配置。客户端必须显式校验旧模型列表与批准模型的包含关系、重复和交叉项，不能将缺失限制解释成自动豁免。

| 模型 | 已测上限 |
|---|---|
| `kokoro-multilang` | 512 字符；速度固定 1.0 |
| `sensevoice-int8` | 20 秒；源采样率最多 24000 Hz；无 hotwords；输入最多 1 MiB |

192 的候选预算为 4500m CPU、4 GiB 系统内存及独占 GPU 槽，统一内存不另计显存。232 当前实际 provider 为 CPU，保守预算 24 CPU、4 GiB RAM、无 GPU 槽，覆盖观测到的进程启动峰值约 19.33 核及后续原生调用。内核不提供 cgroup `memory.peak` 时，清单明确区分采样 `memory.current`、宿主可用内存变化及进程高水位。

Voice 绑定使用管理 API 当前保存的节点预算，不能继续按初始配置文件中的旧预算判断。上述早期混合执行候选不等于当前 CPU 记录：192 的回收后模板使用 4500m CPU、1792 MiB RAM、GPU 0；232 使用 24000m CPU、4096 MiB RAM、GPU 0。`0.0.22` 在两台机器均完成直调与网关的真实 TTS / ASR、512 字符 TTS、20 秒 ASR及越界拒绝，许可全部释放；Thor 同步 TTS / ASR 复测通过。完整范围和原始校准的区分见[发布验收记录](validation.md)。

## 准入响应丢失时的未认领许可恢复

已验证的 V-Voice `0.0.21` / `0.0.22` Python 直调在加载或运行模型前，必须成功原子认领许可。若准入请求超时且没有取得回执，控制器可能已经保存预约；不能把这种预约永久保留为未知计算。

自动绑定协调器仅回收匹配当前可信安装与校准绑定的 Python `model.infer` 直调许可：应用、模型、执行器、节点、固定上游及执行令牌哈希全部匹配，租约已过期，且数据库从未写入 `execution_id`。关联任务、委派资源、跨节点 placement、异步任务、Router 许可、未知元数据和其他应用均不适用。当前恢复规则只对上述已核验版本启用。

回收和执行认领使用同一 PostgreSQL 许可行的原子状态检查。认领先提交则保留预约，即使认领响应丢失也不会自动释放；回收先完成则迟到的认领失败，不能启动推理。证据为 `vvoice_expired_without_execution_claim`，表示从未越过执行门禁，不表示已启动的原生计算被强制停止。已经认领或停止状态未知的计算继续保留预算，仍须真实停止证明。

## Thor 小模型共享内存预算（2026-10-21）

原通用 Thor 审批同时覆盖原生语音与 VoxCPM2 等创作模型，曾为所有同步模型预约 8 GiB 主存加 4 GiB GPU 内存。现将 SenseVoice INT8、原生 Kokoro 和 CAM++ 独立成组，预约 4500m CPU、4096 MiB 统一内存及一个 GPU 并发计数；`gpu_memory_mib=0` 表示 CUDA 已包括在这份共享内存预算内，不表示 CPU 推理。Whisper Tiny、Qwen3-ASR 和创作模型保留原预算。

两次隔离 Thor/CUDA 测量各执行两轮 512 字符 TTS、20 秒 ASR 及 ASR 与声纹组合：cgroup 峰值 1594/1587 MiB，宿主可用内存下降 2659/2705 MiB，峰值取两种观察的最大值。原生容器受 4096 MiB 内存及 swap 上限约束，测量由真实 8192 MiB 外层许可覆盖；确认卸载、容器退出后才释放许可。证据 ID 为 `voice-tc229-46bbfb08294441009daa7251dbdfd271`、`voice-tc229-d3e508b9dcc043dbb67f3c07dd7b9feb`。保留旧 Thor 输入契约；以上证明这些测试输入，不声称穷举所有可能的输入长度。

拆分预算后，跨组的 ASR 加声纹调用仍须有独立的联合校准，峰值不得超过主执行器预算；单独校准两个模型不能自动批准组合。192 Orin 的 1.75 GiB CPU 校准保持不变，不把它当作 Thor CUDA 测量。
