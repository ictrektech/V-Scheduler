//! 节点监控组件状态实体：`modelinfer_node_monitors`
//!
//! 一行 = 某节点**某个监控组件**的部署状态与**最近一次**部署的步骤记录（N:1，`(node_id, component)`
//! 逻辑唯一，唯一性由应用层保证）。监控组件部署为节点级 Temporal 工作流，本表为其状态台账
//! （步骤列表满足「执行情况 + 重试」）。

use sea_orm::entity::prelude::*;
use sea_orm::sea_query::StringLen;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "modelinfer_node_monitors")]
pub struct Model {
    /// 记录 ID（ULID）
    #[sea_orm(primary_key, column_type = "String(StringLen::N(32))")]
    pub id: String,
    /// 节点 ID（与 `component` 组成逻辑唯一键）
    #[sea_orm(indexed)]
    pub node_id: String,
    /// 组件标识（`node-exporter` / `nvidia-gpu-exporter`；与 `node_id` 组成逻辑唯一键）
    #[sea_orm(default_value = "node-exporter", indexed)]
    pub component: String,
    /// 状态：pending / deploying / deployed / failed
    #[sea_orm(default_value = "pending")]
    pub state: String,
    /// consul 服务 ID
    #[sea_orm(default_value = "")]
    pub service_id: String,
    /// consul 服务名
    #[sea_orm(default_value = "")]
    pub service_name: String,
    /// 使用的镜像
    #[sea_orm(default_value = "")]
    pub image: String,
    /// 指标分类（`system` / `gpu`）
    #[sea_orm(default_value = "")]
    pub category: String,
    /// 最近一次部署的工作流 ID
    #[sea_orm(default_value = "")]
    pub workflow_id: String,
    /// 最近错误
    #[sea_orm(column_type = "Text", default_value = "")]
    pub last_error: String,
    /// 最近一次部署的步骤（JSON 数组）
    #[sea_orm(column_type = "Text", default_value = "[]")]
    pub steps_json: String,
    /// 最近成功部署时间（CST ISO-8601）
    pub deployed_at: Option<String>,
    /// 创建时间（CST ISO-8601）
    pub created_at: String,
    /// 更新时间（CST ISO-8601）
    pub updated_at: String,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}