//! 全局 VRRP 分配（etcd 权威 + 本地 DB 缓存）。
//!
//! 原实现仅使用各节点本地 postgres，不同 allocator/服务可能重复分配同一 VRRP ID。
//! 这里把占用视图提升到 etcd：`/vos/cluster/vrrp/vips/<vip>` 与
//! `/vos/cluster/vrrp/ids/<id>` 由 VCM etcd 客户端 CAS 原子维护；本地表仅作缓存/审计。

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};

use crate::entity::vcm_vrrp_allocation;
use crate::error::{VcmError, codes};
use crate::etcd::{EtcdClusterClient, VrrpAllocation};

/// 分配者标识（文件网关/RGW 均通过 VCM 分配；释放校验使用同一 owner）。
pub const VRRP_OWNER: &str = "vcm";

fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

/// 将本地 DB 中的历史分配迁移到 etcd；冲突时显式失败，禁止静默覆盖。
pub async fn migrate_local(
    etcd: &dyn EtcdClusterClient,
    db: Option<&DatabaseConnection>,
) -> Result<(), VcmError> {
    let Some(db) = db else { return Ok(()) };
    let local = vcm_vrrp_allocation::Entity::find()
        .all(db)
        .await
        .map_err(|e| VcmError::new(codes::DB_FAILED, format!("读取本地 VRRP 分配失败: {e}")))?;
    if local.is_empty() {
        return Ok(());
    }
    let current = etcd.list_vrrp_allocations().await?;
    for row in local {
        let Some(id) = u8::try_from(row.vrrp_id).ok().filter(|id| (1..=255).contains(id)) else {
            return Err(VcmError::new(
                codes::GATEWAY_FAILED,
                format!("本地 VRRP ID 非法: vip={} id={}", row.vip, row.vrrp_id),
            ));
        };
        if let Some(existing) = current.iter().find(|a| a.vip == row.vip) {
            if existing.vrrp_id != id {
                return Err(VcmError::new(
                    codes::GATEWAY_FAILED,
                    format!(
                        "VRRP 迁移冲突: vip={} 本地 id={} etcd id={}",
                        row.vip, id, existing.vrrp_id
                    ),
                ));
            }
            continue;
        }
        if let Some(existing) = current.iter().find(|a| a.vrrp_id == id) {
            return Err(VcmError::new(
                codes::GATEWAY_FAILED,
                format!(
                    "VRRP 迁移冲突: id={} 本地 vip={} etcd vip={}",
                    id, row.vip, existing.vip
                ),
            ));
        }
        let alloc = VrrpAllocation {
            vip: row.vip.clone(),
            vrrp_id: id,
            owner: VRRP_OWNER.into(),
            updated_at: now(),
        };
        if !etcd.claim_vrrp_allocation(&alloc).await? {
            // 并发迁移：重新读取后确认是否已有同一分配
            let latest = etcd.list_vrrp_allocations().await?;
            let same_vip = latest.iter().find(|a| a.vip == row.vip);
            let same_id = latest.iter().find(|a| a.vrrp_id == id);
            if same_vip.is_some_and(|a| a.vrrp_id != id)
                || same_id.is_some_and(|a| a.vip != row.vip)
            {
                return Err(VcmError::new(
                    codes::GATEWAY_FAILED,
                    format!("VRRP 迁移并发冲突: vip={} id={id}", row.vip),
                ));
            }
        }
    }
    Ok(())
}

/// 分配（或复用）一个 VIP 的 VRRP ID。
pub async fn allocate(
    etcd: &dyn EtcdClusterClient,
    db: Option<&DatabaseConnection>,
    vip: &str,
    preferred: Option<u8>,
) -> Result<VrrpAllocation, VcmError> {
    migrate_local(etcd, db).await?;
    let current = etcd.list_vrrp_allocations().await?;
    if let Some(existing) = current.iter().find(|a| a.vip == vip) {
        return Ok(existing.clone());
    }
    let used: std::collections::HashSet<u8> = current.iter().map(|a| a.vrrp_id).collect();
    let candidate = preferred
        .filter(|id| (1..=255).contains(id) && !used.contains(id))
        .or_else(|| (!used.contains(&51)).then_some(51))
        .or_else(|| (1..=255).find(|id| !used.contains(id)))
        .ok_or_else(|| VcmError::new(codes::GATEWAY_FAILED, "VRRP ID 已耗尽（1-255 全部占用）"))?;

    // 首选失败时在剩余 ID 中重试；最多 255 次，避免并发下无限循环。
    let mut tried = 0usize;
    let mut next = Some(candidate);
    while let Some(id) = next {
        tried += 1;
        let alloc = VrrpAllocation {
            vip: vip.to_string(),
            vrrp_id: id,
            owner: VRRP_OWNER.into(),
            updated_at: now(),
        };
        if etcd.claim_vrrp_allocation(&alloc).await? {
            cache_upsert(db, &alloc).await?;
            return Ok(alloc);
        }
        if tried >= 255 {
            break;
        }
        let latest = etcd.list_vrrp_allocations().await?;
        if let Some(existing) = latest.iter().find(|a| a.vip == vip) {
            return Ok(existing.clone());
        }
        let used: std::collections::HashSet<u8> = latest.iter().map(|a| a.vrrp_id).collect();
        next = (1..=255).find(|id| !used.contains(id));
    }
    Err(VcmError::new(
        codes::GATEWAY_FAILED,
        format!("无法为 VIP {vip} 分配唯一 VRRP ID（并发冲突）"),
    ))
}

/// 释放（etcd 权威 + 本地缓存）。owner/id 不匹配时拒绝。
pub async fn release(
    etcd: &dyn EtcdClusterClient,
    db: Option<&DatabaseConnection>,
    vip: &str,
    vrrp_id: u8,
    owner: &str,
) -> Result<bool, VcmError> {
    let released = etcd.release_vrrp_allocation(vip, vrrp_id, owner).await?;
    if released
        && let Some(db) = db
    {
        vcm_vrrp_allocation::Entity::delete_many()
            .filter(vcm_vrrp_allocation::Column::Vip.eq(vip))
            .exec(db)
            .await
            .map_err(|e| {
                VcmError::new(codes::DB_FAILED, format!("删除本地 VRRP 缓存失败: {e}"))
            })?;
    }
    Ok(released)
}

async fn cache_upsert(db: Option<&DatabaseConnection>, alloc: &VrrpAllocation) -> Result<(), VcmError> {
    let Some(db) = db else { return Ok(()) };
    let existing = vcm_vrrp_allocation::Entity::find()
        .filter(vcm_vrrp_allocation::Column::Vip.eq(&alloc.vip))
        .one(db)
        .await
        .map_err(|e| VcmError::new(codes::DB_FAILED, format!("查询 VRRP 缓存失败: {e}")))?;
    if let Some(row) = existing {
        let mut active: vcm_vrrp_allocation::ActiveModel = row.into();
        active.vrrp_id = Set(i32::from(alloc.vrrp_id));
        active.updated_at = Set(chrono::Utc::now());
        active
            .update(db)
            .await
            .map_err(|e| VcmError::new(codes::DB_FAILED, format!("更新 VRRP 缓存失败: {e}")))?;
    } else {
        let active = vcm_vrrp_allocation::ActiveModel {
            vip: Set(alloc.vip.clone()),
            vrrp_id: Set(i32::from(alloc.vrrp_id)),
            created_at: Set(chrono::Utc::now()),
            updated_at: Set(chrono::Utc::now()),
            ..Default::default()
        };
        active
            .insert(db)
            .await
            .map_err(|e| VcmError::new(codes::DB_FAILED, format!("写入 VRRP 缓存失败: {e}")))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::etcd::{MockEtcdClusterClient, VrrpAllocation};

    #[tokio::test]
    async fn allocate_is_global_and_idempotent() {
        let etcd = MockEtcdClusterClient::new();
        let a = allocate(&etcd, None, "192.168.100.240", Some(51)).await.unwrap();
        assert_eq!(a.vrrp_id, 51);
        let b = allocate(&etcd, None, "192.168.100.240", None).await.unwrap();
        assert_eq!(b.vrrp_id, 51);
        let c = allocate(&etcd, None, "192.168.100.241", Some(51)).await.unwrap();
        assert_ne!(c.vrrp_id, 51);
    }

    #[tokio::test]
    async fn concurrent_claim_only_one_wins() {
        let etcd = MockEtcdClusterClient::new();
        let a = VrrpAllocation { vip: "1.1.1.1".into(), vrrp_id: 51, owner: "vcm".into(), updated_at: now() };
        let b = VrrpAllocation { vip: "1.1.1.2".into(), vrrp_id: 51, owner: "vcm".into(), updated_at: now() };
        assert!(etcd.claim_vrrp_allocation(&a).await.unwrap());
        assert!(!etcd.claim_vrrp_allocation(&b).await.unwrap());
    }

    #[tokio::test]
    async fn etcd_unavailable_fails_closed() {
        let etcd = crate::etcd::UnavailableEtcdClusterClient::new("test");
        let err = allocate(&etcd, None, "192.168.100.242", None)
            .await
            .unwrap_err();
        assert_eq!(err.code, codes::ETCD_FAILED);
    }

    #[tokio::test]
    async fn release_requires_matching_owner_and_id() {
        let etcd = MockEtcdClusterClient::new();
        let a = VrrpAllocation { vip: "1.1.1.1".into(), vrrp_id: 51, owner: "vcm".into(), updated_at: now() };
        etcd.claim_vrrp_allocation(&a).await.unwrap();
        assert!(release(&etcd, None, "1.1.1.1", 51, "other").await.is_err());
        assert!(release(&etcd, None, "1.1.1.1", 51, "vcm").await.unwrap());
    }
}
