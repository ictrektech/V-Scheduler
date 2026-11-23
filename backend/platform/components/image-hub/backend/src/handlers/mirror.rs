//! 镜像源（mirror）目录 handler

use axum::{Json, extract::State};
use serde::Serialize;

use crate::error::ResultBody;
use crate::mirror::{MirrorOption, mirror_catalog};
use crate::state::AppState;

/// 镜像源目录响应
#[derive(Serialize)]
pub struct MirrorListData {
    pub count: usize,
    pub mirrors: Vec<MirrorOption>,
}

/// `GET /api/registry/mirrors` —— 内置镜像源目录（上游 registry → 候选镜像源）
pub async fn list_mirrors(State(state): State<AppState>) -> Json<ResultBody<MirrorListData>> {
    let mirrors = mirror_catalog();
    Json(ResultBody::ok(
        MirrorListData {
            count: mirrors.len(),
            mirrors,
        },
        state.version,
    ))
}