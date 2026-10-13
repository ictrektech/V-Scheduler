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
