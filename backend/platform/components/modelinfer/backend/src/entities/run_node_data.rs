//! 阶段节点结构化数据实体：`modelinfer_run_node_data`
//!
//! 一行 = 某次运行中，某阶段在某节点上输出的结构化数据聚合成的 JSON 对象。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_run_node_data")]
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
    /// 节点 ID
    #[sea_orm(default_value = "")]
    pub node_id: String,
    /// 节点名
    #[sea_orm(default_value = "")]
    pub node_name: String,
    /// 节点地址
    #[sea_orm(default_value = "")]
    pub node_host: String,
    /// 聚合后的节点 JSON 对象
    #[sea_orm(column_type = "Text", default_value = "{}")]
    pub data_json: String,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}