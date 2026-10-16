# 固定应用的可信绑定（开发中）

此接入正在验证，尚未随 `0.0.26` 发布。内置 `config/app-calibrations.json` 现记录 Thor 的 MusicGen-small / SD-Turbo 及 AMD RTX 5880 的 MusicGen-small 实测预算；该源码尚未发布到已安装实例，其他 profile/模型及 Desensitize 仍无对应批准。隔离测试使用的模拟预算不能进入正式清单；真实任务校准、发布和逐机验收完成前，不宣称客户端全面迁移已完成。

控制器只为下列固定 VOS 应用接入，不提供允许调用者自报 URL 和预算的公开注册入口。原 V-Voice 专用绑定保持独立，见 [V-Voice 主动接入](v-voice-auto-binding.md)。

| 应用 | 固定执行服务 | 声明入口 | 任务类型 |
|---|---|---|---|
| `com.ictrek.v-music` | `http://v-music-backend:7080` | `/api/v1/discovery` | 同步 `model.infer` |
| `com.ictrek.desensitize` | `http://desensitize-backend:5000` | `/api/v1/discovery` | 同步 `model.infer` |
| `com.ictrek.image-studio` | `http://image-studio-backend:5000` | `/api/v1/v-router/registration` | 异步 `image.generate` / `video.generate` |

## 信任与校准

控制器读取自己由 VOS 挂载的应用 context，向 VOS 核验 `sub=com.ictrek.v-scheduler`，再确认目标应用在同一 VOS 已安装。发现请求不携带服务令牌；只向表中固定服务的 `/api/v1/scheduler/binding` 下发凭据，禁止跟随 HTTP 重定向。浏览器、模型共享目录和 discovery 不包含这些凭据。

模型必须同时满足：文件和后端已就绪、声明的原生 ID 与任务类型匹配、目标应用实际安装 profile 匹配、节点实际架构与 GPU/内存类型匹配、`execution_settings` 完整对象与实测配置一致。硬件取自 Scheduler 遥测，profile 取自 VOS 安装记录；不信任应用自报预算。

校准还须保存实测峰值、已确认停止的证据、固定 CPU/RAM/GPU 资源、输入字节上限和 `model_limits`。缺少任意必要条件不建立相应执行器。内置清单由发行方实测后发布，普通用户不需要 SSH 或逐模型手动批准。管理员私有覆盖文件为控制器 `/config/app-approvals.json`，须为普通文件、0600、最大 256 KiB。

测试容器的资源上限必须与正式运行配置对应；不能把限制了 CPU/内存的测试峰值用于未限额、线程数不同的正式进程。模型权重、运行后端、线程、库版本、offload 或输入参数的变化须重新匹配校准。

## 交接字段与执行责任

绑定包含 `protocol_version=1`、单调递增 `binding_revision`、`app_id`、固定调度 `url=http://v-router:8080/scheduler/api/v1`、本应用的 `token` 与 `executor_token`、固定 `upstream_url`、`execution_settings`、`models`、`gateway_executors`、`model_limits` 及 Image Studio 的 `model_parameters`。客户端先核验 VOS 身份、固定地址和控制器执行器，再以 0600 原子落盘；服务凭据不得交给业务用户。

- Music 限额包含 `duration_seconds`、`prompt_characters`、`negative_prompt_characters`。
- Desensitize NER 包含 `text_characters`、`max_tokens`；OCR 包含 `input_bytes`、`input_width`、`input_height`、`input_pixels`、`input_pages`。解码前及实际推理前均检查。
- Image Studio 包含图像、批次、视频、hires 和文本范围；`model_parameters` 完整匹配实测 JobRecord 的 19 个运行参数及 `has_reference`，使用 `model@variant` 原生 ID。不能用默认参数的预算放行其他 offload、缓存、后端或参考图组合。

直接同步调用由应用取得、续租和释放自己的许可。经统一网关调用时，应用使用独立身份核验并认领网关许可，不重复预约。Image Studio 使用持久任务、outbox 和现有 `start/inspect/cancel` 执行适配；网关的异步桥接许可经控制器核验后移交。原生业务接口、输入文件、输出权限仍由业务应用管理，业务 API 不必全部改写成网关 API。

## 持久化与撤销

控制器 `/config/app-bindings/<app>.json` 保存私有绑定状态，网关 `/gateway-data/app-bindings/<app>.json` 的对应卷保存网关自己的受限身份。网关每次请求读取原子快照；执行器和服务身份持久化在资源账本。异步适配更新与 V-Voice 的本地适配配置合并，保留其他应用条目。

内容相同时保留 revision 和凭据。变更必须等待本应用活动任务、许可及停止未知的预约结束；提高 `credential_epoch` 才轮换凭据，并撤销旧身份。管理员已禁用的执行器和已撤销的身份不会被同一配置重新启用。运行配置不再匹配时，旧自动执行器退出可执行集合；即使客户端尚持旧快照，也不能据此取得执行授权。

V-Sub 的每用户 worker/root broker 和动态 namespace 上游、HybRAG 的独立后台网关凭据不属于本固定三应用模块，仍须分别完成契约及验证，见[迁移协同契约](client-migration-contract.md)。

## 当前实测清单（尚未发布）

| 目标应用 VOS profile / 硬件 | 模型 | CPU / RAM | GPU | 已测输入范围 |
|---|---|---|---|---|
| `thor` / NVIDIA Thor 统一内存 | MusicGen-small | 4 核 / 12 GiB | 独占 1 槽；内存计入系统 RAM | 最长 10 秒，prompt / negative 各 512 字符 |
| `thor` / NVIDIA Thor 统一内存 | SD-Turbo | 4 核 / 8 GiB | 独占 1 槽；内存计入系统 RAM | 512×512、1 张、最多 4 步，prompt / negative 各 4000 字符；其余运行参数完整匹配 |
| `amd-cuda` / NVIDIA RTX 5880 Ada 独立显存 | MusicGen-small | 4 核 / 8 GiB | 独占 1 槽 / 3 GiB 显存 | 最长 10 秒，prompt / negative 各 512 字符 |

Image 使用实际 `--threads 4` 配置；其他线程或 offload 设置不能借用该预算。Music 使用实际 Torch 4/1 线程、单任务、单加载模型。资源值包含实测余量，不是模型文件大小。

229 节点经用户授权将总内存规划从 16 GiB 调到 20 GiB，保留原驻留基线 6 GiB；这使 12 GiB Music 预算可以在规划内被预约。192 经用户授权只将 CPU 规划从 4 核调到 4.5 核，覆盖 Whisper/Electron 的实测开销，内存规划未变。调整均通过现有带版本校验、节点空闲保护的管理 API 保存并回读，原规划已备份；物理空闲内存与安全余量检查继续生效。

这些是隔离真实原生任务的预算证据。正式绑定、网关调用、客户端安装和逐机验收仍须通过后才发布；192 的 Music 仍因实际内存不足待验收，不能据规划调整宣称通过。

AMD Music 记录进程 `ru_maxrss` 内核高水位、NVML 按进程显存采样及宿主内存变化；该机内核不提供 cgroup `memory.peak`，所以 `memory.current` 的采样最大值不标为精确峰值。批准预算覆盖已测值并留有余量，原始测量方法保留在清单证据中。

校准匹配目标应用自己的安装 profile，不能直接套用 Scheduler 的同平台名称：Music / Image 的 CUDA profile 为 `amd-cuda` / `arm-cuda` / `thor`，Desensitize 则为 `amd` / `arm` / `thor-spark`。目标 profile 从 VOS 安装记录读取，再与各自清单及真实运行设置一起精确匹配。

Image Studio 使用原生 `/api/v1` 适配，上游资源身份固定为 `http://image-studio-backend:5000` 根地址；不能附加不存在的 OpenAI `/v1`。Music / Desensitize 的 OpenAI 上游身份仍为各自根地址加 `/v1`。发现、绑定、网关 origin 和许可中的上游身份须一致。
