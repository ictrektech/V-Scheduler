//! model-hub 目录信息 HTTP handlers

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};

use crate::error::{ModelinferError, ResultBody};
use crate::model_hub::{self, ModelHubInfo, ModelHubVariant, VariantFile};
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct MatchQuery {
    #[serde(default)]
    pub weight_id: String,
}

#[derive(Debug, Deserialize)]
pub struct TreeQuery {
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub weight_id: String,
}

#[derive(Debug, Serialize)]
pub struct VariantTreeVo {
    pub source: String,
    pub weight_id: String,
    /// 变体根相对路径（`export/<source>/<weight>/current`）
    pub rel_root: String,
    pub files: Vec<VariantFile>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct VariantListVo {
    pub data: Vec<ModelHubVariant>,
    pub total: usize,
    pub weight_id: String,
}

/// `GET /api/model-hub/info`
pub async fn info(State(state): State<Arc<AppState>>) -> Json<ResultBody<ModelHubInfo>> {
    let info: ModelHubInfo = model_hub::info(std::path::Path::new(&state.model_hub_dir));
    Json(ResultBody::ok(info, state.version))
}

/// `GET /api/model-hub/match?weight_id=<org>/<repo>`
pub async fn match_variants(
    State(state): State<Arc<AppState>>,
    Query(q): Query<MatchQuery>,
) -> Result<Json<ResultBody<VariantListVo>>, ModelinferError> {
    if q.weight_id.trim().is_empty() {
        return Err(ModelinferError::new(
            crate::error::codes::PARAM_INVALID,
            "weight_id 不能为空",
        ));
    }
    let data = model_hub::find_variants(std::path::Path::new(&state.model_hub_dir), &q.weight_id);
    let total = data.len();
    Ok(Json(ResultBody::ok(
        VariantListVo { data, total, weight_id: q.weight_id },
        state.version,
    )))
}

/// `GET /api/model-hub/tree?source=<hf|ms>&weight_id=<org>/<repo>`
pub async fn tree(
    State(state): State<Arc<AppState>>,
    Query(q): Query<TreeQuery>,
) -> Result<Json<ResultBody<VariantTreeVo>>, ModelinferError> {
    if q.weight_id.trim().is_empty() {
        return Err(ModelinferError::new(
            crate::error::codes::PARAM_INVALID,
            "weight_id 不能为空",
        ));
    }
    let source = q.source.trim().to_ascii_lowercase();
    if source.is_empty() {
        return Err(ModelinferError::new(crate::error::codes::PARAM_INVALID, "source 不能为空"));
    }
    let files = model_hub::list_variant_files(
        std::path::Path::new(&state.model_hub_dir),
        &source,
        &q.weight_id,
    )?;
    let total = files.len();
    let weight_id = q.weight_id.trim().trim_matches('/').to_string();
    let rel_root = format!("export/{source}/{weight_id}/current");
    Ok(Json(ResultBody::ok(
        VariantTreeVo { source, weight_id, rel_root, files, total },
        state.version,
    )))
}