//! 管理面状态库（平台 postgres，D-VCM-09）
//!
//! 连接平台 postgres（平台注入 `VOS_HOST_GW_IP` + `VOS_POSTGRES_PORT/USER/PASSWORD/DB`，
//! 实测变量集见 `.agents/context/troubleshooting.md`——**非** spec §9.1 表格老写的 `VOS_DB_*`）；
//! 启动时 `schema::sync_schema` 增量幂等建表（`vcm_` 前缀）。
//!
//! 降级策略：平台未注入 DB 变量 → `init_db` 返回 `Ok(None)`，主容器照常启动，
//! 管理写操作返回 20503（DB_FAILED）；etcd 仍是集群状态真相。

use sea_orm::{Database, DatabaseConnection};

use crate::entity::{vcm_cluster_registration, vcm_gateway_backend, vcm_operation_log, vcm_vrrp_allocation};
use crate::error::VcmError;

pub mod vrrp;

/// 平台 postgres 连接配置（环境变量解析结果）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DbConfig {
    pub host: String,
    pub port: String,
    pub user: String,
    pub password: String,
    pub db: String,
}

impl DbConfig {
    /// 构造 sea-orm 连接 URL（`postgres://user:pass@host:port/db`）
    pub fn url(&self) -> String {
        format!(
            "postgres://{}:{}@{}:{}/{}",
            self.user, self.password, self.host, self.port, self.db
        )
    }
}

/// 从环境变量解析 DB 配置（纯函数，便于单测）
///
/// 需要的变量：`VOS_HOST_GW_IP` / `VOS_POSTGRES_PORT` / `VOS_POSTGRES_USER` /
/// `VOS_POSTGRES_PASSWORD` / `VOS_POSTGRES_DB`。任一缺失 / 空 → `None`（降级）。
pub fn db_config_from_env<K: AsRef<str>, V: AsRef<str>, I: IntoIterator<Item = (K, V)>>(
    env: I,
) -> Option<DbConfig> {
    let mut map = std::collections::HashMap::new();
    for (k, v) in env {
        map.insert(k.as_ref().to_string(), v.as_ref().to_string());
    }
    let get = |key: &str| -> Option<String> {
        let v = map.get(key)?.trim().to_string();
        if v.is_empty() {
            None
        } else {
            Some(v)
        }
    };
    Some(DbConfig {
        host: get("VOS_HOST_GW_IP")?,
        port: get("VOS_POSTGRES_PORT")?,
        user: get("VOS_POSTGRES_USER")?,
        password: get("VOS_POSTGRES_PASSWORD")?,
        db: get("VOS_POSTGRES_DB")?,
    })
}

/// 初始化数据库连接 + 幂等建表
///
/// - `None`（未注入 DB 变量）→ `Ok(None)`：管理面降级（写操作 20503）
/// - `Some(config)` → 连接 + `sync_schema`；**连接失败 → Err**（配置错误应暴露）；
///   **建表失败 → 降级 `Ok(None)`**（管理面可重建，etcd 仍是真相，仅记录告警）
pub async fn init_db(config: Option<&DbConfig>) -> Result<Option<DatabaseConnection>, VcmError> {
    let url = config.map(DbConfig::url);
    init_db_from_url(url.as_deref()).await
}

pub async fn init_db_from_url(url: Option<&str>) -> Result<Option<DatabaseConnection>, VcmError> {
    let Some(url) = url else {
        tracing::warn!("未检测到 VOS_POSTGRES_* 注入，管理面状态库降级（写操作将返回 20503）");
        return Ok(None);
    };

    let db = Database::connect(url)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("连接平台 postgres 失败: {e}")))?;

    if let Err(e) = crate::schema::sync_schema(&db).await {
        tracing::warn!(
            err = %e,
            "管理面表结构同步失败，降级为无状态库（etcd 仍是集群状态真相；影响：管理面记录缺失）"
        );
        return Ok(None);
    }

    tracing::info!("管理面状态库连接并同步完成");
    Ok(Some(db))
}

/// 写入集群注册记录（幂等：同 cluster_id 存在则更新）
pub async fn upsert_cluster_registration(
    db: &DatabaseConnection,
    cluster_id: &str,
    cluster_name: &str,
    status: &str,
    registered_at: chrono::NaiveDateTime,
) -> Result<(), VcmError> {
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

    let existing = vcm_cluster_registration::Entity::find()
        .filter(vcm_cluster_registration::Column::ClusterId.eq(cluster_id))
        .one(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("查询注册记录失败: {e}")))?;

    let now = chrono::Utc::now();
    if let Some(row) = existing {
        let mut active: vcm_cluster_registration::ActiveModel = row.into();
        active.status = Set(status.to_string());
        active.updated_at = Set(now);
        active
            .update(db)
            .await
            .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("更新注册记录失败: {e}")))?;
    } else {
        let new = vcm_cluster_registration::ActiveModel {
            cluster_id: Set(cluster_id.to_string()),
            cluster_name: Set(cluster_name.to_string()),
            status: Set(status.to_string()),
            registered_at: Set(registered_at),
            updated_at: Set(now),
            ..Default::default()
        };
        new.insert(db)
            .await
            .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("插入注册记录失败: {e}")))?;
    }
    Ok(())
}

/// 写入操作审计日志
pub async fn insert_operation_log(
    db: &DatabaseConnection,
    operation: &str,
    target: &str,
    detail: Option<serde_json::Value>,
) -> Result<(), VcmError> {
    use sea_orm::{ActiveModelTrait, Set};

    let new = vcm_operation_log::ActiveModel {
        operation: Set(operation.to_string()),
        target: Set(target.to_string()),
        detail: Set(detail),
        created_at: Set(chrono::Utc::now()),
        ..Default::default()
    };
    new.insert(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("写操作日志失败: {e}")))?;
    Ok(())
}

/// 读取全部网关后端行（按 service / node_ip 排序，输出稳定）
pub async fn list_gateway_backends(
    db: &DatabaseConnection,
) -> Result<Vec<vcm_gateway_backend::Model>, VcmError> {
    use sea_orm::{EntityTrait, QueryOrder};

    vcm_gateway_backend::Entity::find()
        .order_by_asc(vcm_gateway_backend::Column::Service)
        .order_by_asc(vcm_gateway_backend::Column::NodeIp)
        .all(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("查询网关后端失败: {e}")))
}

/// 全量替换某服务的网关集合（保留既有节点的排空状态）
///
/// - 移除不在新集合中的节点
/// - 新增节点以 `active` 入表
/// - 既有节点保持原状态（排空中的网关不会因"重设集合"被静默恢复）
pub async fn replace_gateway_backends(
    db: &DatabaseConnection,
    service: &str,
    node_ips: &[String],
) -> Result<(), VcmError> {
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

    let existing = vcm_gateway_backend::Entity::find()
        .filter(vcm_gateway_backend::Column::Service.eq(service))
        .all(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("查询网关后端失败: {e}")))?;

    for row in &existing {
        if !node_ips.iter().any(|ip| ip == &row.node_ip) {
            vcm_gateway_backend::Entity::delete_by_id(row.id)
                .exec(db)
                .await
                .map_err(|e| {
                    VcmError::new(crate::error::codes::DB_FAILED, format!("删除网关后端失败: {e}"))
                })?;
        }
    }

    let now = chrono::Utc::now();
    for ip in node_ips {
        if existing.iter().any(|r| &r.node_ip == ip) {
            continue;
        }
        let row = vcm_gateway_backend::ActiveModel {
            service: Set(service.to_string()),
            node_ip: Set(ip.clone()),
            state: Set(vcm_gateway_backend::state::ACTIVE.to_string()),
            reason: Set(None),
            updated_at: Set(now),
            ..Default::default()
        };
        row.insert(db).await.map_err(|e| {
            VcmError::new(crate::error::codes::DB_FAILED, format!("插入网关后端失败: {e}"))
        })?;
    }
    Ok(())
}

/// 设置单个网关的排空状态（不存在 → Ok(false)）
pub async fn set_gateway_drain(
    db: &DatabaseConnection,
    service: &str,
    node_ip: &str,
    drained: bool,
    reason: Option<&str>,
) -> Result<bool, VcmError> {
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

    let row = vcm_gateway_backend::Entity::find()
        .filter(vcm_gateway_backend::Column::Service.eq(service))
        .filter(vcm_gateway_backend::Column::NodeIp.eq(node_ip))
        .one(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("查询网关后端失败: {e}")))?;
    let Some(row) = row else { return Ok(false) };
    let mut active: vcm_gateway_backend::ActiveModel = row.into();
    active.state = Set(if drained {
        vcm_gateway_backend::state::DRAINED.to_string()
    } else {
        vcm_gateway_backend::state::ACTIVE.to_string()
    });
    active.reason = Set(reason.map(|s| s.to_string()));
    active.updated_at = Set(chrono::Utc::now());
    active
        .update(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("更新网关后端失败: {e}")))?;
    Ok(true)
}

/// 参与轮转的网关 IP（仅 `active`，按 IP 排序）
pub async fn active_gateway_ips(
    db: &DatabaseConnection,
    service: &str,
) -> Result<Vec<String>, VcmError> {
    Ok(list_gateway_backends(db)
        .await?
        .into_iter()
        .filter(|r| r.service == service && r.state == vcm_gateway_backend::state::ACTIVE)
        .map(|r| r.node_ip)
        .collect())
}

/// 读取 VIP 的 VRRP ID 分配（不存在返回 None）
pub async fn get_vrrp_allocation(
    db: &DatabaseConnection,
    vip: &str,
) -> Result<Option<vcm_vrrp_allocation::Model>, VcmError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    vcm_vrrp_allocation::Entity::find()
        .filter(vcm_vrrp_allocation::Column::Vip.eq(vip))
        .one(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("查询 VRRP 分配失败: {e}")))
}

/// 为 VIP 分配稳定的 VRRP ID（1-255）。
///
/// - 已存在该 VIP 的记录 → 直接返回（幂等）；
/// - `preferred` 合法且未被占用 → 优先使用（用于 allocator 变化时保持稳定）；
/// - 否则选最小空闲 ID。
pub async fn allocate_vrrp_id(
    db: &DatabaseConnection,
    vip: &str,
    preferred: Option<u8>,
) -> Result<vcm_vrrp_allocation::Model, VcmError> {
    use std::collections::HashSet;

    use sea_orm::{ActiveModelTrait, EntityTrait, Set};

    if let Some(row) = get_vrrp_allocation(db, vip).await? {
        return Ok(row);
    }
    let rows = vcm_vrrp_allocation::Entity::find()
        .all(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("查询 VRRP 分配失败: {e}")))?;
    let used: HashSet<i32> = rows.iter().map(|r| r.vrrp_id).collect();
    let pick = preferred
        .filter(|id| (1..=255).contains(id) && !used.contains(&(*id as i32)))
        .map(i32::from)
        .or_else(|| (!used.contains(&51)).then_some(51))
        .or_else(|| (1..=255).find(|id| !used.contains(id)));
    let Some(vrrp_id) = pick else {
        return Err(VcmError::new(
            crate::error::codes::GATEWAY_FAILED,
            "VRRP ID 已耗尽（1-255 全部占用）",
        ));
    };

    let now = chrono::Utc::now();
    let model = vcm_vrrp_allocation::ActiveModel {
        vip: Set(vip.to_string()),
        vrrp_id: Set(vrrp_id),
        created_at: Set(now),
        updated_at: Set(now),
        ..Default::default()
    };
    match model.insert(db).await {
        Ok(row) => Ok(row),
        Err(e) => {
            // 并发分配：唯一约束冲突时重新读取已有记录
            if let Some(row) = get_vrrp_allocation(db, vip).await? {
                Ok(row)
            } else {
                Err(VcmError::new(
                    crate::error::codes::DB_FAILED,
                    format!("写入 VRRP 分配失败: {e}"),
                ))
            }
        }
    }
}

/// 释放 VIP 的 VRRP ID（VIP 下线/迁移时使用）
pub async fn release_vrrp_id(db: &DatabaseConnection, vip: &str) -> Result<bool, VcmError> {
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

    let res = vcm_vrrp_allocation::Entity::delete_many()
        .filter(vcm_vrrp_allocation::Column::Vip.eq(vip))
        .exec(db)
        .await
        .map_err(|e| VcmError::new(crate::error::codes::DB_FAILED, format!("释放 VRRP 分配失败: {e}")))?;
    Ok(res.rows_affected > 0)
}

/// 测试辅助：创建临时 SQLite 数据库并建表（多方言支持：生产 PG，测试 SQLite）
#[cfg(test)]
pub async fn init_test_sqlite(path: &std::path::Path) -> DatabaseConnection {
    let url = format!("sqlite://{}?mode=rwc", path.display());
    let db = Database::connect(&url).await.unwrap();
    crate::schema::sync_schema(&db).await.unwrap();
    db
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn db_config_from_env_missing_returns_none() {
        let empty: [(&str, &str); 0] = [];
        assert!(db_config_from_env(empty).is_none());
        // 缺单个变量 → None（降级）
        let env = [
            ("VOS_HOST_GW_IP", "172.17.0.1"),
            ("VOS_POSTGRES_PORT", "5432"),
            ("VOS_POSTGRES_USER", "postgres"),
            ("VOS_POSTGRES_PASSWORD", "pw"),
            // 缺 VOS_POSTGRES_DB
        ];
        assert!(db_config_from_env(env).is_none());
        // 空值也视为缺失
        let env = [
            ("VOS_HOST_GW_IP", "172.17.0.1"),
            ("VOS_POSTGRES_PORT", ""),
            ("VOS_POSTGRES_USER", "postgres"),
            ("VOS_POSTGRES_PASSWORD", "pw"),
            ("VOS_POSTGRES_DB", "postgres"),
        ];
        assert!(db_config_from_env(env).is_none());
    }

    #[test]
    fn db_config_from_env_full_and_url() {
        let env = [
            ("VOS_HOST_GW_IP", "172.17.0.1"),
            ("VOS_POSTGRES_PORT", "5432"),
            ("VOS_POSTGRES_USER", "postgres"),
            ("VOS_POSTGRES_PASSWORD", "pg-secret"),
            ("VOS_POSTGRES_DB", "vos"),
        ];
        let cfg = db_config_from_env(env).unwrap();
        assert_eq!(
            cfg.url(),
            "postgres://postgres:pg-secret@172.17.0.1:5432/vos"
        );
    }

    #[tokio::test]
    async fn vrrp_allocation_is_idempotent_and_avoids_conflicts() {
        let tmp = TempDir::new().unwrap();
        let db = init_test_sqlite(&tmp.path().join("vrrp.db")).await;

        let first = allocate_vrrp_id(&db, "192.168.100.240", Some(51))
            .await
            .unwrap();
        assert_eq!(first.vrrp_id, 51);

        // 同一 VIP 重复分配幂等
        let again = allocate_vrrp_id(&db, "192.168.100.240", None)
            .await
            .unwrap();
        assert_eq!(again.vrrp_id, 51);

        // preferred 已被占用 → 分配其他 ID
        let other = allocate_vrrp_id(&db, "192.168.100.241", Some(51))
            .await
            .unwrap();
        assert_ne!(other.vrrp_id, 51);
        assert!((1..=255).contains(&other.vrrp_id));

        assert!(release_vrrp_id(&db, "192.168.100.241").await.unwrap());
        assert!(!release_vrrp_id(&db, "192.168.100.241").await.unwrap());
    }

    #[tokio::test]
    async fn sqlite_schema_sync_creates_vcm_tables() {
        use sea_orm::{ActiveModelTrait, Set};

        let tmp = TempDir::new().unwrap();
        let db = init_test_sqlite(&tmp.path().join("test.db")).await;

        let insert = vcm_cluster_registration::ActiveModel {
            cluster_id: Set("c-1".to_string()),
            cluster_name: Set("测试集群".to_string()),
            status: Set("initialized".to_string()),
            registered_at: Set(chrono::Utc::now().naive_utc()),
            updated_at: Set(chrono::Utc::now()),
            ..Default::default()
        };
        let row = insert.insert(&db).await.unwrap();
        assert_eq!(row.cluster_id, "c-1");

        // 幂等：再次 sync_schema 不报错
        crate::schema::sync_schema(&db).await.unwrap();

        // operation_log 也可插入
        let op = vcm_operation_log::ActiveModel {
            operation: Set("cluster_init".to_string()),
            target: Set("c-1".to_string()),
            detail: Set(Some(serde_json::json!({"ok": true}))),
            created_at: Set(chrono::Utc::now()),
            ..Default::default()
        };
        op.insert(&db).await.unwrap();
    }

    #[tokio::test]
    async fn upsert_registration_is_idempotent() {
        use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

        let tmp = TempDir::new().unwrap();
        let db = init_test_sqlite(&tmp.path().join("t2.db")).await;

        let ts = chrono::Utc::now().naive_utc();
        upsert_cluster_registration(&db, "c-x", "集群X", "initialized", ts)
            .await
            .unwrap();
        upsert_cluster_registration(&db, "c-x", "集群X", "created", ts)
            .await
            .unwrap();

        let rows = vcm_cluster_registration::Entity::find()
            .filter(vcm_cluster_registration::Column::ClusterId.eq("c-x"))
            .all(&db)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "重复 upsert 应保持单行");
        assert_eq!(rows[0].status, "created");
    }
}
