use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};

use crate::error::ResultBody;
use crate::registry::{
    DeleteResult, RepositoryDetail, RepositoryImages, collect_images, collect_repository,
    delete_image,
};
use crate::state::AppState;

/// `GET /api/registry/info` 响应：前端据此拼 `<ip>:<port>` 展示地址
#[derive(Serialize)]
pub struct RegistryInfo {
    pub registry_port: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published_address: Option<String>,
}

/// 镜像列表响应
#[derive(Serialize)]
pub struct ImageListData {
    pub count: usize,
    pub images: Vec<RepositoryImages>,
}

/// 镜像详情查询参数
#[derive(Deserialize)]
pub struct ImageDetailQuery {
    pub repository: String,
}

/// 删除请求体（tag 缺省 = 删除整仓全部标签）
#[derive(Deserialize)]
pub struct DeleteImageRequest {
    pub repository: String,
    #[serde(default)]
    pub tag: Option<String>,
}

/// `GET /api/registry/info`
pub async fn registry_info(State(state): State<AppState>) -> Json<ResultBody<RegistryInfo>> {
    Json(ResultBody::ok(
        RegistryInfo {
            registry_port: state.registry_port,
            published_address: std::env::var("SCHEDULER_IMAGE_HUB_BIND")
                .ok()
                .filter(|address| !address.trim().is_empty()),
        },
        state.version,
    ))
}

/// `GET /api/registry/images` —— 枚举 registry 中的仓库与标签
pub async fn list_images(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<ImageListData>>, crate::error::ImageHubError> {
    let images = collect_images(state.registry.as_ref()).await?;
    Ok(Json(ResultBody::ok(
        ImageListData {
            count: images.len(),
            images,
        },
        state.version,
    )))
}

/// `GET /api/registry/images/detail?repository=<name>` —— 单仓库标签详情（含多架构平台条目）
pub async fn image_detail(
    State(state): State<AppState>,
    Query(query): Query<ImageDetailQuery>,
) -> Result<Json<ResultBody<RepositoryDetail>>, crate::error::ImageHubError> {
    let detail = collect_repository(state.registry.as_ref(), query.repository.trim()).await?;
    Ok(Json(ResultBody::ok(detail, state.version)))
}

/// `DELETE /api/registry/images` —— 删除单标签（tag）或整仓（tag 缺省）
pub async fn remove_image(
    State(state): State<AppState>,
    Json(req): Json<DeleteImageRequest>,
) -> Result<Json<ResultBody<DeleteResult>>, crate::error::ImageHubError> {
    let result = delete_image(
        state.registry.as_ref(),
        req.repository.trim(),
        req.tag.as_deref().map(str::trim).filter(|s| !s.is_empty()),
    )
    .await?;
    Ok(Json(ResultBody::ok(result, state.version)))
}