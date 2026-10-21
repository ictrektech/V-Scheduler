# MVP 验证记录

日期：2026-10-13。执行环境为获授权 tc232 隔离测试服务，代码从本地同步，测试数据与产物位于 `~/dev/build/V-Scheduler/`；没有修改已安装 VOS、Cluster Manager 或现有业务容器。

- Rust：节点与应用 DTO、单位转换、公开字段白名单、URL 凭据拒绝，以及 GPU 槽位请求必须具有真实遥测，5 项通过。
- 界面：Vue/TypeScript 生产构建通过；语言回退、订阅优先于快照、旧平台存储只读、手动覆盖及状态保留，4 项通过。
- Image Studio：68 项通过，包括已绑定任务鉴权、路径约束、同名不同扩展文件的原子写隔离、独立 Linux 进程租约超时后的停止与回收。
- V-Router：完整 Go 测试及 race 检查通过；覆盖云端绕过、共享本地许可、准入拒绝、delegated 避免双重预约、流中断保留隔离。
- 版本脚本：6 个模拟协议场景通过，覆盖成功、脏工作区、已有本地/远端标签、远端查询失败、推送失败；使用模拟 Git 命令，不运行真实 Git、不发布任何标签。
- 包模板：manifest/profiles/conflicts、导航和 Agent projects 路径及 workflow YAML 校验通过；主 compose 与两种 Agent compose 均通过目标机器 `docker compose config`。两种镜像 profile 的 dry-run 通过。
- 真实端到端：实际 Image Studio 原业务入口提交两个 sd-turbo CUDA 任务，验证单槽排队、幂等/冲突、服务身份隔离、真实 PNG、停止后释放、controller 重启、排空和排队取消、在线许可与图像任务共享预算、suspect 保留、运行中取消停止确认。
- 故障与恢复：Agent 被终止后，controller 租约到期保留预约并进入 recovering；恢复持久 journal 后只对账原 attempt，无并行重放，最终释放。任务可能在失联期间自行完成；这个场景不声称真实 CUDA 子进程一定由 watchdog 杀死。独立进程超时停止由上面的 Linux supervisor 测试覆盖。
- VOS 适配：使用与已核验源码 DTO 一致的只读契约模拟源，验证新节点仅观察、资源单位、节点/本机安装实例关联、字段脱敏，以及来源不可达时保留旧值并标记过期。
- 浏览器：实际构建页面检查 API 文档和新图标；演示模式中验证取消后转终态、删除确认/输出保留说明、人工重试回队列，切换繁中保持操作后的页面状态。演示操作不代表生产业务操作。

## 首版发布验证

- AMD64（tc232）与 ARM64（tc229）原生镜像构建、容器 CLI / UI 文件检查、仓库推送、飞书写入与回读通过；标签分别为 `amd_20261013` / `arm_20261013`，registry manifest 架构分别为 `amd64` / `arm64`。
- 两套实际发布镜像使用临时网络、专用 PostgreSQL 18 容器与随机身份配置，通过数据库迁移、健康检查、UI 和鉴权任务 API；验证容器与网络已清理。
- [GitHub v0.0.1](https://github.com/ictrektech/V-Scheduler/releases/tag/v0.0.1) 的 `v_scheduler_0.0.1_pull.tar` 已下载核验：外层仅 `app.tar.gz`，内层版本、完整镜像引用、两套 Agent projects、导航及三种语言说明正确，无残留模板变量。SHA-256：`9ed96f9c27ddbade206c16e2b8ce4eb463fb6e67c067fdc318a3d6a65fad9cbe`，与 GitHub asset digest 相同。
- [发布 CI](https://github.com/ictrektech/V-Scheduler/actions/runs/38339762990) 的前端、协议测试、飞书读取、打包和 GitHub 发布步骤通过；首次 App Store 步骤因缺少仓库级 Secrets 失败。补齐后，[续发 CI](https://github.com/ictrektech/V-Scheduler/actions/runs/38341855065) 下载已有 GitHub 安装包并成功发布，商店返回 `com.ictrek.v-scheduler@0.0.1`；没有重建或升版。
- 修复后的分离发布工作流通过 YAML、发布/重试条件和全部 shell 步骤语法检查。

## 商店列表兼容修复（0.0.3）

上传成功不代表 VOS 客户端可解析 manifest。首版缺少 `profiles[].description`，使用 VOS 原生 `vos-app-manifest` 模型复现：`profiles[0]: missing field description`；客户端目录会跳过解析失败的应用。另将未在分类枚举内的 `tools` 修正为 `system`。0.0.2 仅修正分类，完整修复位于 0.0.3。

- 补齐 AMD64 / ARM64 profile 描述；打包检查验证支持的分类和每个 profile 的描述，回归检查能拒绝缺少描述的包。
- 完整修复 manifest 通过 VOS 原生模型反序列化及 `validate()`；实际发布包与公开 metadata 的 manifest 字节一致。
- [0.0.3 发布 CI](https://github.com/ictrektech/V-Scheduler/actions/runs/38348346765) 的 GitHub 和 App Store 作业均成功；商店返回 `com.ictrek.v-scheduler@0.0.3`，渠道 `stable`。沿用原服务镜像，没有改动 VOS 平台代码。
- tc229 的实际 Safari 应用商店页面刷新目录后，搜索 `scheduler` 正常显示「V-Scheduler 资源调度 v0.0.3」，图标及描述可见。
- `v_scheduler_0.0.3_pull.tar` SHA-256：`89b83e2639396de3a7804160dceb5b289723392c0ddceba167f0d790ce1c81d4`。

以上为 0.0.3 阶段的边界；后续目标机器验证见下文。

## tc192、tc229 实际 VOS 验证（0.0.9）

- [0.0.9 发布 CI](https://github.com/ictrektech/V-Scheduler/actions/runs/38365367581) 的 GitHub Release 与应用商店发布均成功。AMD64、ARM64 镜像分别由 tc232、tc229 原生构建，推送并写入对应飞书表；安装包外层仅含 `app.tar.gz`，清单含原生 `oauth2.client`，路由为单个页面并带 `?v=0.0.9`。
- tc192、tc229 的 VOS 异步升级工作流均达到 `Completed`，应用详情均返回已安装 0.0.9。实际 VOS 页面复测：沿用平台登录后自动进入资源与任务页面，未出现 Scheduler 令牌表单；两台均显示真实资源、在线节点和已安装应用。节点名称从 VOS 设备信息读取：这两台的 `device_name` 均为空，分别回退到 VOS 主机名 `ubuntu`、`ictrek`；内部 Agent `node_id` 不再作为界面名称。保存了两台真实页面截图。tc192 起初复用了同日标签的旧镜像，显式拉取新 digest 并重跑 VOS 升级后，已在实际页面确认新名称。
- 两台 Controller 均连接专用 PGV 数据库，tc192 的旧 PGV 自定义管理凭据通过一次性受保护文件完成初始化，之后移除。两台资源采集后台服务均在线；tc192 未安装 Image Studio，Agent 采用只上报资源、不认领任务的空执行器配置。当前 VOS projects API 在目标版本报暂存 `.env` 不存在，故用本应用自带 Compose 模板部署后台服务；不声称该 projects API 已验证通过。没有修改 VOS 平台或 vos-cluster-manager 代码。
- tc229 的实际 Image Studio 入口完成两个 SD-Turbo CUDA 任务，输出两份真实 PNG；任务串行占用一个 GPU 槽，预约最终归零。实际 V-Router 本地 Qwen 模型请求成功；资源被许可或图像任务占用时返回 429。此项在 0.0.7 的同机 Controller 与业务验证包上执行，0.0.9 升级后两条完成任务仍在真实任务列表中；未为 0.0.9 重复生成图像。
- Image Studio `0.0.61-vscheduler.1` 与 V-Router `0.0.43-vscheduler.1` 是 tc229 定向验证包，不等同所有 profile 的正式业务应用发布。Cluster Manager 清单的跨节点自动关联及更多新机器的自动后台服务部署仍待实现。

## 节点健康、容量与 V-Router 接入顺序（2026-10-14）

- 本轮源码同步至 tc232 的 `~/dev/repos/V-Scheduler`，构建产物留在 `~/dev/build/V-Scheduler`。Rust 11 项单元测试、前端 4 项测试及生产构建通过。独立 PostgreSQL/Controller 端口上的 API 故障注入通过：离线、排空、执行器探测失败、Agent 遥测过期、常驻基线加预约后的容量已满、进度事件快照，以及心跳/租约失效进入 recovering/suspect；旧执行确认停止前不派发后续任务。隔离容器已清理，原有测试服务和已安装 VOS 未改动。
- 只读核对运行实例：tc229 的 V-Router 有字段结构可解析的非空调度配置，V-Scheduler 有 V-Router scoped principal 与本地推理 executor；tc192、tc232 的 V-Router 均缺调度配置，对应 V-Scheduler 也缺 scoped principal/executor。此项只证明配置存在与否，未在本轮用 V-ChatCut 的真实模型请求重新验证 permit。V-Router 目前在缺配置时沿旧路径直转，因此 tc192/tc232 的本地 V-Router 请求尚不能宣称受调度；启用需分别准备受保护配置并安排两服务重载、做本地请求/云端绕过与忙碌拒绝验证。

## 完整网关准入与系统余量（2026-10-14，0.0.11 发布门槛）

- tc232 新源码 Rust 13 项、Go 全包测试、前端 4 项与生产构建通过。网关契约覆盖原生 HTTP、SSE、Responses/Messages 转换、双向 WebSocket、异步终态缓存/用户隔离/重启恢复，未知模型、输入/资源参数上限、容量拒绝以及云端范围。网关模型请求饱和时 `/health` 可用，请求结束后容量恢复；multipart 资源参数也不能绕过预算。
- 真实 PostgreSQL API 验证：错误用户/地址不能借用任务预约，关联子调用总预算不超父任务；父任务终态保留 suspect 预约直到子调用停止，停止前不能派发重试。桥接许可仅移交给匹配的持久业务任务，许可历史按 app 隔离。
- tc232 隔离运行新版 Controller、Agent、V-Router 和只读共享模型的 Image Studio 执行端，完成真实 sd-turbo CUDA PNG，网关返回原异步工作协议，Scheduler 保留并核验桥接→task 移交，最终预约归零。同次验证通过排队、取消、Controller 重启、Agent 崩溃的独立 watchdog 与 journal 恢复；未重放创建，未修改已安装 VOS 应用。证据目录：`~/dev/build/V-Scheduler/e2e-1791944490755`。
- 初次真实测试暴露 GPU 遥测可能拖延 Agent 续租的问题。已将单一采样循环移到后台，限制 nvidia-smi 等待，保留采样原时间；遥测失效暂停新工作，独立续租不受采样阻塞。修正后上述真实链路与崩溃恢复均通过。
- 本轮是发布源码和包的验证，不是全部运行节点的升级验收。旧配置只有 default_local 的节点也须新增逐模型预算、executor upstream_urls；未配置节点升级到强制版后本地请求将明确拒绝。系统余量是保守准入保护，不是针对未纳管进程或错误模型峰值的硬隔离保证。

## 已安装应用完整调用链验收（2026-10-14）

- 实际安装版本：两台均为 V-Scheduler `0.0.12`、V-Router `0.0.43`。tc229 是 Jetson Thor，Scheduler 使用 `thor-spark`，Router 使用其架构 profile `arm`；tc232 是 AMD64 NVIDIA，两个应用均使用 `amd`。Router 不依赖 GPU runtime；两台正式 Scheduler 均使用 NVIDIA runtime。通过 VOS 应用详情、实际容器和驱动采样分别核验。
- 补齐测试机受保护的逐模型策略、固定资源模板及上游地址绑定，原配置先备份，没有把凭据提交到仓库。tc229 的旧配置还缺少桥接任务移交授权：只批准 V-Router 移交给既有 `image-studio.generate`，之后队列恢复执行，没有绕过资源预约。
- 两台实际 V-Router → V-Scheduler → Model Hub：Qwen3.5 2B 普通 JSON 和完整 SSE 均成功；BGE-M3 返回 1024 维向量。每次请求在调度账本中具有对应模型许可，完整响应后以停止证据释放。
- 真实网关请求的超预算参数在分配前返回 422；预占一个实际资源槽后返回 429。未确认停止的许可进入 suspect 并继续阻止计算，只有确认测试许可从未启动计算后才释放。测试结束没有遗留预约。
- 两台通过已安装网关完成 SD-Turbo 256×256、4 步 CUDA 生图并核验 PNG。创建响应后重启网关，不重放业务创建：tc232 从持久异步日志恢复轮询，执行端终态后释放；tc229 将桥接许可移交给已批准的持久任务，任务成功后释放。另创建独立测试任务验证取消，原应用等待停止后删除该测试任务，调度预约最终归零。
- 跨节点：tc229 使用同一已发布 ARM64 网关镜像启动仅监听回环地址的临时测试实例，通过临时 SSH 通道访问 tc232 的真实 Model Hub 与 Scheduler。Qwen 推理成功，许可计入 tc232 的真实节点并释放；占用资源槽后远端推理返回 429。这验证经该测试通道的跨机准入，不代表两台的生产网络已直通，也不代表已完成多节点统一控制器部署。
- 云端：同一临时网关调用真实外部 DeepSeek 模型。tc232 的受管 GPU 槽被测试许可占用时，云端推理仍返回 200，调度账本没有新增云端许可。临时网关、外部 API 凭据副本、测试 executor 与跨机通道均已清理。
- CPU profile 路径：AMD64、ARM64 分别使用已发布 Scheduler 镜像、独立 PostgreSQL 和显式 `runc`，不注入 GPU 设备或驱动库。在容器限制 1 CPU / 512 MiB 下，应用正常启动，自动上报宿主机逐核 CPU 与 RAM，GPU 列表为空，证明该路径不依赖 NVIDIA runtime。没有把两台正式 GPU 安装切换为 CPU profile。
- 以上是真实代表性调用和故障场景验收，并非模型目录中每个模型、任意上下文/输入大小或所有设备的峰值认证。此次本地模型策略限定输入与资源参数，实际验证使用短文本和上述小尺寸图像。两台应用最终健康，节点持续采集，无未释放测试预约。脱敏验收记录保存在测试产物目录 `~/dev/build/V-Scheduler/live-model-validation-20261014/`。

## 0.0.20 节点管理验收

- 使用现有 VOS App 只读链路核验 ModelInfer 节点清单和四个节点的监控接口，均返回有效数据。未部署第二套 Agent，未修改节点、监控或现有推理部署。此项只证明读取能力。
- 隔离 AMD 环境运行真实 Rust controller、PostgreSQL 17、统一网关和内部协议组件；四个 worker 身份位于同一构建主机，ModelInfer/模型执行为受控协议 fixture，不是四台 Spark 性能或真实远端控制验收。
- 覆盖上一版数据库迁移和任务历史保留、来源清单失败保留缓存、指标脱敏与过期、VOS 来源规划的权限/并发/重启恢复、规划不授予执行、预留控制返回 501。
- 内部协议覆盖 pending 身份不能执行、单次邀请与幂等恢复、真实受控任务执行、固定四节点许可整单预约/拒绝、单个非主节点阻塞、未知停止保持全部预约、撤销与预算冲突保护，以及网关与后端核验不重复扣减。脚本：`scripts/e2e-node-management.mjs`。
- Rust 25 项单元测试和前端 10 项测试通过；前端类型检查、生产构建通过。桌面页面验证预算规划保存与重读，并检查简中、英文、繁中及手机宽度。
- ModelInfer 真实远端调度仍待 VOS App 管理适配器的执行、停止、租约和故障恢复验证；来源节点在此之前保持仅观察/规划，不宣称已受调度。

## 0.0.21 同日镜像缓存修复

- 实际 VOS 升级到 0.0.20 后仍只显示本机：安装 manifest 已是新版，但运行中的 `/discovery` 缺少新版能力、`/management` 返回 404。同期 ModelInfer 节点接口正常返回四台节点，排除来源缺失。
- 根因是同日日期标签复用，而 controller 没有继承 `pull_policy: always`。平台预拉取使用 `pull --policy missing`；已有旧镜像不会在该步骤更新。controller 现与 gateway 一样，创建容器时检查并拉取标签的当前镜像。
- 回归测试修复前四种 controller 服务均失败；修复后 11 项打包/依赖检查通过，实际 Docker Compose 对六种 profile 解析得到的服务均为 `pull_policy: always`。本次只改包配置，复用已核验的 AMD/ARM 镜像。
- 通过 VOS 应用商店升级接口完成实际 0.0.21 升级，继承原存储、运行配置和用户设置；升级前确认无未完成任务和资源预约。运行中的新版接口返回本机加四台 ModelInfer 来源节点，发现状态 ready；四台 GB10 的 CPU、内存、GPU 指标均有新鲜样本，实际提供的前端也包含节点管理与预算规划。
- 四台来源节点保持观察/规划状态。ModelInfer 的现有部署状态与节点分配保持一致；未安装额外 Agent、未改动远端推理部署。真实远端调度控制仍待后续适配器验证。
- [发布与应用商店 workflow](https://github.com/ictrektech/V-Scheduler/actions/runs/38760077103) 的 release、appstore 均成功；[v0.0.21](https://github.com/ictrektech/V-Scheduler/releases/tag/v0.0.21) 下载包的 SHA-256 与 GitHub asset digest 匹配，包内 controller/gateway 的拉取策略已核验。

## 2026-10-16：0.0.26 试聊与终态许可修复

- [v0.0.26 发布及商店 workflow](https://github.com/ictrektech/V-Scheduler/actions/runs/38843687553) 均成功；192、229、232 经 VOS 安装流程更新，profile 分别为 `l4t`、`thor-spark`、`amd`。每台六个容器健康，九项默认内外端点均可达，Ollama 仍使用对应平台原 `20261012` 镜像。
- 原共享目录原位保留：192 的 3 个原模型/18 个原文件、229 的 24/566、232 的 12/46，逐个检查原文件 inode、大小和修改时间，均保持一致。上述数量不包含验收临时模型。
- 229 默认 Qwen3:4b 流式请求实际收到思考字段和正文。旧界面忽略思考流，且等待阶段只有省略号；现显示阶段、耗时，识别思考而不当作正文，并报告流内错误及无正文结束。
- 实际复现客户端收到 `[DONE]` 后立即关闭、传输 EOF 尚未读取时，网关将已结束计算记为 suspect，后续请求被遗留预约阻挡。修复仅信任固定自带 Ollama 的完整 SSE 终态事件；残缺流、未到终态的断开和其他来源仍保留原停止确认要求。
- 三机分别完成两轮真实 SSE 请求，客户端都在 `[DONE]` 后直接关闭，每轮新增许可均为 released：192 使用小模型/受控输入，约 1.87/1.81 秒；229 默认输入约 61.10/10.41 秒；232 受控输入约 90.06/3.76 秒。首轮包含冷启动，不能据此承诺延迟。229 已安装页面另完成默认试聊约 16.20 秒并显示完整回答，浏览器无相关错误；当次浏览器经回环验收代理使用服务端身份，不代替 VOS iframe 登录流程验收。
- 232 升级曾因遗留旧 Scheduler 容器占用 `36005` 等端口失败，VOS 补偿撤销安装记录。核验其无活动计算后，仅停止这组孤儿容器并保留回滚，使用原卷、配置和共享目录经 VOS 重新安装，最终审计通过。旧 Model Hub 的两条真实计算未被停止；等待其自然结束及负载回落后才补完新网关调用。
- 上述证明涵盖本次故障、三个已安装 profile 和代表性真实调用，不构成任意模型/输入的峰值认证，也不表示其他客户端的新增自动绑定已发布。新应用绑定的模拟协议验收与真实校准状态见[固定应用绑定](application-auto-binding.md)。

## 2026-10-16：仓库下载修复源码验证（尚未发布）

- 相同 build host、相同 reqwest 与相同 ModelScope LFS 文件进行两轮对照：未设 User-Agent 时 CDN 返回 403；使用固定应用 User-Agent 时返回 206。仓库元信息和非 LFS 文件均可访问，不能把该故障归因为模型不存在或调度准入拒绝。
- 修复后在独立空目录运行真实 Rust 模型仓库服务：`huluxiaohuowa/rapidocr-ppocrv4-onnx` 全部 7 个文件约 3.01 秒 READY；`huluxiaohuowa/bert4ner-base-chinese-onnx` 全部 12 个文件（含 406,945,730 字节 ONNX 权重）约 40.33 秒 READY。下载过程执行文件大小与 SHA-256 校验，未修改三台机器的原共享模型。
- 下载错误现在保留安全分类，区分 HTTP 状态、网络/TLS、存储、校验和范围错误；不会输出带签名的重定向 URL、令牌或存储私有路径。Rust 测试与构建通过。本节仅证明修复源码，三台已安装的 `0.0.26` 尚不包含该修复。

## 2026-10-16：应用接入源码验收（尚未发布）

- 经用户明确授权，通过管理 API 保存并回读节点规划：192 CPU 4000→4500m；229 总 RAM 16384→20480 MiB，保留 6144 MiB 驻留基线。原规划私有备份，GPU 与安全余量配置未改。192 的 Music 仍受真实可用内存保护，不据规划调整宣称已通过。
- MusicGen-small：229 真实 1/10 秒 WAV、512 字符 prompt/negative 边界、卸载与运行中取消通过；232 真实 10 秒 WAV、512/512 边界及取消通过。清单分别匹配目标应用 `thor` / `amd-cuda` profile、实际 provider/线程和硬件；Image 使用自己的 `thor`，Desensitize 保留 `thor-spark`，不混用 Scheduler profile 名称。
- 229 新版 Controller/Gateway、独立 PostgreSQL 与最终 Music 镜像构成隔离真实链路，外层持有实际 Scheduler 的 12 GiB 许可：实际 VOS context 身份及目标安装 profile 核验、固定地址自动交接、业务直连与统一网关各生成真实 10 秒 WAV，每条调用恰好一笔停止后已释放的内部许可；duration=11 两条路径均在计算前拒绝。生产实例未替换。证据 `~/dev/build/V-Scheduler/actual-binding/246ce55a6a9b4c5dbb50f77e3def99a8/result.json`。
- HybRAG 新身份的隔离真实 Controller/PostgreSQL/Gateway 协议测试通过：固定可信回调、推理凭据不能调用管理/图片接口、错误凭据拒绝、重启不重置凭据及修订、卸载停用、重装轮换、旧 Model Hub 模式停用。使用模拟 VOS/HybRAG 端点，不将此结果当作真实 HybRAG 后台索引验收。脚本 `scripts/e2e-gateway-identity.py`。
- AMD Rust 44 项及 Go 全包测试通过，新增 Voice 能力/模型输入限额、管理后预算匹配、目标应用 profile、Image 原生根地址和后台身份权限测试。新的 Voice 客户端边界、Image 原生异步绑定和 HybRAG 客户端仍待最终镜像联调；V-Sub 动态 namespace 协议亦须独立完成，不因此宣称全面迁移或最终发版完成。

## 0.0.27 统一模型仓库与会话修复（2026-10-16）

发布流水线 `38865702637` 的测试、GitHub Release 与 VOS App Store 均成功，发布包六种 profile 的镜像映射、公共目录参数、三个默认关闭的预热开关已核对。Rust 50 项、前端 17 项、AMD / ARM Go 全包测试及模型仓库 41 项隔离 API 验收通过；接口验收包括选中文件下载、追加、更新、续传重试、导入及推理后端管理。

192=`l4t`、229=`thor-spark`、232=`amd` 均安装 `0.0.27`，每台 6 个容器健康、9 组 Docker 别名与宿主端口通过验证，运行镜像与实际发布的 manifest/config digest 相符。各自原 Model Hub 公共目录保留，原模型/文件分别为 3/18、24/566、12/46；原文件 inode、大小和 mtime 均保留。关于接口仅输出镜像名与 tag。

三台旧版都曾将无效 VOS 会话误报为 HTTP 403 `FORBIDDEN`；更新后均为 HTTP 401 `UNAUTHENTICATED`。浏览器候选验证覆盖 HTTP 401 后重新获取凭据、宿主令牌轮换后持续轮询、无需页面刷新恢复连接，控制台无相关错误。真实非管理员仍受原权限检查。

229 使用试聊默认请求 `qwen3:4b` 连续两轮取得完整正文与 `[DONE]`；耗时 60.66 / 10.70 秒，正文 159 / 110 字符，各一份许可均释放。证据为 `install-20261016/v27-terminal-close-regression.json`。内置模型绑定缺失及时返回 503，资源忙及时返回 429，避免网关静默等待一至五分钟后撞客户端超时。

客户端可信绑定与实际任务范围见 [固定应用](application-auto-binding.md)、[V-Voice](v-voice-auto-binding.md)、[V-Sub](v-sub-auto-binding.md)及 [HybRAG](hybrag-gateway-identity.md)。192 Voice 的可用内存、232 现有 Model Hub QA 的 CPU 占用仍限制部分正向计算；容量拒绝与认证专项验证不计为原生模型输出通过。

## 2026-10-16：0.0.28 取消、原生停止确认与三机接入

[v0.0.28](https://github.com/ictrektech/V-Scheduler/releases/tag/v0.0.28) 的[正式发布与商店工作流](https://github.com/ictrektech/V-Scheduler/actions/runs/38873976361)均成功。192 使用 `l4t`、229 使用 `thor-spark`、232 使用 `amd`，各自六个服务健康，九组内部别名与宿主端口通过核验，实际镜像与发布 manifest/config 匹配。三台沿用原 Model Hub 公共目录，原有模型和文件保留；公共 ARM CPU VOS 使用 `arm-without-cuda`，原十项模型及公共目录引用保留。

试聊在流式和非流式请求期间显示“停止”，通过取消当前请求保留已返回内容并恢复发送。192 的真实网关流式取消后，内置 Ollama 通过原生停止屏障确认进程退出，控制器以 `builtin_ollama_native_stop_confirmed` 释放许可；随后重试返回 HTTP 200 和非空正文。其他应用、通用 vLLM 或未经确认的远端执行不继承该证明。隔离测试覆盖运行中拒绝停止确认、过期/错误回执不释放、未知执行保留预约及正常重试；AMD/ARM Rust 各 52 项、前端 17 项及对应 Go 测试通过。

229 已安装版本的三个原生后端分别完成对话、1024 维向量和重排，HTTP 200，每项恰好一份许可并正常释放。公共 ARM CPU VOS 的真实 2B 短文本请求返回完整终态及正文，耗时 64.19 秒；这证明该次调用完成，不代表 CPU 推理延迟已优化。ModelInfer 来源节点仍只观察和规划，未安装额外 Agent，未把四台 Spark 的内存当作宿主机可用内存。

HybRAG 0.1.71 的真实 VOS 登录、模型发现和专用网关绑定在三台均通过。运行中的后端与前端共六项镜像 manifest 与正式发布一致。逐模型实测如下：

| 主机 | 对话 | 向量 | 重排 | 许可结果 |
|---|---|---|---|---|
| 229 | 有正文，29.84 秒 | 1024 维，2.62 秒 | 有排序，8.26 秒 | 每项一份，全部释放 |
| 232 | 有正文，81.94 秒 | 1024 维，18.94 秒 | 有排序，58.30 秒 | 每项一份，全部释放 |
| 192 | 资源门禁 429，未启动 | 资源门禁 429，未启动 | 有排序，4.32 秒 | 拒绝项无许可；重排一份且释放 |

192 随后的只读核验显示可用内存约 3981 MiB、有效系统保留 2048 MiB、未释放许可为零。当前对话和向量模型批准的内存需求分别为 6254、3234 MiB，均无法满足当时的“需求加系统保留”准入条件；没有一次失败后旧许可永久占用的证据。未通过修改预算、改为外部资源或停止其他用户应用来绕过门禁。

脱敏发布与验证产物保存在维护端 `~/dev/build/V-Scheduler/releases/0.0.28/verification.json`，客户端结果保存在 `~/dev/build/vai-client-migration/evidence/hybrag-{192,229,232}-functional.json`。上表证明逐模型调用及调度交接。接入步骤统一维护在[统一接入指南](application-integration.md)。

### HybRAG 完整文档入库与 RAG 验收

229 的已安装 HybRAG 0.1.71 通过真实 VOS 登录创建本次专用知识库，上传合成文档，等待解析完成，检索命中该文档，再经实际 RAG 会话返回包含文档验证事实的回答。另检查 SSE 的 answer 本体为 180 字符并包含验证事实，未将引用区命中误算为生成回答通过。流程产生七份调度许可，全部正常释放。测试会话与知识库已删除，未修改用户默认模型或既有知识库。脱敏摘要为维护端 `~/dev/build/vai-client-migration/evidence/hybrag-229-rag.json`。此项为代表性文本入库与回答验收，不扩展为任意文件格式、输入规模或模型组合的认证。

### HybRAG 新旧模型入口共存验收

229 以正式发布的 HybRAG 镜像运行独立 PostgreSQL 和隔离 HybRAG 实例，复用原独立 Model Hub 的三个真实 Ollama 后端，启动独立旧 V-Router 0.0.44 镜像。隔离目录显式登记本次三项旧服务；同一 HybRAG 进程同时发现新旧模型目录，并通过真实 `/api/v1/models/{id}/debug` 完成旧路径对话、1024 维向量、重排，以及新 Scheduler 路径向量调用。四项均有实际输出、各一份许可且已释放；新旧密钥分别绑定，新的专用后台密钥未发送给旧 Router。脱敏证据为 `~/dev/build/vai-client-migration/evidence/hybrag-229-coexist.json`。

旧路径使用本次专属 submitter 和三个管理员批准的 executor，各自绑定真实旧后端地址与原有保守预算。`permit_executors` 仅授予验票能力，不代替申请方与 executor 的 `app_id` 匹配。未降低预算、未将本地计算声明为 cloud、未修改统一准入策略。测试实例只允许本次指定的旧 Router 目标；已安装用户实例对临时私网地址的 SSRF 拒绝保持有效。此项证明正式镜像的受控真实共存能力，不表示修改了用户当前默认模型或已安装实例的旧服务配置。

### 0.1.72 启动修复交付与最终回归

共存验收暴露了 HybRAG root 启动时 DuckDB 预装扩展缓存路径不一致的问题：镜像缓存位于 `/home/appuser/.duckdb`，root 使用 `/root/.duckdb`，会触发外网下载。修复仅补齐 root 的预装缓存，保留 DuckDB、XLSX 与访问控制。AMD、ARM 正式后端镜像均在禁止外网的 internal 网络、root 默认 HOME、扩展启用的条件下验证 spatial/excel 可安装及加载，真实 HybRAG 健康。修复通过 [0.1.72 正式发布](https://github.com/ictrektech/WeKnora/releases/tag/v0.1.72)与[发布工作流](https://github.com/ictrektech/WeKnora/actions/runs/38891867689)交付。

192、229 的 HybRAG 使用 `arm`，232 使用 `amd`，三台均为 0.1.72、installed、runtime_ready；这与 Scheduler 自身的 `l4t`/`thor-spark`/`amd` profile 分别选择。三台前后端共六项运行镜像 manifest 与最终发布一致，真实登录、模型发现、独立网关绑定及后台私钥不暴露均通过。229 已安装的正式 0.1.72 再次完成测试文档上传、解析、检索与含验证事实的实际 answer 事件，七张许可全部释放，测试会话及知识库删除。脱敏记录为 `~/dev/build/vai-client-migration/evidence/hybrag-229-rag72.json`。192 的实际容量限制仍按前表记录，不将拒绝冒充输出通过。

旧路径共存测试的最终只读清理确认：临时容器已移除、三个专用测试执行器已停用、专用服务身份已撤销、全部本次许可已释放。未改用户默认模型、原有知识库或原 Model Hub 模型目录。新旧入口的受控真实共存和已安装新入口的完整 RAG 均已验收。


## 2026-10-16 Rust unified-service candidate

Router is reimplemented in `backend/src/router/`; Scheduler, Router and model management share one Rust process. Each of the six package profiles selects four containers. Existing gateway/model-workspace DNS aliases, nine port mappings, retained gateway state and the installation-selected shared model directory remain compatible. The About dialog lists four actual images as name:tag and marks Router/model management as embedded components.

AMD and ARM each pass 77 Rust tests, including retained Go JSON state/zero timestamps, routing/circuit recovery, private ownership, multipart bytes, Responses/Messages tools and SSE, WebSocket frames/custom close codes, terminal/truncated/cancelled resource lifecycles and persistent consumable async receipts. Model management passes 41 isolated acceptance checks. The 16 packaging/helper tests and frontend tests/build pass. The actual unified-process API fixture passes identity matrices, CRUD/probes/registrations/resources, usage, all six documentation groups, PGV outage/restart and initial PGV unavailability. Browser checks cover all eight pages and shared About, four image rows without registry paths, model-workspace tabs, desktop/mobile and complete streaming/non-streaming chat without JS/request errors.

These candidate checks use isolated data and synthetic model/identity endpoints. Publication, named-machine upgrades and native model/stability results are recorded separately after execution.


### v0.0.30 publication and installed acceptance

[Workflow 38917627885](https://github.com/ictrektech/V-Scheduler/actions/runs/38917627885) completed both GitHub Release and VOS App Store publication. The downloaded `v_scheduler_0.0.30_pull.tar` has exactly one outer member, all six profiles select four containers, and each profile retains the original `ollama_server` image mapping. Both unified service images were pushed and their `v-scheduler` Feishu cells read back successfully.

| VOS test machine | Profile | Healthy containers | Original models retained | Original files retained |
|---|---|---:|---:|---:|
| 192 | `l4t` | 4 | 10 | 457 |
| 229 | `thor-spark` | 4 | 28 | 601 |
| 232 | `amd` | 4 | 18 | 484 |

Every machine is installed at 0.0.30. Original public-directory UUID/mount, model IDs and file inode/size/mtime were preserved; all nine internal alias/host-port pairs passed. Runtime image identities match the published manifests (Docker's containerd image store can report the manifest digest instead of the configuration digest). About returns four image name:tag entries and marks Scheduler's embedded components.

229 and 232 each pass actual QA streaming, BGE-m3 embedding and rerank calls with nonempty/valid native output and exactly one released permit per call. 192 passes a small installed QA model; BGE-m3 and rerank return the expected resource gate under its existing capacity/observed-memory limits. At measurement the model budgets were 3234 MiB and 2238 MiB, while observed free memory minus the 2048 MiB reserve was about 2199 MiB. These two 192 calls are **not** counted as successful inference. No capacity, safety reserve or model policy was weakened.

All three machines pass actual stream cancellation, native stop confirmation, permit release and a subsequent nonempty response. Their five-minute continuous probes completed 891 (192), 900 (229) and 882 (232) successful endpoint checks with four healthy containers, no OOM and no unexpected restart. This is a bounded five-minute check, not a long-duration soak. 229's installed HybRAG 0.1.72 also passed upload, parsing, retrieval and an answer containing the synthetic fact; its test session/knowledge base were deleted and all seven new permits released. All three machines passed unified-container restart recovery after the probes finished: original model workspace, version/profile, built-in model catalog and Scheduler API recovered.

After restart, all three installed HybRAG instances also passed existing gateway binding, real VOS login, built-in model discovery and service-key non-disclosure checks. No client default configuration was changed.


### v0.0.31 installed client compatibility

[Workflow 38923451687](https://github.com/ictrektech/V-Scheduler/actions/runs/38923451687) completed GitHub Release and VOS App Store publication. The downloaded pull package selects four containers for each of the six supported profiles. The existing build scripts published the AMD and ARM unified images and read back the Feishu component cells; Ollama images remain selected from the original `ollama_server` cells.

Actual V-ChatCut calls exposed a Rust Router regression: forwarding browser `Origin` caused Ollama to return 403, which the client labelled as an API-key failure. The same native request succeeded without `Origin`. The previous Go proxy stripped both `Origin` and `Accept-Encoding`; the unified Rust proxy now does the same for server-to-server HTTP/WebSocket forwarding. A rejecting fake native provider reproduces the failure before the fix and passes both streaming and non-streaming calls afterward. All 78 Rust tests pass on AMD and ARM.

192 (`l4t`), 229 (`thor-spark`) and 232 (`amd`) were upgraded through VOS to 0.0.31 with their original installation selections. Each has four healthy containers, matching published unified-image identity, all nine internal/host endpoint pairs, and four `name:tag` About entries. The original shared-directory UUID/mount and all model IDs/files were retained: 192 has 10 models/457 files, 229 has 28/601, 232 has 18/484. File retention checks compare inode, size and mtime.

The compatibility tests use installed clients, actual VOS login and native inference, with synthetic inputs and isolated temporary knowledge bases/jobs. They cover representative integration paths rather than every editor feature. Browser-header tests go through the unified gateway; direct client tests additionally cover native Music, NER/OCR and HybRAG indexing/retrieval/chat. Unsupported profiles and resource refusals are recorded separately from successful inference.


| Installed client / actual operation | 192 `l4t` | 229 `thor-spark` | 232 `amd` |
|---|---|---|---|
| V-ChatCut: VOS login, catalog, non-streaming and streaming QA | Pass | Pass | Pass after transient 429 retry |
| V-Voice: generated WAV and ASR transcript through the gateway | TTS resource gate (429); dependent ASR not completed | Pass | Pass |
| V-Sub: private worker, 20-second native ASR via gateway | Pass | Pass | No calibrated AMD private worker/model; unavailable |
| V-Music: real 10-second WAV, direct and gateway | No approved L4T binding (503) | Pass | Pass |
| Image Studio: asynchronous SD-Turbo PNG through gateway | Selected shared model not ready; unavailable | Pass, native job and Scheduler task succeeded | No approved AMD binding (503) |
| desensitize: native NER and image OCR replacement | Resource gate (429) | Pass | Pass |
| HybRAG: VOS identity, gateway binding and model discovery | Pass | Pass | Pass |
| HybRAG: upload → parse → retrieve → answer containing synthetic fact | Parsing failed, zero inference permits; not passed | Pass, seven permits released | Pass on retry, seven permits released; initial `WAIT_STOP_CONFIRMATION` recorded |

232's first V-ChatCut test received a transient 429 between sequential inference requests. A bounded five-second retry completed both responses with exactly two released permits. The test waits for native stop acknowledgement rather than assuming permit release is synchronous with the response body. HybRAG's first answer stream reported `WAIT_STOP_CONFIRMATION` after successful parsing and retrieval; its five acquired permits all released and its temporary session/knowledge base were deleted. These first-attempt failures are retained as integration limitations, even when a later retry succeeds.

Each machine passed a fresh bounded one-minute health probe during the client checks: 192 and 229 each completed 180 successful probes, 232 completed 177; zero errors, no new restarts/OOM, and four healthy containers. This is not a long-duration soak. No resource reserve, approved model budget, profile calibration or client default was weakened to obtain a pass. Test-created knowledge bases/sessions and image jobs are removed; unrelated user data and pre-existing tasks are preserved.

Private operational evidence is stored under `~/dev/build/V-Scheduler/compatibility-20261016-231300/` on the test hosts and the local development machine. `gateway-browser.json`, `post-clients.json`, per-case logs, `upgrade/audit.json` and `stability-0.0.31.json` retain distinct success, blocked and unavailable outcomes. Older baseline results remain separate from post-upgrade results; a zero script exit alone is not counted as native inference success.

232 subsequently passed the complete RAG retry: the returned answer contained the synthetic fact, all seven new permits released, and both test session and knowledge base were deleted. The transient first-attempt refusal remains recorded above. Final idle audits on all three machines found no active/suspect permits, running compute tasks, native active slots or model downloads.

### v0.0.36 publication and installed acceptance

[Workflow 39201212533](https://github.com/ictrektech/V-Scheduler/actions/runs/39201212533) completed GitHub Release and VOS App Store publication on its third attempt. The first two attempts never started a runner; after the repository became public, the original workflow successfully ran frontend/API checks, all 85 AMD Rust tests, packaging and publication. ARM host tests passed all 86 cases, including the bundled seven legacy Voice plus four bounded music model binding and installed-version gate.

The downloaded pull package SHA-256 is `3d1e9ec81c821ae4ced502e6aaa663c6321aeee3b6038ccddff457db120d61ee`. Its single inner archive, seven rendered core files, published AMD/ARM Scheduler tags and stable aliases passed inspection. Registry manifests and the exact Feishu component cells were checked independently.

| VOS test machine | Preserved profile | Original models/files retained | Healthy containers | Alias/host endpoint pairs | Successful 60-second probes |
|---|---|---|---:|---:|---:|
| 192 | `l4t` | 13 / 478 | 4 | 9 | 180 |
| 229 | `thor-spark` | 28 / 601 | 4 | 9 | 180 |
| 232 | `amd` | 19 / 491 | 4 | 9 | 174 |

All three actual installed versions and image identities match 0.0.36. Shared-directory UUIDs, storage paths and original file inode/size/mtime were preserved. About reports four image name:tag entries. All bounded probes passed without errors, new restarts or OOM; this is a one-minute check, not a long-duration soak.

232's first upgrade failed because three old Scheduler Ollama containers retained host port 12434. The failed workflow rolled back and deleted its installation record; a generic platform `installed` status was not counted as successful upgrade. After rollback completed, the three containers were confirmed idle and stopped. Recovery installation reused the verified original storage volume, shared directory, profile and saved configuration. Its workflow completed and the full version/image/file/endpoint audit passed. No model data or shared directories were removed.

On installed 0.0.36, 232 passes V-ChatCut non-streaming followed immediately by streaming, real V-Voice TTS WAV plus nonempty ASR, and V-Sub direct/gateway, oversize/settings rejection, native cancellation and worker-generation recovery. All eight permits released; the chat checks use no client-side retry. Evidence is retained under `~/dev/build/V-Scheduler/p0-20261019/`, with `upgrade36/`, `regression36-results.json` and `stability-0.0.36.json` kept separate from earlier releases.

The new Voice 0.0.21 CPU/music approvals still require that application's matching trusted installed version and advertised capabilities. Native calibration and this Scheduler release do not by themselves prove that Voice 0.0.21 has been packaged, deployed or accepted. Its final image and deployed API checks remain a separate delivery gate.

## P0 最终交叉验收（2026-10-19，Scheduler 0.0.36 / V-Voice 0.0.21）

V-Voice 0.0.21 的正式 Release 与商店发布成功，192、229、232 均通过 VOS 升级到该版本。实际 profile 分别为 `l4t`、`thor`、`amd`；Scheduler 分别为 `l4t`、`thor-spark`、`amd`。核验运行镜像、原生源码哈希、About 和前端版本，保留原共享目录、应用数据目录、CPU 模型选择与运行参数。Scheduler 模型文件仍为 192 的 13 模型 / 478 文件、229 的 28 / 601、232 的 19 / 491。

| 测试机 | 真实功能验收 | 最后稳定性 |
|---|---|---|
| 192 | Voice 直调/网关 TTS、ASR，512 字符 TTS / 20 秒 ASR 边界；连续试聊、停止后下一次调用；脱敏 NER、OCR；受限轻量 RAG；V-Sub 六项含取消与重启恢复 | 300.7 秒、1088 次成功探测，五容器健康，无新增重启/OOM/探测错误 |
| 229 | Voice 旧语音、说话人、网关 TTS/ASR；四个音乐模型实际 WAV、网关 FLAC；自动绑定完整 11 模型；真实 VOS UI 多语言、生成、播放、下载及边界拦截 | 至少 300 秒、21 次采样，五容器健康，无新增重启/OOM，预约归零 |
| 232 | Voice 同上六项正例及输入越界拒绝；V-ChatCut 连续 JSON → SSE；脱敏 NER、OCR；完整 RAG 精确事实验证；V-Sub 六项 | 300.2 秒、1144 次成功探测，五容器健康，无新增重启/OOM/探测错误 |

192/232 的原生无效输入不产生许可；网关校验拒绝涉及的许可全部释放。真实 RAG 测试使用独立测试知识库、会话和 agent，测试后清理，未删除用户模型。Orin CPU Voice 仅批准匹配校准的 Kokoro / SenseVoice，未将 Paraformer 或其他模型据此放行。

232 首轮 V-Sub 重启测试的脚本误将就绪期间 HTTP 503 当成终止错误；保留该失败记录，改为有界等待新 worker PID 与 generation 后，六项复测全通过。此修改仅属于测试脚本，没有降低应用预算或绕过准入。

音乐 HTTP 客户端断开时，服务保留许可直到原生工作退出，随后释放；后续请求实测成功。这不是立即终止原生音乐计算的保证。上述范围不包含 Image Studio 当前不可用的 P1 环境问题，也不代表未测模型或其他硬件 profile 已获批准。

## 0.0.37 接入地址与 0.0.38 未认领许可恢复（2026-10-19）

应用内「API 文档 → 应用接入 / 模型管理 API」、README 和统一接入指南均已区分正式、其他正式及兼容 host alias，覆盖 Compose 实际声明的全部 11 个别名与九组映射端口。0.0.37 曾在三台安装并通过地址、当前镜像、界面资源及原模型文件保留核验。

0.0.37 最后复测中，232 的 Voice 准入请求约八秒后返回 503，产生一条过期 suspect 许可。以专属普通数据库角色只读核验，该许可从未写入 `execution_id`；可信 Voice 0.0.21 在成功原子认领前不会启动模型推理。原始失败和数据库事实均保留，没有手动清除该许可。

0.0.38 加入严格限定的未认领直调恢复，不调整资源预算或放宽模型授权。常规 Rust 86 项通过；另在一次性 PostgreSQL 上实际执行恢复边界和并发认领集成测试：保留已认领、其他应用/模型/节点/上游、异步任务、关联/委派及未知元数据；认领先提交则不回收，回收先提交则迟到认领被拒绝。GitHub Release 与商店作业均成功（[CI 39215279435](https://github.com/ictrektech/V-Scheduler/actions/runs/39215279435)）。正式包 SHA-256：`688b765e6ab25e5ccfa32d5e65c82f3f1faf2fc4ee32475bf306ae2e73574c8d`。

| 测试机 | 0.0.38 当前验收 |
|---|---|
| 192 / l4t | VOS 升级 Completed，镜像、About、所有别名/九组端口及 13 模型 / 478 文件核验通过；完整六组 P0 回归全部通过、21 条新许可全部释放；300.6 秒 / 1088 次探测，五容器健康，无探测错误、新增重启或 OOM |
| 229 / thor-spark | 升级及 28 模型 / 601 文件保留核验通过；Voice 0.0.21 的语音/说话人、四音乐模型与网关格式、输入越界、断连后保留预约直到原生退出并释放、下一次生成均通过；301.91 秒 / 21 次采样，五容器健康、许可归零、无新增重启/OOM |
| 232 / amd | VOS 恢复安装 Completed，发布镜像、About、全部别名/九组端口及原 19 模型 / 491 文件核验通过；原未认领许可自动释放。Voice、连续 JSON → SSE 试聊、脱敏 NER、OCR、完整 RAG 精确事实及 V-Sub 六组回归通过，24 条新许可全部释放；300.8 秒 / 1144 次探测，五容器健康，无错误、新增重启或 OOM |

232 首次升级的停止旧容器活动五次超时，VOS 补偿撤销安装记录。内核阻塞日志早于此次升级，不能据此归因于 0.0.38。后续复查 Docker 查询和端口已恢复，核验没有运行计算后，仅停止原 Scheduler 四个容器，通过 VOS 应用商店恢复安装 0.0.38，沿用原存储卷、profile、公共目录引用和安装参数；未重启宿主机或 Docker，也未删除容器数据。原 suspect 许可由新版自动回收，证据为 `vvoice_expired_without_execution_claim`，没有手动清账。上述记录不把八秒 RPC 卡顿的底层原因视为已经查明，也不把未认领恢复推广到已经启动的计算。

192 / 229 仅清理可重建的编译缓存及 Docker 未使用构建缓存，未删除模型权重、应用数据卷或回滚镜像。

## 0.0.40 单一正式 host alias（2026-10-19）

统一容器正式名称统一为 `v-scheduler`：`8080` 为模型与网关 API，`38120` 为调度管理，`6005` 为模型仓库。其余七个统一容器名称全部标为兼容别名，仍可访问；三个独立 Ollama 保留各自名称。新应用和已有应用后续适配均推荐使用 `v-scheduler`。应用内四个 API 文档标签页、README、统一接入指南、模型下载示例及包内三种语言安装说明保持此规则。网关总览与 API 文档复用运行时地址数据，外部端口读取当前安装配置。

代码改动的 Rust 常规测试 86 项、前端测试 23 项、类型检查和前端构建通过。AMD / ARM 镜像均完成发布及飞书记录回读。0.0.39 发布界面与地址修复；0.0.40 仅补齐包内安装说明，复用同一组已验证镜像。0.0.40 的 [Release 与商店工作流](https://github.com/ictrektech/V-Scheduler/actions/runs/39225456391)均成功，正式包 SHA-256：`ffa4406dc72382b474d94701ae9aae66a124ab76e3ff5e66ee9ee6483be9d539`。实际包校验单一内层归档、模板替换、必需文件与三种语言的新旧地址。

在 232 实际安装的页面通过只读本地转发逐项切换四个 API 文档标签页，核验正式/兼容分类及推荐说明，中英文均通过；最终 0.0.40 页面和全局「关于」核验通过，关于显示应用版本与四项 `name:tag` 镜像，无完整仓库 URL。桌面截图、页面文字与无 error/warn 的浏览器日志已保存；未将静态 bundle 检查替代页面验证，也未据此宣称完成移动端或长期稳定性测试。

192、229 的首轮固定模型请求紧接启动返回 404；后续发现目录可见后，使用相同正式入口复测，普通及 SSE 响应均含正文、SSE 完整结束且预约释放。首轮响应正文未保留，不将其底层原因写成已证实。232 的首次升级在 `compose-start` 失败，VOS 补偿撤销记录；核验旧实例空闲后，仅停止原 Scheduler 四个容器，使用原卷、配置及共享目录通过 VOS 恢复安装，没有重启宿主机、Docker 或删除权重。完整 P0 功能与五分钟稳定性结果保留在前述 0.0.38 验收中，本次针对地址、文档与同镜像包升级执行下列复测。

| 测试机 / profile | 保留模型 / 文件 | 正式入口普通 / SSE 响应 | 最后健康检查 |
|---|---|---|---|
| 192 / l4t | 13 / 478 | 均有正文，2.89 / 2.5 秒，两条新许可释放 | 60.6 秒、220 次成功探测，五容器健康，无错误、新增重启或 OOM |
| 229 / thor-spark | 28 / 601 | 均有正文，33.1 / 1.61 秒，两条新许可释放 | 60.5 秒、228 次成功探测，五容器健康，无错误、新增重启或 OOM |
| 232 / amd | 20 / 500 | 均有正文，50.49 / 2.03 秒，两条新许可释放 | 60.3 秒、228 次成功探测，五容器健康，无错误、新增重启或 OOM |

三台实际镜像与发布的 manifest/config 相符，原 profile、存储位置及共享目录引用保留，正式入口与全部兼容入口均通过 HTTP 检查。232 的表格基线为本轮升级前已存在的 20 模型 / 500 文件，包含此前 19 / 491 的原文件；未删除或重新下载原权重。操作证据位于维护端 `~/dev/build/V-Scheduler/p0-20261019/`，`final38-*` 与 `final40-*` 分开保存，避免把不同版本的测试混为同一轮。

## 历史模型仓库合并验收（0.0.23）

原 `model-hub-merge.md` 的发布记录归并至此；当前功能与入口以[模型管理](model-management.md)和[统一接入指南](application-integration.md)为准。0.0.23 完成原模型 registry、flock、选中文件下载/追加/恢复、导入回退及共享目录接管；原文件 inode 与内容保留。使用原 Ollama 子模块和 Feishu `ollama_server` 列，无独立 Python/React 服务或重复试聊。

当次 AMD/ARM Rust 各 34 项、模型 API 生命周期各 46 项、Vue 10 项、打包 15 项通过；浏览器覆盖文件选择、推理卡片、统一文档、三语言与全局关于。属于隔离 fixture 验证，未证明已安装机器或 DGX 原生推理。当前版本的回归结果应另记，不沿用这些旧数量。

[0.0.23 Release](https://github.com/ictrektech/V-Scheduler/releases/tag/v0.0.23) 对应 Actions `38834563141`，Release 与 Store 成功，包 SHA-256 `c5b169dc19861abc66470abb0a0fb9b4557f8790e5e84774f62af4f60d97b7ac`。0.0.22 因飞书链接单元格解析而未发布；已修为读取 `text`，没有修改共享表格。共享目录与回退边界继续见[统一应用](unified-application.md)。

## 2026-10-20 就绪、准入错误与地址迁移回归

候选代码在原生 AMD / ARM 分别通过 92 / 93 项 Rust 测试（各另有一项需独立 PG 环境的测试）；该 PG 测试已在一次性 PostgreSQL 18 中单独通过，覆盖 0.0.21 / 安装参数迁移版 0.0.22 的未认领许可恢复、幂等和并发认领屏障。打包契约 16 项通过。

真实 Rust 进程完成延迟模型发现、目录就绪、后端故障与恢复循环，存活检查独立；准入响应正文卡住的回归用例先在旧代码四秒超时，再在修复代码三秒内返回明确错误。统一 API 隔离验收覆盖身份、私有注册、SSE、PG 故障隔离和恢复；模型仓库 41 项下载/更新/导入/接管检查通过。浏览器候选 UI 以合成状态验证中英文初始化、后端不可用提示和恢复后的清除；未用这些 fixture 代替测试机真实推理验收。

地址迁移使用固定来源白名单、能力协商和原凭据递增 revision；不以 DNS 名称变化扩大执行权限或资源预算。V-Voice 0.0.22 只改变安装参数并复用经核验的六个 0.0.21 原生镜像；实际发布包、安装版本和运行配置的最终核验见下节。

## 0.0.41 发布与三机回归（2026-10-20）

[0.0.41 发布与商店工作流](https://github.com/ictrektech/V-Scheduler/actions/runs/39353703170)成功；官方包 SHA-256 为 `423bd53e044ee5f0ddb332a231649e43a8e6cbff20e3b6a6ee2756d75b0c0f39`。AMD / ARM 统一镜像分别为 `amd_20261020` / `arm_20261020`，原 Ollama 镜像沿用已发布的各 profile 版本。实际包和运行容器均核验正式 / 兼容别名、九项内外映射端点、中文 / 英文地址数据以及全局关于中的四项 `name:tag`。

同批客户端发布并经 VOS 安装流程升级：V-Voice [0.0.22](https://github.com/ictrektech/V-yovoice/actions/runs/39355088938)、V-Sub [0.0.8](https://github.com/ictrektech/V-Sub/actions/runs/39355955191)、V-ChatCut [0.0.29](https://github.com/ictrektech/V-ChatCut/actions/runs/39355954882)、HybRAG [0.1.74](https://github.com/ictrektech/WeKnora/actions/runs/39358968034)。各工作流的 GitHub Release 与应用商店发布均成功。V-Voice 六个原生镜像与 0.0.21 完全一致；HybRAG 的 DocReader、沙箱、Redis 和 Neo4j 镜像引用与 0.1.73 一致。V-Sub、V-ChatCut 和 HybRAG 受影响镜像已完成发布、飞书回读和安装后 registry manifest/config 对照。

| 测试机 | Scheduler / Voice / Sub / ChatCut / HybRAG 原 profile | 保留模型 / 文件 |
|---|---|---|
| 192 | l4t / l4t / l4t / l4t / arm | 13 / 478 |
| 229 | thor-spark / thor / thor / thor-spark / arm | 28 / 601 |
| 232 | amd / amd / amd-cuda / amd-with-cuda / amd | 20 / 500 |

三个节点均保留原安装参数、存储路径和公共目录引用，没有重下载或替换原权重。真实统一入口普通 / SSE 请求均返回正文及完整终态，每次调用只产生一份许可并释放。三机 V-ChatCut 登录、模型发现、普通与流式调用通过；V-Sub 直调、网关、输入边界、设置变化保护、取消后原生进程停止及 worker generation 重建通过。新 worker 等待可信绑定时的精确 503 先确认没有新增许可，再在绑定完成后发起计算，不把未知执行错误作为可盲目重放的请求。

V-Voice 三机 TTS / ASR 真实输出通过。192 / 232 另完成直调与网关、512 字符 TTS、20 秒 ASR，共六项正向许可，以及六项越界 / 非校准设置拒绝；直调负例不预约，网关负例释放已取得的许可。资源分别为 1792 / 4096 MiB、GPU 0。Thor 四项原生音乐审批及正式地址绑定保留；本轮没有重新生成全部音乐模型，不把旧版本的音乐音质或峰值测试冒充新测量。

HybRAG 三机真实 VOS 登录、私有网关身份、入库、解析、检索与 RAG 答案包含合成事实均通过；临时知识库、会话及轻量测试智能体已清理，许可全部释放。192 使用原有已校准轻量 QA / 向量配置；229 / 232 另通过 QA、向量、重排调试。新建 `v-scheduler` 正式地址模型的实际回复、模型删除与原模型 ID 集合保留通过。232 一次冷重排调试触发客户端 60 秒截止时间，许可释放；保留该次记录后，三类调试复测均成功，不将冷加载延迟写成容量拒绝。正式 RAG 结果独立记录。

### 升级失败、重试与数据恢复

平台侧补丁在 232 单独验证：完整 Rust 测试 4332 项通过、6 项跳过。就绪收敛核对最新安装快照；旧 project 清理不影响新 project 或共享目录；失败补偿保留 Error 安装记录及参数。兼容启动兜底增加连续状态观察，拒绝启动后异常退出，保留退出码 0 的一次性任务语义。真实临时 VOS 应用完成“失败升级 → 可见 Error → 同版本重试成功 → 原配置 / 数据标记保留 → 卸载清理”，不是仅凭模拟测试或 start 退出码报告成功。

232 更换平台后台时核验环境变量、挂载及其他 22 个应用状态保留；未重启宿主机或 Docker。192 / 229 的平台后台保持原安装版本，应用均通过 VOS 升级。该平台补丁不属于 V-Scheduler 应用包；不能把应用升到 0.0.41 等同于其他设备也已更新平台后台。

229 的 HybRAG 重启暴露 Redis AOF 格式错误，应用因 Redis 未启动而失败。先用只读原件和隔离副本复现；标准截断方案未用于实际数据。仅移除命令边界之外的无效零填充，完整命令与事务保持，Redis 校验和隔离启动通过；保留原始目录后启用验证副本，再通过 VOS 原参数安装及实际 RAG 验收。未据此认定填充产生的时间或底层原因。

### 验收边界与最终稳定性

旧兼容应用按已配置范围单列：NER / OCR 三机通过；独立 V-Music 在 229 / 232 生成有效 WAV；Image Studio 在 229 生成真实 PNG 并删除本次任务。192 的独立 V-Music 缺少绑定、Image Studio 缺少 SD-Turbo 权重；232 的 Image Studio 缺少匹配绑定。这些组合保持拒绝，不记为输出成功，也不静默下载权重或借用其他 profile 审批。

最终五分钟窗口检查每机 13 个本批应用容器、目录就绪、活动 / suspect 许可及重启 / OOM。192：300.13 秒、60 次采样；232：300.57 秒、57 次采样，均健康且无窗口内重启、OOM 或遗留预约。229：300.15 秒、60 次采样通过。每机统一 Scheduler 镜像及九个客户端组件均与当前 registry manifest/config 匹配；三个 Ollama 原镜像 ID 在升级前后保持一致。此为有界回归与稳定性检查，不是所有模型、任意输入或长时间浸泡认证。

HybRAG 接入改动经当前主分支合并结果的 Go race、完整前端构建、真实组件交互、九项启动配置测试及 GitHub 格式 / vet / 测试 / lint / Docker 检查后，[PR 1](https://github.com/ictrektech/WeKnora/pull/1) 已合并。0.1.74 的公开 tag 与已核验发布基线未移动，不将主分支并行上游功能声称为本次已部署内容。

删除已被统一指南和本验收记录替代的模型仓库合并旧文档、会话迁移协同契约；当前操作统一从[应用接入指南](application-integration.md)进入。原始证据按版本保存于维护端 `~/dev/build/V-Scheduler/p1-20261020/`，与此前 P0 记录分开。

## 推理排队、模型驻留与服务关联候选验收（2026-10-20）

本节记录功能候选的源码和隔离运行验收，安装发布结果另行记录，不将模拟后端当成实际模型输出。

- AMD Rust 98 项、ARM Rust 99 项通过，各保留 1 项需专用数据库的历史跳过项；Vue/HTTP/流解析等 23 项、类型检查和生产构建通过。
- 实际统一 Rust 进程与独立 PostgreSQL 的 45 项新增场景通过：交互/后台优先级、等待老化、原生任务领取、队列身份隔离、提前取消、八条后台等待不占满执行槽位、原始请求不重放、空闲模型预算保留、同模型流式复用、模型切换、卸载失败保留 suspect、原生恢复、策略持久化、服务卡片准入预热和停止，以及旧客户端直接申请许可时协调卸载、驻留模式未传排队头的并发请求自动等待、显式 0 保持立即拒绝及独占执行；运行中禁止删除、下载期间禁止新推理、更新前确认卸载及预算刷新、删除后立即移出网关目录。
- 统一 API／身份／PGV 故障恢复回归通过。模型仓库 41 项通过，覆盖旧目录接管、选中文件及追加、更新失败保留旧版本、归档、任务和服务接口；发布脚本六种协议模拟场景通过，没有在构建机执行真实 Git 操作。
- 232 使用现有 Ollama 镜像及原共享目录的只读挂载，在独立网络、数据库、限额 CPU 容器中完成真实 `qwen3.5:2b` 普通与 SSE 输出，两次使用同一驻留预约；空闲策略关闭后原生运行列表清空，控制器取得原生停止屏障证明后释放预约。原安装实例启动时间未改变。重复验证一次普通 / SSE 分别约 5.98 / 1.44 秒；宿主文件缓存已热，不将此作为冷启动性能基准。更早的首次请求约 57.74 秒。
- 浏览器候选使用合成 API 数据，桌面 1440×1080 与移动 390×844、简中/英文/繁中交互通过，覆盖驻留设置保存、所选文件范围提示、精确权重关联、等待项取消、试聊排队和停止、统一指南及全局关于的四项 `name:tag`；未发现相关页面异常或控制台错误。此项不代替已安装 VOS 登录与原生业务验收。

驻留最多保留一份空闲预约，继续占用完整保守预算；配置就绪不等同当前容量可用。外部后端驻留由其运行时负责，真实远端节点控制仍不在本轮范围。原始证据保存在维护端 `~/dev/build/V-Scheduler/features-20261020/`。


## 0.0.45 发布与实机验收（2026-10-20）

[发布与应用商店流水线](https://github.com/ictrektech/V-Scheduler/actions/runs/39395161254)均成功。[官方安装包](https://github.com/ictrektech/V-Scheduler/releases/tag/v0.0.45) SHA-256：`17a139769174a9ca45613bb32f36dfaf7c091491a19d86a0eb7a04f84f3f882b`。已核验外层仅一个 `app.tar.gz`、必需文件齐全、模板与镜像引用已渲染；安装后统一镜像与发布的 manifest/config 摘要对应。三个原 Ollama 镜像 ID 均未变化。

真实 VOS 验收补齐两条隔离测试不足的路径：管理员权限从现有 `/v1000/user/check` 可信校验取得，不能从 OIDC 用户名推断；开启驻留时，已验证身份调用内置 Ollama 且未提供排队头，默认最多等待 30 秒，以协调既有应用的并发和模型切换。显式等待 0、关闭驻留、匿名和其他后端保持原行为。所有路径仍执行原有预算、身份、节点及原生停止屏障检查，没有用重放推理或放宽资源限制取得成功。

三机均通过 VOS 应用升级，保留原 profile、安装参数、公共目录引用、权重文件、正式/兼容别名、九项映射端点和关于中的四项 `name:tag`。应用内简中/英文/繁中指南的实际运行资源包已核验包含新的默认等待规则。

| 测试机 | 原 profile | 保留模型 / 文件 | 实际新增功能 | 五分钟稳定性 |
|---|---|---|---|---|
| 192 | l4t | 13 / 478 | 8 项通过 | 300.14 秒 / 60 次采样 |
| 229 | thor-spark | 28 / 601 | 8 项通过 | 300.15 秒 / 60 次采样 |
| 232 | amd | 20 / 500 | 8 项通过 | 300.32 秒 / 58 次采样 |

每机八项实际功能检查包括普通回复、SSE 正文及完整终态、同模型复用单份完整预约、共享权重到网关及调度绑定的可调用状态、提前取消不产生新计算、等待项不遗留，以及原生卸载确认后释放全部预约。驻留策略在测试后恢复到原 60 秒。每机 13 个相关应用容器在独立五分钟窗口内均无重启、OOM 或遗留活动/suspect 预约；此为有界检查，不是长期浸泡或所有模型的性能认证。

229、232 的 V-ChatCut 登录/模型发现/普通与流式调用、V-Voice TTS/ASR、V-Sub 直调/网关/输入边界/配置保护/原生取消/worker generation 恢复，以及 HybRAG 入库/解析/检索/答案均通过。HybRAG 在默认驻留 60 秒下验证多模型并发交接，最终答案包含合成校验事实，每机六份许可全部释放；本轮的知识库、会话已删除。未更换这些客户端的版本或配置。

192 的 V-ChatCut、轻量已校准 QA/向量 RAG 和新增调度功能通过；语音及字幕正向复测没有计作通过。语音请求时观测可用内存 3820 MiB，已批准的 TTS 预算加有效安全预留需要 3840 MiB；字幕请求时可用 3793 MiB，需要 4096 MiB。语音的直接准入诊断确认 `WAIT_MEMORY_HEADROOM`，拒绝后没有活动或 suspect 计算遗留。没有降低预算/安全预留、停掉其他应用、删除现用模型或绕过准入。该机需在内存余量恢复后补跑这两项正向验收；此前版本的正向结果仍按原日期保留，不能代替本轮结果。

原始分版本证据保存在维护端 `~/dev/build/V-Scheduler/features-20261020/`，最终三机汇总为 `final-acceptance45.json`。源代码、镜像/飞书记录、官方包、实际安装与功能/稳定性证据分别核对，未把触发打包或模型列表可见当作推理成功。

## 统一应用生命周期接入验收（2026-10-21）

本次新增调度命名空间 `/integrations` 登记/查询/安全注销、应用周期心跳及同步 permit 停止请求/回执。复用已有节点采集、批准预算、原子验票与异步任务取消；不新增容器或要求用户部署 Agent。已有自动绑定兼容保留。

- AMD Rust 98 项、ARM Rust 99 项通过，各有 1 项既有专用数据库测试忽略；前端 23 项、类型检查及生产构建通过。
- `scripts/e2e-application-lifecycle.py` 在两个架构各通过 44 项：真实 Rust/隔离 PostgreSQL，验证禁止自批预算、独立节点/应用新鲜度、应用隔离、revision、重启恢复、停止重投、迟到认领拒绝、错误 nonce/执行/令牌拒绝、卸载前不释放、原完成接口不绕过停止、回执幂等与安全注销。原生后端为受控合成服务，不冒充真实多机模型验收。
- 既有 `e2e-inference-management.py` 的 45 项排队、默认驻留、并发交接、停止证明及权重更新/删除保护通过；`e2e-api-contract.py` 的统一身份、协议与故障恢复通过；`e2e-model-hub.py` 的 41 项通过。
- Browser 插件未提供，使用构建主机已有 Playwright/Chromium 验证真实编译界面和合成 API：1440×1080 与 390×844、简中/英文/繁中，节点/应用/预算同屏、点击停止后保持待核验、统一指南和新旧地址可见，无页面运行错误。截图留在本任务构建目录，不加入源码。

本次不宣称其他应用已经全部切换新协议；未注册唯一停止适配的执行返回 `STOP_ADAPTER_NOT_REGISTERED`，不会强行杀进程或释放预算。四台 Spark 的 ModelInfer 管理适配仍须完成真实执行、停止和故障恢复验证；观察节点不作为可预约资源。发布不增加 DGX Agent，不改旧共享模型目录和原生 Ollama 镜像。
