//! 模型广场 HTTP handlers（来源 / 卡片 / 下载登记）

use std::sync::Arc;

use axum::{
    Json,
    extract::{Multipart, Path, Query, State},
};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::error::{ModelinferError, ResultBody, codes};
use crate::paging::{GRID_PAGE_SIZE, PageMeta, PageQuery, paginate};
use crate::plaza::{
    self, CardFilter, LocalModelGroupView, LocalModelStats, ModelFilter, ModelSort, PlazaCardGroupView,
    PlazaCardStats, PlazaModelView, RegisterReq, SourceCreate, SourceUpdate, SourceView,
};
use crate::state::AppState;

/// 本地上传模型卡片包大小上限（8 GiB；默认 body limit 2 MiB 不足以传模型包）
pub const MAX_CARD_PACKAGE_BYTES: usize = 8 * 1024 * 1024 * 1024;

/// `GET /api/plaza/sources/{id}/cards` 查询参数（分页 + 筛选）
#[derive(Debug, Default, Deserialize)]
pub struct CardsQuery {
    #[serde(default)]
    pub page_num: Option<u64>,
    #[serde(default)]
    pub page_size: Option<u64>,
    /// 关键词（名称 / 卡片 id / 描述 / tags，大小写不敏感）
    #[serde(default)]
    pub keyword: Option<String>,
    /// 仅看已下载（至少一个版本已登记）
    #[serde(default)]
    pub registered_only: Option<bool>,
    /// 强制重新拉取上游索引并刷新同步元数据（绕过 TTL 缓存）
    #[serde(default)]
    pub refresh: Option<bool>,
}

/// `GET /api/plaza/models` 查询参数（分页 + 筛选 + 排序）
#[derive(Debug, Default, Deserialize)]
pub struct ModelsQuery {
    #[serde(default)]
    pub page_num: Option<u64>,
    #[serde(default)]
    pub page_size: Option<u64>,
    /// 关键词（名称 / 卡片 id / 权重，大小写不敏感）
    #[serde(default)]
    pub keyword: Option<String>,
    /// 家族标识（`all` 表示不限）
    #[serde(default)]
    pub family: Option<String>,
    /// 来源（`plaza` / `local`；`all` 表示不限）
    #[serde(default)]
    pub source: Option<String>,
    /// 排序：`name` / `size` / `versions` / `latest`
    #[serde(default)]
    pub sort: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SourceListVo {
    pub data: Vec<SourceView>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct CardListVo {
    /// 按 `card_id` 聚合的分组（同一卡片的多版本收敛为一张卡片）——当前页
    pub data: Vec<PlazaCardGroupView>,
    /// 分页元数据（`total` 为**筛选后**卡片分组数）
    #[serde(flatten)]
    pub page: PageMeta,
    /// 未筛选统计（页头统计条）
    pub stats: PlazaCardStats,
    pub source_id: String,
}

#[derive(Debug, Serialize)]
pub struct ModelListVo {
    /// 按 `model_id` 聚合的分组——当前页
    pub data: Vec<LocalModelGroupView>,
    /// 分页元数据（`total` 为**筛选后**卡片分组数）
    #[serde(flatten)]
    pub page: PageMeta,
    /// 未筛选统计（页头统计条 / 家族下拉）
    pub stats: LocalModelStats,
    pub plaza_server: String,
}

#[derive(Debug, Deserialize)]
pub struct ManifestQuery {
    /// manifest 相对路径（来自卡片列表的 `manifest` 字段）
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct DeleteVo {
    pub deleted: String,
}

/// `GET /api/plaza/sources`
pub async fn list_sources(State(state): State<Arc<AppState>>) -> Result<Json<ResultBody<SourceListVo>>, ModelinferError> {
    let data = plaza::list_sources(&state.db).await?;
    let total = data.len();
    Ok(Json(ResultBody::ok(SourceListVo { data, total }, state.version)))
}

/// `POST /api/plaza/sources`
pub async fn create_source(
    State(state): State<Arc<AppState>>,
    Json(req): Json<SourceCreate>,
) -> Result<Json<ResultBody<SourceView>>, ModelinferError> {
    let s = plaza::create_source(&state.db, req).await?;
    Ok(Json(ResultBody::ok(s, state.version)))
}

/// `PUT /api/plaza/sources/{id}`
pub async fn update_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Json(req): Json<SourceUpdate>,
) -> Result<Json<ResultBody<SourceView>>, ModelinferError> {
    let s = plaza::update_source(&state.db, &id, req).await?;
    // 地址可能变化 → 失效该来源的卡片索引缓存
    plaza::invalidate_cache(&state.plaza_card_cache, &id).await;
    Ok(Json(ResultBody::ok(s, state.version)))
}

/// `DELETE /api/plaza/sources/{id}`
pub async fn delete_source(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<DeleteVo>>, ModelinferError> {
    plaza::delete_source(&state.db, &id).await?;
    plaza::invalidate_cache(&state.plaza_card_cache, &id).await;
    Ok(Json(ResultBody::ok(DeleteVo { deleted: id }, state.version)))
}

/// `GET /api/plaza/sources/{id}/cards`
///
/// 返回**按 `card_id` 聚合**的卡片分组（含全部版本）的当前页，供前端"列出所有版本并选择
/// 指定版本"；`keyword` / `registered_only` 为服务端筛选，`total` 为筛选后分组数。
pub async fn list_cards(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<CardsQuery>,
) -> Result<Json<ResultBody<CardListVo>>, ModelinferError> {
    let cards = plaza::sync_cards(
        state.plaza.as_ref(),
        &state.db,
        &id,
        q.refresh.unwrap_or(false),
        &state.plaza_card_cache,
    )
    .await?;
    let all = plaza::group_cards(cards);
    let stats = plaza::plaza_card_stats(&all);
    let filtered = plaza::filter_cards(
        all,
        &CardFilter {
            keyword: q.keyword.clone().unwrap_or_default(),
            registered_only: q.registered_only.unwrap_or(false),
        },
    );
    let params = PageQuery { page_num: q.page_num, page_size: q.page_size }.resolve(GRID_PAGE_SIZE);
    let (data, page) = paginate(filtered, params);
    Ok(Json(ResultBody::ok(
        CardListVo { data, page, stats, source_id: id },
        state.version,
    )))
}

/// `GET /api/plaza/sources/{id}/manifest?path=<rel>`
pub async fn card_manifest(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(q): Query<ManifestQuery>,
) -> Result<Json<ResultBody<serde_json::Value>>, ModelinferError> {
    let text = plaza::fetch_card_manifest(state.plaza.as_ref(), &state.db, &id, &q.path).await?;
    Ok(Json(ResultBody::ok(
        serde_json::json!({ "manifest": text }),
        state.version,
    )))
}

/// `GET /api/plaza/models`
///
/// 返回**按 `model_id` 聚合**的本地模型卡片分组（含全部版本）的当前页；`keyword` / `family` /
/// `source` 为服务端筛选，`sort` 为服务端排序，`total` 为筛选后分组数。
pub async fn list_models(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ModelsQuery>,
) -> Result<Json<ResultBody<ModelListVo>>, ModelinferError> {
    let models = plaza::list_models(&state.db).await?;
    let all = plaza::group_models(models);
    let stats = plaza::local_model_stats(&all);
    let filtered = plaza::filter_models(
        all,
        &ModelFilter {
            keyword: q.keyword.clone().unwrap_or_default(),
            family: q.family.clone().unwrap_or_else(|| "all".into()),
            source: q.source.clone().unwrap_or_else(|| "all".into()),
            sort: ModelSort::parse(q.sort.as_deref().unwrap_or("name")),
        },
    );
    let params = PageQuery { page_num: q.page_num, page_size: q.page_size }.resolve(GRID_PAGE_SIZE);
    let (data, page) = paginate(filtered, params);
    Ok(Json(ResultBody::ok(
        ModelListVo { data, page, stats, plaza_server: state.plaza_server.clone() },
        state.version,
    )))
}

/// `POST /api/plaza/models`
pub async fn register_model(
    State(state): State<Arc<AppState>>,
    Json(req): Json<RegisterReq>,
) -> Result<Json<ResultBody<PlazaModelView>>, ModelinferError> {
    let m = plaza::register_model(state.plaza.as_ref(), &state.db, &state.plaza_dir, req).await?;
    Ok(Json(ResultBody::ok(m, state.version)))
}

/// `DELETE /api/plaza/models/{id}`
pub async fn delete_model(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> Result<Json<ResultBody<DeleteVo>>, ModelinferError> {
    plaza::delete_model(&state.db, &state.plaza_dir, &id).await?;
    Ok(Json(ResultBody::ok(DeleteVo { deleted: id }, state.version)))
}

/// `POST /api/plaza/models/upload`（`multipart/form-data`，字段 `file`）
///
/// 上传本地模型卡片包（包含 `model.tar.gz` 的 `tar.*` 归档），流式落盘临时文件后登记为
/// 本地模型卡片（`source = "local"`），返回 `PlazaModelView`。
pub async fn upload_local_card(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Json<ResultBody<PlazaModelView>>, ModelinferError> {
    let uploads_dir = state.plaza_dir.join("_uploads");
    tokio::fs::create_dir_all(&uploads_dir).await.map_err(|e| {
        ModelinferError::new(codes::IO_FAILED, format!("创建上传目录失败: {e}"))
    })?;
    // 保留扩展名提示（格式判定以魔数为准）
    let staged = uploads_dir.join(format!("upload-{}.tar", ulid::Ulid::new()));

    let mut filename = String::new();
    let mut found = false;
    let mut size: u64 = 0;
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ModelinferError::new(codes::PARAM_INVALID, format!("解析上传请求失败: {e}")))?
    {
        if field.name() != Some("file") {
            continue;
        }
        filename = field.file_name().unwrap_or_default().to_string();
        let mut file = tokio::fs::File::create(&staged).await.map_err(|e| {
            ModelinferError::new(codes::IO_FAILED, format!("创建上传临时文件失败: {e}"))
        })?;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ModelinferError::new(codes::PARAM_INVALID, format!("读取上传数据失败: {e}")))?
        {
            size += chunk.len() as u64;
            if size > MAX_CARD_PACKAGE_BYTES as u64 {
                let _ = tokio::fs::remove_file(&staged).await;
                return Err(ModelinferError::new(
                    codes::PARAM_INVALID,
                    format!("上传文件超过上限 {} 字节", MAX_CARD_PACKAGE_BYTES),
                ));
            }
            file.write_all(&chunk).await.map_err(|e| {
                ModelinferError::new(codes::IO_FAILED, format!("写入上传文件失败: {e}"))
            })?;
        }
        file.flush().await.map_err(|e| {
            ModelinferError::new(codes::IO_FAILED, format!("落盘上传文件失败: {e}"))
        })?;
        found = true;
        break;
    }

    if !found {
        let _ = tokio::fs::remove_file(&staged).await;
        return Err(ModelinferError::new(
            codes::PARAM_INVALID,
            "缺少上传文件字段 file",
        ));
    }
    tracing::info!(filename = %filename, size, "收到本地模型卡片包上传");

    // 解压 / 落位为阻塞 IO：内部经 spawn_blocking，避免长时间占用异步 worker 线程
    let result = plaza::register_local_card_package(&state.db, &state.plaza_dir, &staged).await;
    let _ = tokio::fs::remove_file(&staged).await;
    let model = result?;
    Ok(Json(ResultBody::ok(model, state.version)))
}