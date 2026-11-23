//! `vcm_gateway_backend` 实体：SMB/NFS 网关 VIP 后端集合（管理面状态）

use chrono::{DateTime, Utc};
use sea_orm::entity::prelude::*;

/// 网关服务类型
pub mod service {
    /// SMB（445）
    pub const SMB: &str = "smb";
    /// NFS（2049）
    pub const NFS: &str = "nfs";
    /// RGW S3（7480；HAProxy 前端，后端为各节点 RGW）
    pub const RGW: &str = "rgw";
}

/// 后端状态
pub mod state {
    /// 参与 VIP 轮转
    pub const ACTIVE: &str = "active";
    /// 已排空（不参与轮转；HAProxy 健康检查返回 503）
    pub const DRAINED: &str = "drained";
}

#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "vcm_gateway_backend")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i64,
    /// smb | nfs
    pub service: String,
    /// 网关节点 IP
    pub node_ip: String,
    /// active | drained
    pub state: String,
    /// 排空原因（审计 / 状态展示；不含敏感信息）
    #[sea_orm(nullable)]
    pub reason: Option<String>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}