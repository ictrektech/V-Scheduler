//! ComfyUI 实例权重管理 HTTP handlers

use std::sync::Arc;

use axum::{
    Json,
    extract::{Path, State},
};
use serde::Serialize;

use crate::comfyui::{self, AddWeightRequest, WeightView};
use crate::deployments;
use crate::error::{ModelinferError, ResultBody};
use crate::state::AppState;

/// 实例权重台账视图
#[derive(Debug, Serialize)]
pub struct ComfyuiWeightsVo {
    pub instance_id: String,
    /// ComfyUI 预设类型目录清单
    pub types: Vec<String>,
    pub weights: Vec<WeightView>,
}

/// `GET /api/deployments/{id}/comfyui/weights`
pub async fn list_weights(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<ComfyuiWeightsVo>>, ModelinferError> {
    // 运行配置必须存在
    let _ = deployments::get_deployment(&state.db, &id).await?;
    let weights = comfyui::list_weights(&state.db, &id).await?;
    let types: Vec<String> = comfyui::preset_types().into_iter().map(str::to_string).collect();
    Ok(Json(ResultBody::ok(
        ComfyuiWeightsVo { instance_id: id, types, weights },
        state.version,
    )))
}

/// `POST /api/deployments/{id}/comfyui/weights`
pub async fn add_weight(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<AddWeightRequest>,
) -> Result<Json<ResultBody<WeightView>>, ModelinferError> {
    let _ = deployments::get_deployment(&state.db, &id).await?;
    let hub_dir = std::path::PathBuf::from(&state.model_hub_dir);
    let v = comfyui::add_weight(&state.db, &hub_dir, &id, &req).await?;
    Ok(Json(ResultBody::ok(v, state.version)))
}

/// `DELETE /api/deployments/{id}/comfyui/weights/{rid}`
pub async fn delete_weight(
    State(state): State<Arc<AppState>>,
    Path((id, rid)): Path<(String, String)>,
) -> Result<Json<ResultBody<serde_json::Value>>, ModelinferError> {
    let _ = deployments::get_deployment(&state.db, &id).await?;
    let hub_dir = std::path::PathBuf::from(&state.model_hub_dir);
    comfyui::remove_weight(&state.db, &hub_dir, &id, &rid).await?;
    Ok(Json(ResultBody::ok(
        serde_json::json!({ "deleted": rid }),
        state.version,
    )))
}