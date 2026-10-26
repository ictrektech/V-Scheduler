# 固定应用的可信绑定

内置 `config/app-calibrations.json` 按目标应用 profile、硬件、模型、完整运行设置及输入边界记录批准预算。Thor Music / Image 已有真实直调及网关输出，Thor Desensitize 已有可信绑定后的真实业务 API 输出；AMD / L4T 的资源测量与各项验收边界列在下文。未校准组合不自动借用其他机器的预算。

控制器只为下列固定 VOS 应用接入，不提供允许调用者自报 URL 和预算的公开注册入口。原 V-Voice 专用绑定保持独立，见 [V-Voice 主动接入](v-voice-auto-binding.md)。

| 应用 | 固定执行服务 | 声明入口 | 任务类型 |
|---|---|---|---|
| `com.ictrek.v-music` | `http://v-music-backend:7080` | `/api/v1/discovery` | 同步 `model.infer` |
| `com.ictrek.desensitize` | `http://desensitize-backend:5000` | `/api/v1/discovery` | 同步 `model.infer` |
| `com.ictrek.image-studio` | `http://image-studio-backend:5000` | `/api/v1/v-router/registration` | 异步 `image.generate` / `video.generate` |
| `com.ictrek.v-yue2` | `http://v-yue2-backend:8791` | `/api/v1/scheduler/discovery` | 原生 `model.infer`；应用 broker 持有许可 |

## 信任与校准

控制器读取自己由 VOS 挂载的应用 context，向 VOS 核验 `sub=com.ictrek.v-scheduler`，再确认目标应用在同一 VOS 已安装。发现请求不携带服务令牌；只向表中固定服务的 `/api/v1/scheduler/binding` 下发凭据，禁止跟随 HTTP 重定向。浏览器、模型共享目录和 discovery 不包含这些凭据。

模型首次授权必须同时满足：文件和后端已就绪、声明的原生 ID 与任务类型匹配、目标应用实际安装 profile 匹配、节点实际架构与 GPU/内存类型匹配、`execution_settings` 完整对象与实测配置一致。硬件取自 Scheduler 遥测，profile 取自 VOS 安装记录；不信任应用自报预算。

校准还须保存实测峰值、已确认停止的证据、固定 CPU/RAM/GPU 资源、输入字节上限和 `model_limits`。缺少任意必要条件不建立相应执行器。内置清单由发行方实测后发布，普通用户不需要 SSH 或逐模型手动批准。管理员私有覆盖文件为控制器 `/config/app-approvals.json`，须为普通文件、0600、最大 256 KiB。

测试容器的资源上限必须与正式运行配置对应；不能把限制了 CPU/内存的测试峰值用于未限额、线程数不同的正式进程。模型权重、运行后端、线程、库版本、offload 或输入参数的变化须重新匹配校准。

## 交接字段与执行责任

绑定包含 `protocol_version=1`、单调递增 `binding_revision`、`app_id`、固定调度 `url=http://v-router:8080/scheduler/api/v1`、本应用的 `token` 与 `executor_token`、固定 `upstream_url`、`execution_settings`、`models`、`gateway_executors`、`model_limits` 及 Image Studio 的 `model_parameters`。客户端先核验 VOS 身份、固定地址和控制器执行器，再以 0600 原子落盘；服务凭据不得交给业务用户。

这里的 `v-router` 是上述三款已发布旧客户端的固定兼容契约，实际指向 `v-scheduler` 统一容器。新客户端推荐正式 `v-scheduler` 地址；这些旧客户端后续切换时须同步核验收发两端契约，不能只改文档或放宽地址白名单。

- Music 限额包含 `duration_seconds`、`prompt_characters`、`negative_prompt_characters`。
- Desensitize NER 包含 `text_characters`、`max_tokens`；OCR 包含 `input_bytes`、`input_width`、`input_height`、`input_pixels`、`input_pages`。解码前及实际推理前均检查。
- Image Studio 包含图像、批次、视频、hires 和文本范围；`model_parameters` 完整匹配实测 JobRecord 的 19 个运行参数及 `has_reference`，使用 `model@variant` 原生 ID。不能用默认参数的预算放行其他 offload、缓存、后端或参考图组合。

直接同步调用由应用取得、续租和释放自己的许可。经统一网关调用时，应用使用独立身份核验并认领网关许可，不重复预约。Image Studio 使用持久任务、outbox 和现有 `start/inspect/cancel` 执行适配；网关的异步桥接许可经控制器核验后移交。原生业务接口、输入文件、输出权限仍由业务应用管理，业务 API 不必全部改写成网关 API。

## 持久化与撤销

控制器 `/config/app-bindings/<app>.json` 保存私有绑定状态，网关 `/gateway-data/app-bindings/<app>.json` 的对应卷保存网关自己的受限身份。网关每次请求读取原子快照；执行器和服务身份持久化在资源账本。异步适配更新与 V-Voice 的本地适配配置合并，保留其他应用条目。

内容相同时保留 revision 和凭据。变更必须等待本应用活动任务、许可及停止未知的预约结束；提高 `credential_epoch` 才轮换凭据，并撤销旧身份。管理员已禁用的执行器和已撤销的身份不会被同一配置重新启用。运行配置不再匹配时，旧自动执行器退出可执行集合；即使客户端尚持旧快照，也不能据此取得执行授权。

[V-Sub 每用户 worker / Root broker](v-sub-auto-binding.md) 和 [HybRAG 后台网关凭据](hybrag-gateway-identity.md) 使用各自契约，不属于本固定三应用模块。协议测试与真实任务验收分别记录，见[发布验收记录](validation.md)。

## v-YuE2 原生 broker

v-YuE2 自己管理业务异步 job，Go broker 在启动账户的原生引擎前通过 `/permits/wait` 等待同一资源账本准入；无需再增加 task/Agent 适配或网关原生音乐接口。固定 origin 与 `upstream_url` 均为 `http://v-yue2-backend:8791`，不追加 `/v1`。声明使用 `scheduler.sync_models` 中的 `model.infer`，完整运行参数放入 `execution_settings`。

控制器只向安装在同一 VOS 的固定接收端下发单一独立 submitter 身份、批准 executor 与输入边界。该应用的正式调度地址为 `http://v-scheduler:8080/scheduler/api/v1`；不创建额外 gateway identity、网关模型策略或 Agent adapter，不下发 `executor_token`。供应商模型仍从统一网关目录选择；外部独立算力不占用本地音乐的预算。

每个批准组合必须精确匹配硬件、实际安装 profile、模型及完整线程/引擎设置。`model_limits` 必须包含正数 `duration_seconds`、`steps`、`prompt_characters`、`lyrics_characters`；独立显存预算还须有正数 `evidence.peak_gpu_memory_mib`，且不能超过预约。缺少实测、原生停止证明、输入上限或就绪模型时，不创建可执行授权。单纯准备模型不授予计算权限，未校准组合继续拒绝。

运行时 broker 先认领 permit，再启动本地工作；续租和核验失败必须停止进程组并等待原生退出。正常完成先卸载再 release；收到 stop nonce 必须提交匹配的原生停止证明，结果未知不释放预约。原生计算的优先级、等待、抢占授权与其他应用共用，不能凭 VOS 管理员身份直接跳过准入。

AMD `yue2-q8` 已完成同账本排队、10 秒及最大 120 秒 / 32 步 full planner 生成、应用取消和 Scheduler stop nonce 实测。仅批准 `amd` / x86_64 / NVIDIA RTX 5880 Ada Generation，预算为 8 CPU、8 GiB RAM、1 GPU 槽和 6 GiB 独立显存；最大请求 65536 字节、风格 1024 Unicode 字符、歌词 4096 字符。Q8 主干、F32 VAE、companion-v9、引擎/模型清单哈希、8 线程及专用 native-parent 线程保护全部固定匹配。其他平台、量化、运行设置及输入仍须独立校准。

内核不提供 `memory.peak`：以约 200ms 的 `memory.current` 采样最大值 2103 MiB 加宿主可用内存下降 1761 MiB 保守计量（可能重叠），GPU 采样峰值 3934 MiB；预算包含余量。原生进程组退出、显存归零后才释放许可；这些隔离校准证据仍不能替代正式包安装后的可信自动绑定验收。

## 当前实测清单

| 目标应用 VOS profile / 硬件 | 模型 | CPU / RAM | GPU | 已测输入范围 |
|---|---|---|---|---|
| `thor` / NVIDIA Thor 统一内存 | MusicGen-small | 4 核 / 12 GiB | 独占 1 槽；内存计入系统 RAM | 最长 10 秒，prompt / negative 各 512 字符 |
| `thor` / NVIDIA Thor 统一内存 | SD-Turbo | 4 核 / 8 GiB | 独占 1 槽；内存计入系统 RAM | 512×512、1 张、最多 4 步，prompt / negative 各 4000 字符；其余运行参数完整匹配 |
| `amd-cuda` / NVIDIA RTX 5880 Ada 独立显存 | MusicGen-small | 4 核 / 8 GiB | 独占 1 槽 / 3 GiB 显存 | 最长 10 秒，prompt / negative 各 512 字符 |
| `amd` / NVIDIA RTX 5880 Ada 独立显存 | Desensitize NER / OCR | 每次 4 核 / 4 GiB | 独占 1 槽 / 2 GiB 显存 | NER 最多 4096 字符、512 tokens；OCR 最多 4 MiB、1024×1024、单页 |
| `thor-spark` / NVIDIA Thor 统一内存 | Desensitize NER / OCR | 每次 4 核 / 6 GiB | 独占 1 槽；内存计入系统 RAM | 同上，完整匹配 ORT 1.27.0 等实际运行设置 |
| `l4t` / Orin 统一内存 | Desensitize NER / OCR | 每次 4 核 / 5 GiB | 独占 1 槽；内存计入系统 RAM | 同上，完整匹配 ORT 1.23.0 等实际运行设置 |

Image 使用实际 `--threads 4` 配置；其他线程或 offload 设置不能借用该预算。Music 使用实际 Torch 4/1 线程、单任务、单加载模型。资源值包含实测余量，不是模型文件大小。

229 节点经用户授权将总内存规划从 16 GiB 调到 20 GiB，保留原驻留基线 6 GiB；这使 12 GiB Music 预算可以在规划内被预约。192 经用户授权只将 CPU 规划从 4 核调到 4.5 核，覆盖 Whisper/Electron 的实测开销，内存规划未变。调整均通过现有带版本校验、节点空闲保护的管理 API 保存并回读，原规划已备份；物理空闲内存与安全余量检查继续生效。

这些是隔离真实原生任务的预算证据。正式绑定、网关调用、客户端安装和逐机验收仍须通过后才发布；192 的 Music 仍因实际内存不足待验收，不能据规划调整宣称通过。

AMD Music 记录进程 `ru_maxrss` 内核高水位、NVML 按进程显存采样及宿主内存变化；该机内核不提供 cgroup `memory.peak`，所以 `memory.current` 的采样最大值不标为精确峰值。批准预算覆盖已测值并留有余量，原始测量方法保留在清单证据中。

校准匹配目标应用自己的安装 profile，不能直接套用 Scheduler 的同平台名称：Music / Image 的 CUDA profile 为 `amd-cuda` / `arm-cuda` / `thor`，Desensitize 则为 `amd` / `arm` / `thor-spark`。目标 profile 从 VOS 安装记录读取，再与各自清单及真实运行设置一起精确匹配。

Image Studio 使用原生 `/api/v1` 适配，上游资源身份固定为 `http://image-studio-backend:5000` 根地址；不能附加不存在的 OpenAI `/v1`。Music / Desensitize 的 OpenAI 上游身份仍为各自根地址加 `/v1`。发现、绑定、网关 origin 和许可中的上游身份须一致。

模型下载/更新可能短暂从就绪发现中消失：仅保留该模型完全相同的既有校准执行器，不因瞬时缺项永久停用它；不会据此给尚未就绪的新模型或变更后的预算首次授权。运行设置、硬件、实际 profile、资源/输入校准改变时仍须匹配新批准，管理员已禁用的同 ID 也不会被重新启用。

系统自动停用与人工禁用使用现有审计表区分：仅存在本模块针对该执行器当前版本的 `executor.auto-retired` 记录、定义/节点完全相同且校准重新匹配时，才恢复系统自己停用的执行器。管理员 API（包括旧版）修改会推进版本号，使旧自动记录失效；人工禁用不被恢复。该机制不新增 schema、表或字段，回退旧版时仍是原有数据形状。

Thor 候选实例已通过可信 VOS 安装身份到真实客户端的隔离端到端验收：Music 直调和网关各生成真实 WAV，每次仅占一份许可，超出 10 秒先行拒绝；Image 直调和网关各生成 512×512 PNG，超出 4 步不创建任务，业务取消在观察到原生子进程后确认退出并释放预约。证据目录分别为 `actual-binding/246ce55a6a9b4c5dbb50f77e3def99a8` 和 `actual-binding/d031abd040f844ed936e71a4e98673cf`。这些结果不替代正式包升级后的逐机验收。

AMD Desensitize 记录 `des-local-0485474694a64047ab5e982d6f41d890` 实测 NER 4096 字符截断到真实 512 tokens，以及精确 4 MiB / 1024×1024 OCR 与小图，均有真实输出。NER ORT 线程固定 4/1，OCR 的 det/cls/rec 会话各固定 1/1、OpenCV 1、OMP/MKL/OpenBLAS 环境均为 1，完整库版本及运行设置须匹配。CPU 观测最大 1727 millicores，保守内存合计 3201 MiB（cgroup 采样 961 + 宿主变化 2240），NVML 观测 736 MiB；批准留有余量。旧默认线程达到约 82 核的失败记录不作为批准依据。OCR 网关请求字节上限为 6 MiB，以容纳 4 MiB 图像的 base64 JSON，实际解码后的图像仍受 4 MiB 限额。此次原生校准不替代最终客户端的可信自动绑定与正式安装验收。

Desensitize 的 ARM 预算分别来自 `des-tc229-b0acd2ad3f4f495fb81e6c9fa6f608be`（CPU 1179 millicores，保守内存 4371 MiB）和 `des-tc192-5aa1ed15f71b4460a4d698574f22dc36`（CPU 1003 millicores，保守内存 4091 MiB），均完成最大输入、真实 CUDA provider / 线程核对及卸载退出。统一内存计入 RAM，不将进程显存采样当作独立 VRAM 预算。L4T 未把接近 4 GiB 的观测值批准为无余量的 4 GiB；使用 5 GiB 后仍须通过实际物理空闲内存及安全余量检查，资源不足时保持等待。原生测量通过不代表最终业务 API / 自动绑定已验收。

Thor Desensitize 真实可信绑定业务验收 `actual-binding/7c5f41f7a94e448fa845ad3cdaebbed1` 已通过：4096 字符 NER 实际替换、精确 4 MiB / 1024×1024 OCR 实际文本、两次原生许可均释放；越界输入先行拒绝且不新增许可。该应用使用真实业务 API，不提供 OpenAI 模型推理接口。
