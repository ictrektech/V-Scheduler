//! 运行配置推理指标字段持久化（`modelinfer_deployments` 的引擎 / 注册状态列）
//!
//! 均为按部署 id 的幂等更新；字段用途见 `design.md` D-MI-IM-10。

use sea_orm::{ActiveModelTrait, DatabaseConnection, Set};

use crate::entities::deployment;
use crate::error::ModelinferError;
use crate::inference::{EngineInfo, metrics_state};
use crate::time;

async fn load(db: &DatabaseConnection, id: &str) -> Result<deployment::Model, ModelinferError> {
    crate::deployments::get_deployment(db, id).await
}

/// 写回引擎类型 / 版本 / 来源（不改变注册状态）
pub async fn save_engine(
    db: &DatabaseConnection,
    id: &str,
    info: &EngineInfo,
) -> Result<(), ModelinferError> {
    let m = load(db, id).await?;
    let mut am: deployment::ActiveModel = m.into();
    am.inference_engine = Set(info.engine.as_str().to_string());
    am.inference_engine_version = Set(info.version.clone());
    am.inference_engine_source = Set(info.source.clone());
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(crate::nodes::db_err)?;
    Ok(())
}

/// 置「已注册」（记录服务 ID 与时间）
pub async fn mark_registered(
    db: &DatabaseConnection,
    id: &str,
    service_id: &str,
) -> Result<(), ModelinferError> {
    let m = load(db, id).await?;
    let mut am: deployment::ActiveModel = m.into();
    am.metrics_state = Set(metrics_state::REGISTERED.to_string());
    am.metrics_service_id = Set(service_id.to_string());
    am.metrics_registered_at = Set(Some(time::now_iso()));
    am.metrics_last_error = Set(String::new());
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(crate::nodes::db_err)?;
    Ok(())
}

/// 置「注册失败」（记录原因）
pub async fn mark_error(
    db: &DatabaseConnection,
    id: &str,
    error: &str,
) -> Result<(), ModelinferError> {
    let m = load(db, id).await?;
    let mut am: deployment::ActiveModel = m.into();
    am.metrics_state = Set(metrics_state::ERROR.to_string());
    am.metrics_last_error = Set(error.to_string());
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(crate::nodes::db_err)?;
    Ok(())
}

/// 置「引擎不受支持」（不采集指标）
pub async fn mark_unsupported(db: &DatabaseConnection, id: &str) -> Result<(), ModelinferError> {
    let m = load(db, id).await?;
    let mut am: deployment::ActiveModel = m.into();
    am.metrics_state = Set(metrics_state::UNSUPPORTED.to_string());
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(crate::nodes::db_err)?;
    Ok(())
}

/// 置「未注册」并清空服务 ID / 时间（停止 / 注销）
pub async fn clear(db: &DatabaseConnection, id: &str) -> Result<(), ModelinferError> {
    let m = load(db, id).await?;
    let mut am: deployment::ActiveModel = m.into();
    am.metrics_state = Set(metrics_state::UNREGISTERED.to_string());
    am.metrics_service_id = Set(String::new());
    am.metrics_registered_at = Set(None);
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(crate::nodes::db_err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::inference::InferenceEngine;

    async fn seed(db: &DatabaseConnection) -> String {
        let now = time::now_iso();
        let am = deployment::ActiveModel {
            id: Set("d1".to_string()),
            name: Set("n1".to_string()),
            plaza_model_id: Set("c@1".to_string()),
            created_at: Set(now.clone()),
            updated_at: Set(now),
            ..Default::default()
        };
        am.insert(db).await.unwrap();
        "d1".to_string()
    }

    #[tokio::test]
    async fn engine_and_metrics_state_roundtrip() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let id = seed(&db).await;

        let info = EngineInfo::new(InferenceEngine::Vllm, "0.9.3", "probe");
        save_engine(&db, &id, &info).await.unwrap();
        let m = load(&db, &id).await.unwrap();
        assert_eq!(m.inference_engine, "vllm");
        assert_eq!(m.inference_engine_version, "0.9.3");
        assert_eq!(m.inference_engine_source, "probe");
        assert_eq!(m.metrics_state, "unregistered");

        mark_registered(&db, &id, "modelinfer-vllm-d1").await.unwrap();
        let m = load(&db, &id).await.unwrap();
        assert_eq!(m.metrics_state, "registered");
        assert_eq!(m.metrics_service_id, "modelinfer-vllm-d1");
        assert!(m.metrics_registered_at.is_some());

        mark_error(&db, &id, "consul down").await.unwrap();
        let m = load(&db, &id).await.unwrap();
        assert_eq!(m.metrics_state, "error");
        assert_eq!(m.metrics_last_error, "consul down");

        mark_unsupported(&db, &id).await.unwrap();
        assert_eq!(load(&db, &id).await.unwrap().metrics_state, "unsupported");

        clear(&db, &id).await.unwrap();
        let m = load(&db, &id).await.unwrap();
        assert_eq!(m.metrics_state, "unregistered");
        assert_eq!(m.metrics_service_id, "");
        assert!(m.metrics_registered_at.is_none());
    }
}