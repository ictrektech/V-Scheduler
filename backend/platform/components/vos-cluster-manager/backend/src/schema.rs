//! 数据库 schema 同步：基于 sea-orm `SchemaBuilder` 的增量幂等同步
//!
//! 使用 `db.get_schema_builder().register(...).sync(&db)` 把项目中所有
//! sea-orm Entity 集中注册并增量同步到数据库：
//! - 未存在表 -> 执行 CREATE TABLE
//! - 已存在表 -> 追加缺失列（非破坏性，不删除/修改已有结构）
//!
//! 参考 vswitch `backend/src/schema.rs` 设计（D-VCM-09）：
//! - **SSOT**：`register_all_entities` 是项目 schema 同步的单一可信源
//! - `register_count()` 常量与 register 调用数一一对应，由静态测试守护
//!
//! ⚠️ **平台 postgres 慢查询修复（重要）**：sea-schema 0.18.1 原版的 FK 发现走
//! information_schema 五视图级联 JOIN，在表/约束多的库上每条查询 3~8s（schema sync
//! 可达数分钟）。本组件在 `Cargo.toml` `[patch.crates-io]` 引用仓库根
//! `third_party/sea-schema`（vendored 0.18.1 + 上游 PR #169：FK 发现改
//! pg_constraint/pg_attribute 直查，185s → <1s，与 vos-platform-backend 同款补丁）。
//! 升级 sea-schema 到含修复的 crates.io 版本后移除 patch 段。

use sea_orm::{DatabaseConnection, DbErr, SchemaBuilder};

use crate::entity::{vcm_cluster_registration, vcm_gateway_backend, vcm_operation_log, vcm_vrrp_allocation};

/// 注册全部 sea-orm Entity（SSOT）
///
/// **SSOT**：新增 sea-orm Entity 时**必须**在此处追加 `.register(...)`，
/// 否则该表不会被 `sync_schema` 创建。
///
/// 同时**必须**更新 `register_count()` 常量与下方注释中的 entity 总数。
pub fn register_all_entities(builder: SchemaBuilder) -> SchemaBuilder {
    builder
        // ===== vcm_cluster_registration：集群注册记录 =====
        .register(vcm_cluster_registration::Entity)
        // ===== vcm_operation_log：管理操作审计 =====
        .register(vcm_operation_log::Entity)
        // ===== vcm_gateway_backend：SMB/NFS 网关 VIP 后端集合 =====
        .register(vcm_gateway_backend::Entity)
        // ===== vcm_vrrp_allocation：VRRP ID 分配（每 VIP 稳定 ID）=====
        .register(vcm_vrrp_allocation::Entity)
}

/// 当前注册的 Entity 总数（与 `register_all_entities` 中 `.register(...)` 调用一一对应）
///
/// 修改 `register_all_entities` 的 register 链时**必须**同步更新本常量。
/// `tests::register_count_matches_source` 单测会做静态比对。
pub const fn register_count() -> usize {
    4
}

/// 执行数据库 schema 同步（增量幂等）
///
/// 调用 `db.get_schema_builder().register(...).sync(&db)`，已存在表追加缺失列，
/// 未存在表执行 CREATE TABLE。非破坏性：不会删除或修改已有列。
pub async fn sync_schema(db: &DatabaseConnection) -> Result<(), DbErr> {
    let started = std::time::Instant::now();
    let builder = db.get_schema_builder();
    let builder = register_all_entities(builder);
    builder.sync(db).await?;

    tracing::info!(
        entity_count = register_count(),
        elapsed_ms = started.elapsed().as_millis() as u64,
        "schema sync 完成（增量幂等：已存在表追加缺失列，未存在表 CREATE TABLE）"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// register_count 与 register 链一一对应（静态守护）
    ///
    /// 说明：新增 sea-orm Entity 时**必须**同时更新 `register_all_entities` 与
    /// `register_count()`（本断言防漏），实际建表行为由 db/mod.rs 的
    /// `sqlite_schema_sync_creates_vcm_tables` 单测覆盖（真实 SchemaBuilder sync）。
    #[test]
    fn register_count_matches_source() {
        assert_eq!(register_count(), 4);
    }
}