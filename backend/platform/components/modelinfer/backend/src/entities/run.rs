//! 运行记录实体：`modelinfer_runs`
//!
//! 一行 = 一次部署执行（启动 / 停止 / 健康巡检）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_runs")]
pub struct Model {
    /// 运行 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 运行配置 ID
    #[sea_orm(indexed)]
    pub deployment_id: String,
    /// 类型：start / stop / health
    pub kind: String,
    /// 状态：running / succeeded / failed
    #[sea_orm(default_value = "running")]
    pub state: String,
    /// 触发来源：manual / auto / scheduler
    #[sea_orm(default_value = "manual")]
    pub trigger: String,
    /// Temporal 工作流 id（仅作按需查询句柄；健康巡检为空）
    #[sea_orm(default_value = "")]
    pub workflow_id: String,
    /// 开始时间（CST ISO-8601）
    pub started_at: String,
    /// 结束时间（CST ISO-8601）
    pub finished_at: Option<String>,
    /// 摘要
    #[sea_orm(default_value = "")]
    pub summary: String,
    /// 错误信息
    #[sea_orm(default_value = "")]
    pub error: String,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}