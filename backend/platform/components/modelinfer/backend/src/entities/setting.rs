//! 运行时可调设置实体：`modelinfer_settings`
//!
//! 键值对形式；当前用于运行可观测性的保留策略（`health_history_keep` / `run_history_keep`）。

use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_settings")]
pub struct Model {
    /// 设置键
    #[sea_orm(primary_key)]
    pub key: String,
    /// 设置值（字符串）
    #[sea_orm(column_type = "Text", default_value = "")]
    pub value: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}