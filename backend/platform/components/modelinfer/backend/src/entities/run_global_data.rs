//! 阶段全局结构化数据实体：`modelinfer_run_global_data`
//!
//! 一行 = 某次运行中，某阶段各节点输出的 `global` 对象合并后的全局 JSON 对象。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_run_global_data")]
pub struct Model {
    /// 数据 ID（ULID）
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
    /// 阶段名
    #[sea_orm(default_value = "")]
    pub stage: String,
    /// 合并后的全局 JSON 对象
    #[sea_orm(column_type = "Text", default_value = "{}")]
    pub global_json: String,
    /// 参与合并的节点数
    #[sea_orm(default_value = 0)]
    pub node_count: i32,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}