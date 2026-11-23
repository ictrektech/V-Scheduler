//! `vcm_vrrp_allocation` 实体：VRRP ID 分配表（每个 VIP 一个稳定 ID）
//!
//! VRRP ID 由 VCM 管理面数据库分配，不再由用户手填；同一 VIP 重复 apply 时复用同一 ID。
//! 表位于各节点本地平台 postgres，`apply` 由编排方选择首个网关节点作为 allocator，
//! 并把分配的 ID 缓存到 etcd 配置中，避免 allocator 变化时漂移。

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "vcm_vrrp_allocation")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// 独立 VIP 地址（唯一）
    #[sea_orm(unique)]
    pub vip: String,
    /// VRRP virtual_router_id（1-255，唯一）
    #[sea_orm(unique)]
    pub vrrp_id: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}