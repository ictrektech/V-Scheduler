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

未完成的部署验证：实际 VOS 只读认证/版本/节点拓扑，多节点 Agent projects 安装，ARM64 GPU 采集器验证，以及 VOS 安装。本次未升级任何已安装应用。
