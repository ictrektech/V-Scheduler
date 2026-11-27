//! 部署配置实体：`modelinfer_deployments`
//!
//! 一行 = 一个部署配置（模型卡片 + 模型变体 + 节点 + head + 参数）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_deployments")]
pub struct Model {
    /// 部署配置 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 配置名称
    pub name: String,
    /// 已登记模型卡片主键 `<card_id>@<version>`
    pub plaza_model_id: String,
    /// 模型卡片 id（冗余便于展示）
    #[sea_orm(default_value = "")]
    pub card_id: String,
    /// 权重 id（manifest `model`）
    #[sea_orm(default_value = "")]
    pub weight_id: String,
    /// 模型来源：hf / ms
    #[sea_orm(default_value = "hf")]
    pub hub_source: String,
    /// model-hub 相对路径（如 export/hf/Qwen/Qwen3-0.6B/current）
    #[sea_orm(default_value = "")]
    pub hub_rel_path: String,
    /// 权重覆盖来源（`""` = 未覆盖；`hf` / `ms`）
    ///
    /// 覆盖是**实例级**期望：`weight_id` / `hub_source` / `hub_rel_path` 仍表示卡片默认权重，
    /// 生效权重由覆盖列优先。见 `specs/docs/design/modelinfer/runtime-overrides.md`。
    #[sea_orm(default_value = "")]
    pub weight_override_source: String,
    /// 权重覆盖 id（`<org>/<repo>`；空 = 未覆盖）
    #[sea_orm(default_value = "")]
    pub weight_override_id: String,
    /// 权重覆盖的 model-hub 相对路径（创建时解析，启动时重新解析）
    #[sea_orm(default_value = "")]
    pub weight_override_rel_path: String,
    /// 镜像覆盖（JSON 数组，条目结构与 manifest `images` 一致；空数组 = 使用卡片声明）
    #[sea_orm(default_value = "[]")]
    pub images_override_json: String,
    /// 额外环境变量（JSON 对象；在卡片 `configs` 之后注入）
    #[sea_orm(default_value = "{}")]
    pub extra_env_json: String,
    /// 节点 ID 有序列表（JSON 数组；首元素为 head）
    #[sea_orm(default_value = "[]")]
    pub node_ids_json: String,
    /// head 节点 ID（= node_ids[0]）
    #[sea_orm(default_value = "")]
    pub head_node_id: String,
    /// 卡片 configs 的用户取值（JSON 对象）
    #[sea_orm(default_value = "{}")]
    pub configs_json: String,
    /// 标准接入参数：对外模型名（留空自动推断为卡片权重末段）
    #[sea_orm(default_value = "")]
    pub serve_model_name: String,
    /// 标准接入参数：服务 API Key（空串表示不启用鉴权；平台生成并持久化）
    #[sea_orm(default_value = "")]
    pub serve_api_key: String,
    /// 标准接入参数：服务端口（默认 8000）
    #[sea_orm(default_value = 8000)]
    pub serve_port: i32,
    /// 标准接入参数：API URL 前缀（默认 /v1）
    #[sea_orm(default_value = "/v1")]
    pub serve_url_prefix: String,
    /// 统一网关注册状态：disabled / unregistered / registered / error
    #[sea_orm(default_value = "unregistered")]
    pub vrouter_state: String,
    /// 统一网关上游 ID（注册成功后回写）
    #[sea_orm(default_value = "")]
    pub vrouter_upstream_id: String,
    /// 统一网关模型 ID（注册成功后回写，如 registered/<model>）
    #[sea_orm(default_value = "")]
    pub vrouter_model: String,
    /// 统一网关最近注册 / 续约时间（CST ISO-8601）
    pub vrouter_registered_at: Option<String>,
    /// 统一网关最近一次注册失败原因
    #[sea_orm(default_value = "")]
    pub vrouter_last_error: String,
    /// 本地反代开关（运行期将本机 TCP 端口转发到 head vLLM；默认关闭）
    #[sea_orm(default_value = false)]
    pub local_proxy_enabled: bool,
    /// 本地反代期望端口（0 = 每次启动自动从 31000~31100 取空闲端口）
    #[sea_orm(default_value = 0)]
    pub local_proxy_port: i32,
    /// 本地反代实际监听端口（0 = 未运行）
    #[sea_orm(default_value = 0)]
    pub local_proxy_active_port: i32,
    /// 本地反代最近一次失败原因
    #[sea_orm(default_value = "")]
    pub local_proxy_last_error: String,
    /// 推理引擎类型（vllm / unknown / 空 = 未探测）
    #[sea_orm(default_value = "")]
    pub inference_engine: String,
    /// 推理引擎版本（可空）
    #[sea_orm(default_value = "")]
    pub inference_engine_version: String,
    /// 引擎来源：script / probe / unknown
    #[sea_orm(default_value = "")]
    pub inference_engine_source: String,
    /// 推理指标注册状态：unregistered / registered / error / unsupported
    #[sea_orm(default_value = "unregistered")]
    pub metrics_state: String,
    /// 推理指标 consul 服务 ID
    #[sea_orm(default_value = "")]
    pub metrics_service_id: String,
    /// 推理指标最近注册时间（CST ISO-8601）
    pub metrics_registered_at: Option<String>,
    /// 推理指标最近一次注册失败原因
    #[sea_orm(default_value = "")]
    pub metrics_last_error: String,
    /// 自动重启策略（SSOT）：`never`（默认）/ `unless-stop`（docker 语义）
    ///
    /// 空串表示历史数据：按 `auto_restart_enabled` 兼容映射（见
    /// `deploy::autorestart::effective_policy`）。见
    /// `specs/docs/design/modelinfer/auto-restart.md`。
    #[sea_orm(default_value = "")]
    pub auto_restart_policy: String,
    /// 连续探活失败次数阈值（`1..=100`，默认 10）
    ///
    /// 仅在 `auto_restart_policy=unless-stop` 时生效。
    #[sea_orm(default_value = 10)]
    pub auto_restart_fail_count: i32,
    /// 历史兼容列：只在新策略写入时同步为 `auto_restart_policy != never`。
    #[sea_orm(default_value = false)]
    pub auto_restart_enabled: bool,
    /// 历史列：连续失败阈值（秒）；新逻辑不再使用，仅保留避免破坏旧库结构。
    #[sea_orm(default_value = 300)]
    pub auto_restart_fail_secs: i32,
    /// 当前连续探活失败次数（运行态；`health_status=healthy` 时清零）
    #[sea_orm(default_value = 0)]
    pub auto_restart_consecutive_failures: i32,
    /// 用户手动启停 / 配置变更后的抑制起点：早于该时间的启动失败不再触发自动重启
    #[sea_orm(default_value = "")]
    pub auto_restart_ignore_before: String,
    /// 自动重启相位：`""`（idle）/ `stopping`（已下发停止，待启动）/ `starting`（已下发启动）
    #[sea_orm(default_value = "")]
    pub auto_restart_phase: String,
    /// 本轮连续失败起点（CST ISO-8601；空 = 无失败）
    #[sea_orm(default_value = "")]
    pub auto_restart_failed_since: String,
    /// backoff 截止时间（此时间前不下发启动；空 = 无限制）
    #[sea_orm(default_value = "")]
    pub auto_restart_next_at: String,
    /// 当前循环内已下发的启动次数
    #[sea_orm(default_value = 0)]
    pub auto_restart_attempts: i32,
    /// 最近一次自动重启动作（停止 / 启动）下发时间（CST ISO-8601）
    #[sea_orm(default_value = "")]
    pub auto_restart_last_at: String,
    /// 最近一次自动重启相关失败原因
    #[sea_orm(default_value = "")]
    pub auto_restart_last_error: String,
    /// 运行状态：stopped / starting / running / stopping / failed / degraded
    #[sea_orm(default_value = "stopped")]
    pub state: String,
    /// 期望状态：stopped / running / stopping（编排轮询用）
    #[sea_orm(default_value = "stopped")]
    pub desired_state: String,
    /// 健康状态：unknown / healthy / unhealthy
    #[sea_orm(default_value = "unknown")]
    pub health_status: String,
    /// 最近一次错误信息
    #[sea_orm(default_value = "")]
    pub last_error: String,
    /// 最近启动时间（CST ISO-8601）
    pub last_started_at: Option<String>,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}