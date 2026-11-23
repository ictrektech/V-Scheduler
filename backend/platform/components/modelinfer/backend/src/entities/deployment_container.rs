//! 运行配置容器清单实体：`modelinfer_deployment_containers`
//!
//! 一行 = 某运行配置在某节点上运行的容器（由 `start` 阶段脚本经 `modelinfer:` 结构化输出汇报）。
//! 以（运行配置、节点、容器名）为逻辑唯一键，应用层 upsert（不建数据库部分唯一索引）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_deployment_containers")]
pub struct Model {
    /// 记录 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 运行配置 ID
    #[sea_orm(indexed)]
    pub deployment_id: String,
    /// 节点 ID
    #[sea_orm(default_value = "")]
    pub node_id: String,
    /// 节点名
    #[sea_orm(default_value = "")]
    pub node_name: String,
    /// 节点地址
    #[sea_orm(default_value = "")]
    pub node_host: String,
    /// 容器名
    pub name: String,
    /// 容器镜像（可选）
    #[sea_orm(default_value = "")]
    pub image: String,
    /// 来源运行 ID
    #[sea_orm(default_value = "")]
    pub run_id: String,
    /// 来源阶段
    #[sea_orm(default_value = "")]
    pub stage: String,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}