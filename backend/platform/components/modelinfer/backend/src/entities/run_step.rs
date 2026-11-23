//! 运行步骤实体：`modelinfer_run_steps`
//!
//! 一行 = 一次运行中的一个**编排步骤**（activity 调用 / 调度器动作 / 生命周期阶段）。
//! 步骤是 modelinfer **自身**的编排台账，MUST NOT 存储 Temporal 的 activity 历史
//! （activity id / type / attempt / history 等；Temporal 细节按需经 API 查询，见设计 D-MI-RTL-04）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_run_steps")]
pub struct Model {
    /// 步骤 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 运行 ID
    #[sea_orm(indexed)]
    pub run_id: String,
    /// 运行配置 ID
    #[sea_orm(indexed)]
    pub deployment_id: String,
    /// 运行类型：start / stop / health
    pub kind: String,
    /// 步骤机器标识（prepare / check-node / ensure-images / mount-nfs / distribute /
    /// stage:<阶段> / register-vrouter / cleanup / umount / deregister-vrouter ...）
    #[sea_orm(default_value = "")]
    pub step: String,
    /// 关联的生命周期阶段（非阶段步骤为 ""，用于关联脚本日志与结构化数据）
    #[sea_orm(default_value = "")]
    pub stage: String,
    /// 中文可读名
    #[sea_orm(default_value = "")]
    pub title: String,
    /// 状态：running / succeeded / failed / skipped
    #[sea_orm(default_value = "running")]
    pub state: String,
    /// 编排来源：temporal / scheduler
    #[sea_orm(default_value = "")]
    pub orchestrator: String,
    /// 节点级步骤归属（否则为空）
    #[sea_orm(default_value = "")]
    pub node_id: String,
    #[sea_orm(default_value = "")]
    pub node_name: String,
    #[sea_orm(default_value = "")]
    pub node_host: String,
    /// run 内顺序号
    #[sea_orm(default_value = 0)]
    pub seq: i64,
    /// 结果 / 输入摘要（JSON）
    #[sea_orm(column_type = "Text", default_value = "")]
    pub detail_json: String,
    /// 失败原因
    #[sea_orm(column_type = "Text", default_value = "")]
    pub error: String,
    /// 开始时间（CST ISO-8601）
    pub started_at: String,
    /// 结束时间（CST ISO-8601，进行中为空）
    pub finished_at: Option<String>,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}