# V-Sub 私有 worker 可信绑定（开发中）

此功能尚未随 `0.0.26` 发布。控制器、网关和模拟 Root 的隔离协议验收已通过；L4T Whisper 的原生计算与取消已测量，最终客户端 Root worker 的真实绑定及正式包安装验收仍待完成。普通用户只需通过 VOS 安装应用，不需要部署额外 agent 或逐模型手工配置。

## 发现与身份

控制器核验自己的 VOS context 属于已启用的 `com.ictrek.v-scheduler`，并确认同一 VOS 已安装 `com.ictrek.v-sub`。只向固定服务 `http://v-sub-backend:7090/api/v1/scheduler/discovery` 请求现有 worker 快照；发现不启动 worker、不加载模型，也不接受客户端提交资源预算。

每个模型同时声明原生 ID、完整 64 位十六进制 namespace、独立生成的 32 位 worker generation，以及固定上游 `http://v-sub-backend:7090/inference/<namespace>/v1`。显示模型名中的 12 位前缀不能替代完整 namespace。worker 重启会生成新的 generation，从而使用新的执行器身份；重新绑定先等待旧计算停止。

网关只向拥有该用户上游的 VOS 用户开放模型。加载私有绑定时，还会核对 namespace 等于 `SHA256("v-sub" + NUL + VOS subject)`，上游 ID 等于 `app-v-sub-` 加 subject 的 SHA-256；不能用手工 API 上游借用私有授权。凭据不出现在发现结果、业务用户 API、日志或共享模型目录。

## 绑定与调度责任

控制器只将绑定交给固定 `/api/v1/scheduler/binding`，使用可信 VOS context 验证。绑定包含版本、revision、固定调度 URL、独立应用 token、模型执行器、网关执行器、namespace、generation、上游、完整运行设置和输入范围。Root 核对实际存活 worker、模型和 generation，再原子保存自己的私有状态；不将服务凭据分发给业务用户。

直接调用由 Root broker 为计算申请、续租和释放许可。经网关调用时，Root 核验并认领同一次网关许可，不重复预约。每次计算核对完整输入指标和运行设置，超出校准范围时先行拒绝。worker 退出却无法确认原生计算停止时，仍保留停止未知的资源预约。

控制器私有状态为 `/config/app-bindings/v-sub.json`；网关绑定快照为 `/data/vsub-scheduler.local.json`。文件须为 0600 普通文件，读取上限 256 KiB，网关按请求读取原子快照。管理员覆盖校准文件为 `/config/vsub-approvals.json`；它用于发行方实测和管理维护，不是安装者必须填写的参数。

## 当前实测模板

内置 `config/vsub-calibrations.json` 仅批准 `l4t` / aarch64 / Orin 统一内存的 `builtin/tiny-q5_1`，使用 CPU whisper.cpp、完整默认参数和注册模型导出路径。预算为 4.5 核、2 GiB RAM、不占 GPU；输入最多 1 MiB、20 秒、不附加 prompt。模板中的完整运行设置必须匹配，其他模型、后端或 profile 不自动借用该预算。

模型路径通过共享仓库中已就绪注册文件与实际 realpath 一致性核验后，规范为稳定导出引用；相同文件名不足以取得授权。共享目录仍由安装者选择。

正常计算证据 `vsub-tc192-de26b5081ef4462a8da51518d56a0cc1` 为真实 20 秒音频、8 段非空字幕；取消证据 `vsub-tc192-f9651eab75914f8ebd99362284db16fe` 确认原生进程退出与许可释放。CPU 观测最大 4021 millicores，保守内存观测合计 839 MiB。该内核不提供 cgroup `memory.peak`，采样值不能表述为精确内核峰值。

`scripts/e2e-vsub-bindings.py` 验证私有用户隔离、一次网关认领只占一份许可、直接调用，以及旧许可存在时等待 generation 变更。它使用模拟原生计算，不代替最终客户端的真实 Whisper 验收。
