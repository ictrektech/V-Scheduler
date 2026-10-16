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
