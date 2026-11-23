//! `vcm_cluster_registration` 实体：集群注册记录（管理面本地状态，可重建；etcd 为真相）

use chrono::{DateTime, NaiveDateTime, Utc};
use sea_orm::entity::prelude::*;

/// 集群注册状态常量（与 etcd ClusterInfo.status 对齐）
pub mod status {
    pub const INITIALIZED: &str = "initialized";
    pub const CREATED: &str = "created";
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "vcm_cluster_registration")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// 集群 UUID（与 etcd /vos/cluster/info 的 cluster_id 一致）
    #[sea_orm(unique)]
    pub cluster_id: String,
    pub cluster_name: String,
    /// [`status::INITIALIZED`] / [`status::CREATED`]
    pub status: String,
    /// RFC3339 字符串（与 etcd 侧格式一致，便于跨存储比对）
    pub registered_at: NaiveDateTime,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}