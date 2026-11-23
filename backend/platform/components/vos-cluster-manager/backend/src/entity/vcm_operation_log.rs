//! `vcm_operation_log` 实体：管理操作审计日志（管理面本地状态）

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "vcm_operation_log")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// 操作类型（cluster_init / node_add / node_remove ...）
    pub operation: String,
    /// 操作目标（cluster_id / node_ip / member_id）
    pub target: String,
    /// 操作详情（JSON；**禁止**放入密码等敏感字段）
    #[sea_orm(column_type = "Json", nullable)]
    pub detail: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}