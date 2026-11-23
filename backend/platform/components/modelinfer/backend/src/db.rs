//! 数据库连接与 schema 增量同步
//!
//! 生产**仅支持平台 PostgreSQL**（`VOS_MODELINFER_DATABASE_URL`，缺省由 `VOS_POSTGRES_*` 推导，
//! 见 [`crate::config::Config::resolved_database_url`]）；**不存在本地 SQLite 文件回退**。
//! 连接前 [`connect`] 会拒绝文件型 SQLite 连接串（防误配静默建库），仅放行单元测试用的
//! `sqlite::memory:`。
//! 连接后 `SchemaBuilder::sync` 对注册的全部 Entity 做增量幂等同步
//! （未存在表 → CREATE；缺列 → ALTER ADD COLUMN；非破坏性，不 DROP）。

use std::time::Duration;

use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseBackend, DatabaseConnection, SchemaBuilder};

use crate::entities::{
    ComfyuiWeightEntity, DeploymentContainerEntity, DeploymentEntity, NodeEntity, NodeMonitorEntity,
    PlazaModelEntity, PlazaSourceEntity, RunEntity, RunGlobalDataEntity, RunLogEntity,
    RunNodeDataEntity, RunStepEntity, SettingEntity,
};
use crate::error::{ModelinferError, codes};

/// 连接数据库并同步 schema
///
/// 连接串 MUST 为 postgres；文件型 SQLite（`sqlite://<path>` / `sqlite:<path>`）返回 20726 且
/// 不创建任何文件。`sqlite::memory:` 放行（仅单元测试）。
pub async fn connect(database_url: &str) -> Result<DatabaseConnection, ModelinferError> {
    ensure_supported_url(database_url)?;

    let mut options = ConnectOptions::new(database_url.to_string());
    options
        .max_connections(5)
        .min_connections(1)
        .connect_timeout(Duration::from_secs(8))
        .idle_timeout(Duration::from_secs(8))
        .sqlx_logging(false);

    let db = Database::connect(options)
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("连接数据库失败: {e}")))?;
    sync_schema(&db).await?;
    Ok(db)
}

/// 连接串守卫（纯函数）：拒绝文件型 SQLite，放行 postgres 与内存 SQLite
pub fn ensure_supported_url(database_url: &str) -> Result<(), ModelinferError> {
    let url = database_url.trim();
    if url == "sqlite::memory:" || url.starts_with("sqlite::memory:?") {
        return Ok(());
    }
    if url.starts_with("sqlite:") {
        return Err(ModelinferError::new(
            codes::DATABASE_CONFIG_INVALID,
            format!(
                "不支持本地 SQLite 数据库（当前: {}）：请使用平台 postgres 连接串",
                crate::config::mask_database_url(url)
            ),
        ));
    }
    if !crate::config::is_postgres_url(url) {
        return Err(ModelinferError::new(
            codes::DATABASE_CONFIG_INVALID,
            format!(
                "数据库连接串必须是 postgres（当前: {}）",
                crate::config::mask_database_url(url)
            ),
        ));
    }
    Ok(())
}

/// 注册全部 Entity（SSOT：新增 Entity 时必须同步此处与 [`register_count`]）
fn register_all_entities(builder: SchemaBuilder) -> SchemaBuilder {
    builder
        .register(NodeEntity)
        .register(NodeMonitorEntity)
        .register(DeploymentEntity)
        .register(PlazaSourceEntity)
        .register(PlazaModelEntity)
        .register(RunEntity)
        .register(RunLogEntity)
        .register(RunNodeDataEntity)
        .register(RunGlobalDataEntity)
        .register(RunStepEntity)
        .register(DeploymentContainerEntity)
        .register(SettingEntity)
        .register(ComfyuiWeightEntity)
}

const fn register_count() -> usize {
    13
}

async fn sync_schema(db: &DatabaseConnection) -> Result<(), ModelinferError> {
    let builder = db.get_schema_builder();
    let builder = register_all_entities(builder);
    builder
        .sync(db)
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("schema 增量同步失败: {e}")))?;
    drop_legacy_node_monitor_unique(db).await?;
    tracing::info!(entity_count = register_count(), "数据库 schema 同步完成");
    Ok(())
}

/// 一次性迁移：删除 `modelinfer_node_monitors.node_id` 的历史唯一约束。
///
/// 监控状态由「每节点一行」演进为「每 `(node_id, component)` 一行」后，旧的列级唯一约束
/// (`modelinfer_node_monitors_node_id_key`) 会阻止同节点多组件写入；sea-orm `SchemaBuilder::sync`
/// **只做增量新增、不删除**旧唯一约束，故此处显式幂等清理（postgres 专用；内存 sqlite 无此历史）。
async fn drop_legacy_node_monitor_unique(db: &DatabaseConnection) -> Result<(), ModelinferError> {
    if db.get_database_backend() != DatabaseBackend::Postgres {
        return Ok(());
    }
    db.execute_unprepared(
        "ALTER TABLE modelinfer_node_monitors DROP CONSTRAINT IF EXISTS modelinfer_node_monitors_node_id_key",
    )
    .await
    .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("清理历史唯一约束失败: {e}")))?;
    db.execute_unprepared("DROP INDEX IF EXISTS modelinfer_node_monitors_node_id_key")
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("清理历史唯一索引失败: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sea_orm::{ConnectionTrait, EntityTrait, QuerySelect};

    #[tokio::test]
    async fn sync_schema_creates_all_tables_and_is_idempotent() {
        let db = Database::connect("sqlite::memory:").await.expect("内存库连接");
        sync_schema(&db).await.expect("首次 sync");
        sync_schema(&db).await.expect("二次 sync 幂等");
        assert!(NodeEntity::find().limit(1).one(&db).await.is_ok());
        assert!(DeploymentEntity::find().limit(1).one(&db).await.is_ok());
        assert!(PlazaSourceEntity::find().limit(1).one(&db).await.is_ok());
        assert!(PlazaModelEntity::find().limit(1).one(&db).await.is_ok());
        assert!(RunEntity::find().limit(1).one(&db).await.is_ok());
        assert!(RunLogEntity::find().limit(1).one(&db).await.is_ok());
        assert!(RunNodeDataEntity::find().limit(1).one(&db).await.is_ok());
        assert!(RunGlobalDataEntity::find().limit(1).one(&db).await.is_ok());
        assert!(RunStepEntity::find().limit(1).one(&db).await.is_ok());
        assert!(DeploymentContainerEntity::find().limit(1).one(&db).await.is_ok());
        assert!(SettingEntity::find().limit(1).one(&db).await.is_ok());
        assert!(NodeMonitorEntity::find().limit(1).one(&db).await.is_ok());
        assert!(ComfyuiWeightEntity::find().limit(1).one(&db).await.is_ok());
    }

    #[tokio::test]
    async fn sync_schema_adds_new_column() {
        let db = Database::connect("sqlite::memory:").await.unwrap();
        // 构造一个缺可空列的旧表，验证 sync 自动补列
        db.execute_unprepared(
            "CREATE TABLE modelinfer_nodes (\
                id TEXT PRIMARY KEY, \
                name TEXT NOT NULL, \
                host TEXT NOT NULL UNIQUE, \
                ssh_port INTEGER NOT NULL DEFAULT 22, \
                ssh_user TEXT NOT NULL DEFAULT 'root', \
                ssh_password TEXT NOT NULL DEFAULT '', \
                status TEXT NOT NULL DEFAULT 'unknown', \
                created_at TEXT NOT NULL, \
                updated_at TEXT NOT NULL\
             )",
        )
        .await
        .unwrap();
        sync_schema(&db).await.unwrap();
        assert!(NodeEntity::find().limit(1).one(&db).await.is_ok());
    }

    #[test]
    fn register_count_matches() {
        assert_eq!(register_count(), 13);
    }

    #[test]
    fn rejects_file_sqlite_and_accepts_postgres() {
        // 文件型 sqlite 被拒（20726）
        let err = ensure_supported_url("sqlite:///tmp/modelinfer.db?mode=rwc").unwrap_err();
        assert_eq!(err.code, codes::DATABASE_CONFIG_INVALID);
        assert!(err.msg.contains("不支持本地 SQLite"));
        // 其他非 postgres scheme 被拒
        assert_eq!(
            ensure_supported_url("mysql://u:p@h/db").unwrap_err().code,
            codes::DATABASE_CONFIG_INVALID
        );
        // 内存库（单测用）与 postgres 放行
        assert!(ensure_supported_url("sqlite::memory:").is_ok());
        assert!(ensure_supported_url("postgres://u:p@h:5432/db").is_ok());
    }

    #[tokio::test]
    async fn connect_rejects_sqlite_file_without_creating_it() {
        let tmp = tempfile::TempDir::new().unwrap();
        let db_file = tmp.path().join("modelinfer.db");
        let url = format!("sqlite://{}?mode=rwc", db_file.display());
        let err = connect(&url).await.unwrap_err();
        assert_eq!(err.code, codes::DATABASE_CONFIG_INVALID);
        assert!(!db_file.exists(), "拒绝连接时不应创建数据库文件");
    }
}