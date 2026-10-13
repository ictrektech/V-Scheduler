# 节点发现与资源管理

V-Scheduler 同时提供任务调度与节点资源管理。VOS / Cluster Manager 继续拥有设备与集群成员身份；V-Scheduler 只读复用其接口，在自己的数据库中管理执行准入、预约和节点排空，不修改平台或 Cluster Manager。

## 数据来源与职责

| 数据 | 现有来源 | V-Scheduler 用途 |
|---|---|---|
| 集群节点身份、名称、角色、地址 | Cluster Manager `GET <应用前缀>/api/nodes`，`data.nodes[]` | 自动发现、关联已有调度节点、展示仅观察节点 |
| CPU 逻辑核数、内存/磁盘容量 | 节点 VOS `GET /v1000/nas/device/info` | 自动读取物理容量；不覆盖管理员批准的调度预算 |
| CPU 使用率、内存/磁盘使用 | 节点 VOS `GET /v1000/nas/device_usage` | 显示实时观测与采集时间；CPU `used` 为百分比，RAM/disk 单位为字节 |
| 实际内存可用量、设备 UUID、显存总量/可用量、GPU 利用率 | 本应用 Rust Agent，Linux `/proc` / `nvidia-smi` | 调度前压力检查、真实 GPU 监控 |
| 预约、执行许可、租约、失联隔离 | V-Scheduler PostgreSQL | 分配资源的权威账本 |

源码已核验：`vos-cluster-manager/backend/src/handlers/nodes.rs::node_list`、`etcd/dto.rs::NodeRecordView`；`vos-platform-backend/src/domain/nas/router.rs::device_info_detail` / `device_usage`。VOS NAS 设备状态接口的 GPU/temp/fan 是占位值，不能当成真实测量。本版 NAS 适配器将 GPU 标为未知；真实 NVIDIA 数据来自 Agent。平台另有 `GET /v1000/device/get_gpu_percentage` / `get_gpu_percentage_by_id?id=<Prometheus-instance>` 及历史接口，查询 `DCGM_FI_DEV_GPU_UTIL`；配置相应 exporter/Prometheus 后可以返回真实利用率。但 `vector_to_used_size` 对无样本也返回零，by_id 仅取第一个 sample、未暴露采集时间和多卡标识，不能将其零值当作已证明空闲，也不能用它决定显存预约。Prometheus instance 与 VOS member ID 需明确映射；本版尚未接入这一路来源。其他加速器需另做采集器与目标机器验证。

## 当前实现与配置

已加入 controller 只读轮询适配器、观测缓存、`GET /api/v1/nodes` 合并视图、Agent CPU/GPU 利用率采集和界面实际使用行。未配置 VOS 来源时，原有独立节点运行方式不变。

在 controller 配置中增加以下字段（示例 URL 必须替换成目标部署的实际入口）：

```json
{
  "vos_cluster": {
    "url": "https://vos.example/app/com.ictrek.vos-cluster-manager",
    "token": "<read-only-access-token>"
  },
  "nodes": [{
    "id": "gpu-node",
    "vos_member_id": "<cluster-member-id>",
    "vos": {
      "url": "https://gpu-node.vos.example",
      "token": "<node-read-only-access-token>"
    },
    "capacities": {
      "cpu_millicores": 4000, "memory_mib": 16384,
      "gpu_slots": 1, "gpu_memory_mib": 16384
    }
  }]
}
```

片段需合并进完整配置，保留 principals/executors。来源配置是受保护的服务端文件，不通过公开 API 或 UI 返回。当前支持来源 Bearer 认证；实际 VOS 版本若要求其他认证方式，需要接入经授权的只读代理，不能把登录密码放进 URL。采用服务端轮询，不复用浏览器登录 cookie，不直接读 etcd 或平台数据库。

Cluster Manager URL 指向应用根前缀，适配器追加 `/api/nodes`；节点 VOS URL 指向平台根前缀。节点明细端点需逐节点明确配置：不会跟随清单中的 `node_ip` / `api_endpoint` 自动发送凭据。清单是自动读取的，远端设备监控首次需要配置可信入口与访问权限；不能宣称任何网络上的节点都能无配置访问。

轮询每轮完成后等待 30 秒，每个请求最长 5 秒，响应最多 2 MiB，不跟随重定向。采集超过 90 秒或最新请求失败标为过期；保留上次成功值与时间，不伪造零使用率。Agent 心跳使用独立的 12 秒新鲜度判定；VOS 监控不能代替 worker 心跳。

## 节点生命周期与管理边界

1. 清单自动发现的新节点以 `vos:<member_id>` 展示，`managed=false`，调度容量为零，不授予凭据、不自动执行任务。
2. 管理员通过 `vos_member_id` 映射已有调度节点，配置预算、worker 身份和执行器。在目标节点部署本应用 Agent 后才具备执行条件。
3. 当前部署为 Rust Agent 程序或服务容器，持久化 journal；只访问 controller 和已注册执行器。后续 VOS 安装包可复用本应用自己的 projects 模板部署，不改 Cluster Manager 的项目。
4. “排空”停止接收新预约，已执行任务按协议完成或取消。监控不可达、清单删除或 Agent 失联不会自动删除任务或释放未确认停止的预约。
5. 成功清单中消失的未纳管节点不再展示；已配置节点保留，`vos_present=false`。此版本的清单变更是观察信息，不自动撤销人工配置的执行授权；Agent 失联仍会阻止新任务领取。后续可加入显式“必须在 VOS 集群内”准入策略。

不会在资源页提供修改 VOS 集群角色、卸载基础设施、SSH 命令执行或删除真实节点等操作。

## 资源视图与分配规则

界面分开显示物理资源/实际使用、已批准预算、未释放预约。系统保留量由管理员从容量中预先扣除，例如 manager 节点为 VOS、数据库等保留 CPU 和内存。常驻模型需保留显存预算，在线推理与批任务的预约共享账本。

准入必须同时满足预算剩余量与 Agent 最新测量的压力约束。不能简单计算“实时使用 + 已预约”：已执行任务可能同时出现在两者中，会重复计数。CPU 使用率不是可预约核数；RAM used 不是 MemAvailable；VOS GPU 占位零不是空闲显存。未接入应用只被观察，不能被调度器强制排队。

本版预算仍由管理员配置，不自动随物理容量扩大。自动生成保守预算建议、系统保留配置、CPU/RAM 预约汇总、每设备多 GPU 预算、VOS caps/profile 展示、自动 Agent 部署和资源历史曲线进入后续阶段，不能混同为当前已支持能力。

## 验收与实施顺序

- 当前增量：解析真实 DTO 形状，单位转换正确，未知 GPU 保持未知；脱敏字段白名单；来源失败保持缓存；发现节点不能获得执行权限；既有任务/预约行为不变。
- 部署核验：在实际 VOS 版本上验证只读认证、应用前缀、节点 ID 映射及监控数据，与同机 Agent 采样对照。源码和模拟契约测试通过不代表已完成这一项。
- 下一阶段：VOS projects Agent 部署、节点接入向导、预算/系统保留编辑、按设备资源、外部负载保护与资源历史；分别验证 AMD、ARM、L4T/Thor，其他加速器按采集器逐项验收。

此增量不改变任务重试和删除规则：先确认旧执行停止才重试；仅已停止的终态任务允许人工软删除，输出文件保持原应用管理。

## 应用与节点的部署拓扑

资源归属至少表示为 `VOS member → scheduler node → installed_app_id/project instance → executor → device`。`app_id` 是产品身份，不是运行实例，也不是设备位置。一个 app 可在多个节点安装，前端、controller、worker、模型服务可能分布在不同节点；profile 仅表示该实例选用的配置。

当前只读适配器在明确配置 VOS 入口的节点调用 `GET /v1000/app/list`，解析 `data.total` / `data.data[]`，保存本机的 app ID、installed_app_id、版本及管理状态，在节点视图展示并列出本机批准的 executor。保留列表采集时间；失败不清空缓存；完整性不符拒绝更新。只读身份至少需要 `platform:app:read` / `platform:app:list` 之一。不会保存 owner、configs_json、安装路径、错误原文或凭据。

安装状态不等于容器运行状态，更不等于执行器可接任务。下一阶段通过 `GET /v1000/app/projects/list`（`platform:app-project:read`）补充本机子项目实例，再按需用 projects/status 获取 Docker 实况。这些是每台 VOS 的本机视图，需携带来源节点合并。普通应用运行状态需要另核验 app/status 契约；本版不会把 app/list 的管理态标成实际运行态。

业务执行器的资源位置必须由管理员绑定或经已认证 worker 注册确认，不能因为应用安装在 A 就假定其请求消耗 A 的 GPU。Image Studio 的本地生成使用其 executor 所绑定节点；V-Router 云端 upstream 不占本地预约，本地 upstream 的许可配置须绑定模型服务真正所在节点，delegated upstream 由执行器持有预约，避免在网关所在节点再次预约。

外部应用和手工运行的模型也使用同一 executor/upstream 绑定机制，不要求出现在 VOS 安装清单内。监控发现一个容器或 GPU 进程只证明外部负载，不能据此自动赋予任务执行权限。拓扑来源失效时保留位置及新鲜度，节点失联后停止新增预约，已运行任务继续走停止确认与故障对账。
