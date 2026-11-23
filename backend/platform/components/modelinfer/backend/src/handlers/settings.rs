//! 运行时设置 HTTP handlers（保留策略）

use std::sync::Arc;

use axum::{Json, extract::State};
use serde::{Deserialize, Serialize};

use crate::error::{ModelinferError, ResultBody};
use crate::observability::{Settings, store};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    pub health_history_keep: u32,
    pub run_history_keep: u32,
}

/// 设置视图（含上限，便于前端约束）
#[derive(Debug, Serialize)]
pub struct SettingsVo {
    pub health_history_keep: u32,
    pub run_history_keep: u32,
    pub max_health_history_keep: u32,
    pub max_run_history_keep: u32,
}

fn vo(s: Settings) -> SettingsVo {
    SettingsVo {
        health_history_keep: s.health_history_keep,
        run_history_keep: s.run_history_keep,
        max_health_history_keep: store::MAX_HEALTH_KEEP,
        max_run_history_keep: store::MAX_RUN_KEEP,
    }
}

/// `GET /api/settings`
pub async fn get(
    State(state): State<Arc<AppState>>,
) -> Result<Json<ResultBody<SettingsVo>>, ModelinferError> {
    let s = store::get_settings(&state.db, state.settings_defaults).await?;
    Ok(Json(ResultBody::ok(vo(s), state.version)))
}

/// `PUT /api/settings`
pub async fn update(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SettingsUpdate>,
) -> Result<Json<ResultBody<SettingsVo>>, ModelinferError> {
    let saved = store::put_settings(
        &state.db,
        Settings {
            health_history_keep: req.health_history_keep,
            run_history_keep: req.run_history_keep,
        },
    )
    .await?;
    Ok(Json(ResultBody::ok(vo(saved), state.version)))
}