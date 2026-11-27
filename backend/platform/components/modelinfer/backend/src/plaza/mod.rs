//! 模型广场领域逻辑：来源 CRUD、卡片浏览、模型包下载与登记。

pub mod card_package;
pub mod client;
pub mod model_index;
pub mod store;

use std::path::Path;
use std::sync::Arc;

use sea_orm::{
    ActiveModelTrait, DatabaseConnection, EntityTrait, PaginatorTrait, QueryOrder, Set,
};
use serde::{Deserialize, Serialize};

use crate::entities::{plaza_model, plaza_source};
use crate::error::{ModelinferError, codes};
use crate::time;

pub use client::{MockPlazaClient, PlazaClient, RealPlazaClient};
pub use model_index::{
    ManifestMeta, ModelIndexEntry, join_url, normalize_base_url, parse_index, parse_manifest,
};
pub use store::{card_dir, package_path};

// ============ DTO ============

#[derive(Debug, Clone, Deserialize)]
pub struct SourceCreate {
    pub name: String,
    pub base_url: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SourceUpdate {
    pub name: Option<String>,
    pub base_url: Option<String>,
    pub enabled: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SourceView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub last_synced_at: Option<String>,
    pub card_count: i32,
    pub created_at: String,
    pub updated_at: String,
}

impl From<plaza_source::Model> for SourceView {
    fn from(m: plaza_source::Model) -> Self {
        Self {
            id: m.id,
            name: m.name,
            base_url: m.base_url,
            enabled: m.enabled,
            last_synced_at: m.last_synced_at,
            card_count: m.card_count,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// 卡片视图（列表展示 + 是否已登记）
///
/// `name` / `description` 为卡片 manifest 的多语言值（对象或字符串），前端经 `displayText`
/// 取展示值；不再有独立的 `display_name` 字段。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardView {
    pub id: String,
    pub name: Option<serde_json::Value>,
    pub version: String,
    pub description: Option<serde_json::Value>,
    pub category: Vec<String>,
    pub tags: Vec<String>,
    pub icon: String,
    /// 权重 id（manifest `model`；索引未提供时为空）
    #[serde(default)]
    pub model: String,
    /// 镜像声明（manifest `images`；索引未提供时为空）
    #[serde(default)]
    pub images: Vec<serde_json::Value>,
    pub manifest: String,
    pub package: String,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    /// 是否已登记到本地
    pub registered: bool,
    /// 已登记主键 `<card_id>@<version>`
    pub plaza_model_id: Option<String>,
}

/// 已登记模型视图
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlazaModelView {
    pub id: String,
    pub model_id: String,
    pub model: String,
    pub name: String,
    pub version: String,
    pub source: String,
    pub source_id: String,
    pub path: String,
    pub package_path: String,
    pub package_sha256: String,
    pub manifest_json: String,
    pub state: String,
    pub size_bytes: i64,
    pub created_at: String,
    pub updated_at: String,
}

impl From<plaza_model::Model> for PlazaModelView {
    fn from(m: plaza_model::Model) -> Self {
        Self {
            id: m.id,
            model_id: m.model_id,
            model: m.model,
            name: m.name,
            version: m.version,
            source: m.source,
            source_id: m.source_id,
            path: m.path,
            package_path: m.package_path,
            package_sha256: m.package_sha256,
            manifest_json: m.manifest_json,
            state: m.state,
            size_bytes: m.size_bytes,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// 广场卡片的一个版本（分组内的版本级条目）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CardVersionView {
    pub version: String,
    pub size_bytes: Option<i64>,
    pub sha256: Option<String>,
    /// manifest 绝对 URL
    pub manifest: String,
    /// model.tar.gz 绝对 URL
    pub package: String,
    /// 该版本是否已登记到本地
    pub registered: bool,
    /// 已登记主键 `<card_id>@<version>`
    pub plaza_model_id: Option<String>,
}

/// 广场卡片分组视图：同一 `card_id` 的多个版本聚合为一张卡片
///
/// 展示元数据（`name` / `description` / `category` / `tags` / `icon`）取 `latest_version`
/// 对应条目的值（MUST NOT 由多版本拼合）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlazaCardGroupView {
    pub id: String,
    pub name: Option<serde_json::Value>,
    pub description: Option<serde_json::Value>,
    pub category: Vec<String>,
    pub tags: Vec<String>,
    pub icon: String,
    /// 最新版本声明的权重 id
    #[serde(default)]
    pub model: String,
    /// 最新版本声明的镜像
    #[serde(default)]
    pub images: Vec<serde_json::Value>,
    /// 最新版本号（版本号最大者）
    pub latest_version: String,
    /// 已登记版本数
    pub registered_count: usize,
    /// 家族标识（后端 SSOT 推断，未命中为 `generic`）
    pub family: String,
    /// 版本列表（降序）
    pub versions: Vec<CardVersionView>,
}

/// 本地模型卡片的一个版本（`PlazaModelView` + 是否最新）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelVersionView {
    #[serde(flatten)]
    pub model: PlazaModelView,
    /// 是否为该卡片的最新版本
    pub latest: bool,
}

/// 本地模型卡片分组视图：同一 `model_id`（卡片 id）的多个已登记版本聚合为一张卡片
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelGroupView {
    /// 模型卡片 id（manifest `id`，逆序 DNS）
    pub model_id: String,
    /// 展示名（取最新版本的登记名）
    pub name: String,
    /// 权重 id（manifest `model`）
    pub model: String,
    /// 来源集合（`plaza` / `local`，去重且顺序稳定）
    pub sources: Vec<String>,
    /// 最新版本声明的镜像（manifest `images`）
    #[serde(default)]
    pub images: Vec<serde_json::Value>,
    /// 最新版本号
    pub latest_version: String,
    /// 各版本 `model.tar.gz` 大小之和（字节）
    pub total_size_bytes: i64,
    /// 家族标识（后端 SSOT 推断，未命中为 `generic`）
    pub family: String,
    /// 版本列表（降序，含 `latest` 标记）
    pub versions: Vec<LocalModelVersionView>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegisterReq {
    pub source_id: String,
    pub model_id: String,
    pub version: String,
}

// ============ 卡片 / 版本聚合（纯函数） ============

/// 单个版本段的比较：两段均为数字按数值比较，否则按字典序
fn compare_version_seg(a: &str, b: &str) -> std::cmp::Ordering {
    match (a.parse::<u64>(), b.parse::<u64>()) {
        (Ok(x), Ok(y)) => x.cmp(&y),
        _ => a.cmp(b),
    }
}

/// 版本降序比较（纯函数）：返回 `Less` 表示 `a` 应排在 `b` **之前**（`a` 更新）
///
/// 规则：按 `.` 切分逐段比较（数字段按数值，否则字典序）；共同前缀相同时段数多者更新。
/// 非 Semver 输入不 panic，按字典序兜底。
pub fn compare_versions_desc(a: &str, b: &str) -> std::cmp::Ordering {
    let a = a.trim();
    let b = b.trim();
    let av: Vec<&str> = a.split('.').collect();
    let bv: Vec<&str> = b.split('.').collect();
    for (x, y) in av.iter().zip(bv.iter()) {
        let ord = compare_version_seg(x, y);
        if ord != std::cmp::Ordering::Equal {
            // 段值大的版本更新 → 排在前面（降序）
            return ord.reverse();
        }
    }
    // 共同前缀相同：段数多者更新
    bv.len().cmp(&av.len())
}

/// 按 `card_id` 聚合广场卡片（纯函数），分组与版本均稳定有序
pub fn group_cards(cards: Vec<CardView>) -> Vec<PlazaCardGroupView> {
    let mut grouped: std::collections::BTreeMap<String, Vec<CardView>> =
        std::collections::BTreeMap::new();
    for c in cards {
        grouped.entry(c.id.clone()).or_default().push(c);
    }
    grouped
        .into_iter()
        .map(|(id, mut versions)| {
            versions.sort_by(|a, b| compare_versions_desc(&a.version, &b.version));
            let latest = versions.first().cloned();
            let registered_count = versions.iter().filter(|v| v.registered).count();
            let (name, description, category, tags, icon, model, images, latest_version) = match latest {
                Some(l) => (
                    l.name,
                    l.description,
                    l.category,
                    l.tags,
                    l.icon,
                    l.model,
                    l.images,
                    l.version,
                ),
                None => (
                    None,
                    None,
                    Vec::new(),
                    Vec::new(),
                    String::new(),
                    String::new(),
                    Vec::new(),
                    String::new(),
                ),
            };
            let family = crate::model_family::infer_from_json_name(
                &tags,
                None,
                Some(&id),
                name.as_ref(),
            )
            .to_string();
            PlazaCardGroupView {
                id,
                name,
                description,
                category,
                tags,
                icon,
                model,
                images,
                latest_version,
                registered_count,
                family,
                versions: versions
                    .into_iter()
                    .map(|v| CardVersionView {
                        version: v.version,
                        size_bytes: v.size_bytes,
                        sha256: v.sha256,
                        manifest: v.manifest,
                        package: v.package,
                        registered: v.registered,
                        plaza_model_id: v.plaza_model_id,
                    })
                    .collect(),
            }
        })
        .collect()
}

/// 按 `model_id` 聚合本地模型卡片（纯函数），版本降序并标记最新
pub fn group_models(models: Vec<PlazaModelView>) -> Vec<LocalModelGroupView> {
    let mut grouped: std::collections::BTreeMap<String, Vec<PlazaModelView>> =
        std::collections::BTreeMap::new();
    for m in models {
        grouped.entry(m.model_id.clone()).or_default().push(m);
    }
    grouped
        .into_iter()
        .map(|(model_id, mut versions)| {
            versions.sort_by(|a, b| compare_versions_desc(&a.version, &b.version));
            let latest = versions.first().cloned();
            let (name, model, latest_version) = match latest {
                Some(l) => (l.name, l.model, l.version),
                None => (model_id.clone(), String::new(), String::new()),
            };
            let mut sources: Vec<String> =
                versions.iter().map(|v| v.source.clone()).collect();
            sources.sort();
            sources.dedup();
            let total_size_bytes = versions.iter().map(|v| v.size_bytes).sum();
            // 家族推断取最新版本 manifest 的 tags（解析失败按空处理，不阻断列表）
            let latest_manifest = versions
                .first()
                .and_then(|m| serde_json::from_str::<serde_json::Value>(&m.manifest_json).ok());
            let latest_tags = latest_manifest
                .as_ref()
                .and_then(|m| m.get("tags").cloned())
                .and_then(|t| t.as_array().cloned())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_str().map(str::to_string))
                        .collect::<Vec<String>>()
                })
                .unwrap_or_default();
            let images = latest_manifest
                .as_ref()
                .and_then(|m| m.get("images"))
                .and_then(|v| v.as_array().cloned())
                .unwrap_or_default();
            let family =
                crate::model_family::infer(&latest_tags, Some(&model), Some(&model_id), Some(&name))
                    .to_string();
            LocalModelGroupView {
                model_id,
                name,
                model,
                sources,
                images,
                latest_version,
                total_size_bytes,
                family,
                versions: versions
                    .into_iter()
                    .enumerate()
                    .map(|(idx, m)| LocalModelVersionView { model: m, latest: idx == 0 })
                    .collect(),
            }
        })
        .collect()
}

fn default_true() -> bool {
    true
}

// ============ 列表筛选 / 排序（纯函数，供分页前端参数使用） ============

/// 广场卡片筛选参数
#[derive(Debug, Clone, Default)]
pub struct CardFilter {
    /// 关键词（大小写不敏感），匹配 名称 / 卡片 id / 描述 / tags
    pub keyword: String,
    /// 仅看已下载（至少有一个版本已登记）
    pub registered_only: bool,
}

/// 广场卡片的可搜索文本（与前端治理前字段集合一致）
pub fn card_search_text(card: &PlazaCardGroupView) -> String {
    let mut parts = vec![
        card.name
            .as_ref()
            .and_then(crate::model_family::display_text)
            .unwrap_or_default(),
        card.id.clone(),
        card.description
            .as_ref()
            .and_then(crate::model_family::display_text)
            .unwrap_or_default(),
    ];
    parts.extend(card.tags.iter().cloned());
    parts.push(card.model.clone());
    parts.push(
        serde_json::to_string(&card.images)
            .unwrap_or_default()
            .trim_matches(['[', ']', '"'])
            .replace('"', " "),
    );
    parts.join(" ").to_lowercase()
}

/// 按关键词 / 仅看已下载筛选广场卡片（保持输入顺序）
pub fn filter_cards(cards: Vec<PlazaCardGroupView>, f: &CardFilter) -> Vec<PlazaCardGroupView> {
    let kw = f.keyword.trim().to_lowercase();
    cards
        .into_iter()
        .filter(|c| {
            if f.registered_only && c.registered_count == 0 {
                return false;
            }
            if kw.is_empty() {
                return true;
            }
            card_search_text(c).contains(&kw)
        })
        .collect()
}

/// 本地卡片排序方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelSort {
    /// 名称（Unicode 码点序；不做拼音排序，见 design §6）
    Name,
    /// 卡片包总大小（降序）
    Size,
    /// 版本数（降序）
    Versions,
    /// 最新版本登记时间（降序）
    Latest,
}

impl ModelSort {
    /// 解析排序取值，未知值回落 `Name`
    pub fn parse(value: &str) -> Self {
        match value {
            "size" => Self::Size,
            "versions" => Self::Versions,
            "latest" => Self::Latest,
            _ => Self::Name,
        }
    }
}

/// 本地卡片筛选参数
#[derive(Debug, Clone)]
pub struct ModelFilter {
    /// 关键词（大小写不敏感），匹配 名称 / 卡片 id / 权重
    pub keyword: String,
    /// 家族标识；`all` / 空表示不限
    pub family: String,
    /// 来源（`plaza` / `local`）；`all` / 空表示不限
    pub source: String,
    pub sort: ModelSort,
}

impl Default for ModelFilter {
    fn default() -> Self {
        Self {
            keyword: String::new(),
            family: "all".into(),
            source: "all".into(),
            sort: ModelSort::Name,
        }
    }
}

/// 本地卡片的可搜索文本（与前端治理前字段集合一致）
pub fn model_search_text(m: &LocalModelGroupView) -> String {
    let mut parts = vec![m.name.clone(), m.model_id.clone(), m.model.clone()];
    parts.push(serde_json::to_string(&m.images).unwrap_or_default());
    parts.join(" ").to_lowercase()
}

/// 按关键词 / 家族 / 来源筛选并排序本地卡片
pub fn filter_models(models: Vec<LocalModelGroupView>, f: &ModelFilter) -> Vec<LocalModelGroupView> {
    let kw = f.keyword.trim().to_lowercase();
    let mut list: Vec<LocalModelGroupView> = models
        .into_iter()
        .filter(|m| {
            if f.family != "all" && !f.family.is_empty() && m.family != f.family {
                return false;
            }
            if f.source != "all" && !f.source.is_empty() && !m.sources.iter().any(|s| s == &f.source)
            {
                return false;
            }
            if kw.is_empty() {
                return true;
            }
            model_search_text(m).contains(&kw)
        })
        .collect();

    list.sort_by(|a, b| match f.sort {
        ModelSort::Size => b.total_size_bytes.cmp(&a.total_size_bytes),
        ModelSort::Versions => b.versions.len().cmp(&a.versions.len()),
        ModelSort::Latest => {
            let at = a.versions.first().map(|v| v.model.created_at.as_str()).unwrap_or("");
            let bt = b.versions.first().map(|v| v.model.created_at.as_str()).unwrap_or("");
            bt.cmp(at)
        }
        ModelSort::Name => a.name.cmp(&b.name),
    });
    list
}

/// 本地卡片的未筛选统计（页头统计条 / 家族下拉用）
#[derive(Debug, Clone, Serialize)]
pub struct LocalModelStats {
    pub cards: usize,
    pub versions: usize,
    pub size_bytes: i64,
    /// 出现过的家族标识（去重，按标识排序）
    pub families: Vec<String>,
}

/// 计算本地卡片的未筛选统计
pub fn local_model_stats(models: &[LocalModelGroupView]) -> LocalModelStats {
    use std::collections::BTreeSet;
    let mut families: BTreeSet<String> = BTreeSet::new();
    let mut versions = 0usize;
    let mut size_bytes = 0i64;
    for m in models {
        versions += m.versions.len();
        size_bytes += m.total_size_bytes;
        families.insert(m.family.clone());
    }
    LocalModelStats {
        cards: models.len(),
        versions,
        size_bytes,
        families: families.into_iter().collect(),
    }
}

/// 广场卡片的未筛选统计
#[derive(Debug, Clone, Serialize)]
pub struct PlazaCardStats {
    /// 未筛选卡片分组数
    pub cards: usize,
    /// 已登记（已下载）版本数
    pub downloaded: usize,
}

/// 计算广场卡片的未筛选统计
pub fn plaza_card_stats(cards: &[PlazaCardGroupView]) -> PlazaCardStats {
    PlazaCardStats {
        cards: cards.len(),
        downloaded: cards.iter().map(|c| c.registered_count).sum(),
    }
}

fn db_err(e: sea_orm::DbErr) -> ModelinferError {
    ModelinferError::new(codes::DB_FAILED, format!("数据库操作失败: {e}"))
}

/// 卡片主键 `<card_id>@<version>`
pub fn card_pk(card_id: &str, version: &str) -> String {
    format!("{card_id}@{version}")
}

/// 校验来源 URL
pub fn validate_base_url(url: &str) -> Result<String, ModelinferError> {
    let normalized = normalize_base_url(url);
    if !(normalized.starts_with("http://") || normalized.starts_with("https://")) {
        return Err(ModelinferError::new(
            codes::PARAM_INVALID,
            "来源地址必须以 http:// 或 https:// 开头",
        ));
    }
    Ok(normalized)
}

/// 相对/绝对路径 → 绝对 URL
pub fn resolve_url(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") {
        rel.to_string()
    } else {
        join_url(base, rel)
    }
}

// ============ 来源 CRUD ============

pub async fn list_sources(db: &DatabaseConnection) -> Result<Vec<SourceView>, ModelinferError> {
    let rows = plaza_source::Entity::find()
        .order_by_asc(plaza_source::Column::CreatedAt)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.into_iter().map(SourceView::from).collect())
}

pub async fn get_source(db: &DatabaseConnection, id: &str) -> Result<plaza_source::Model, ModelinferError> {
    plaza_source::Entity::find_by_id(id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ModelinferError::new(codes::NOT_FOUND, format!("来源不存在: {id}")))
}

pub async fn create_source(
    db: &DatabaseConnection,
    req: SourceCreate,
) -> Result<SourceView, ModelinferError> {
    if req.name.trim().is_empty() {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "来源名称不能为空"));
    }
    let base_url = validate_base_url(&req.base_url)?;
    let now = time::now_iso();
    let am = plaza_source::ActiveModel {
        id: Set(ulid::Ulid::new().to_string()),
        name: Set(req.name.trim().to_string()),
        base_url: Set(base_url),
        enabled: Set(req.enabled),
        last_synced_at: Set(None),
        card_count: Set(0),
        created_at: Set(now.clone()),
        updated_at: Set(now),
    };
    let inserted = am.insert(db).await.map_err(db_err)?;
    Ok(SourceView::from(inserted))
}

pub async fn update_source(
    db: &DatabaseConnection,
    id: &str,
    req: SourceUpdate,
) -> Result<SourceView, ModelinferError> {
    let current = get_source(db, id).await?;
    let mut am: plaza_source::ActiveModel = current.into();
    if let Some(name) = req.name {
        if name.trim().is_empty() {
            return Err(ModelinferError::new(codes::PARAM_INVALID, "来源名称不能为空"));
        }
        am.name = Set(name.trim().to_string());
    }
    if let Some(url) = req.base_url {
        am.base_url = Set(validate_base_url(&url)?);
    }
    if let Some(enabled) = req.enabled {
        am.enabled = Set(enabled);
    }
    am.updated_at = Set(time::now_iso());
    let updated = am.update(db).await.map_err(db_err)?;
    Ok(SourceView::from(updated))
}

pub async fn delete_source(db: &DatabaseConnection, id: &str) -> Result<(), ModelinferError> {
    let result = plaza_source::Entity::delete_by_id(id.to_string())
        .exec(db)
        .await
        .map_err(db_err)?;
    if result.rows_affected == 0 {
        return Err(ModelinferError::new(codes::NOT_FOUND, format!("来源不存在: {id}")));
    }
    Ok(())
}

/// 若无来源且提供了默认地址，则种子一条
pub async fn seed_default_source(
    db: &DatabaseConnection,
    default_url: &str,
) -> Result<(), ModelinferError> {
    if default_url.trim().is_empty() {
        return Ok(());
    }
    let count = plaza_source::Entity::find().count(db).await.map_err(db_err)?;
    if count > 0 {
        return Ok(());
    }
    create_source(
        db,
        SourceCreate {
            name: "默认来源".to_string(),
            base_url: default_url.to_string(),
            enabled: true,
        },
    )
    .await?;
    Ok(())
}

// ============ 卡片浏览 ============

/// 广场卡片索引缓存 TTL（秒）
///
/// 分页后同一次浏览会产生多次请求；缓存避免每次翻页都拉取上游整份索引，也让来源的
/// `last_synced_at` 只在**真实拉取**时更新（见 design.md §7）。
pub const PLAZA_INDEX_TTL_SECS: u64 = 60;

/// 某来源的卡片索引缓存条目
#[derive(Debug, Clone)]
pub struct CachedCards {
    /// 拉取时刻（单调时钟）
    pub fetched_at: std::time::Instant,
    pub cards: Vec<CardView>,
}

/// 广场卡片索引缓存（`source_id` → 条目），进程内存，重启即失效
pub type CardCache = Arc<tokio::sync::Mutex<std::collections::HashMap<String, CachedCards>>>;

/// 新建空的卡片索引缓存
pub fn new_card_cache() -> CardCache {
    Arc::new(tokio::sync::Mutex::new(std::collections::HashMap::new()))
}

/// 读缓存（未过期时返回克隆）
async fn read_cache(cache: &CardCache, source_id: &str) -> Option<Vec<CardView>> {
    let guard = cache.lock().await;
    let entry = guard.get(source_id)?;
    if entry.fetched_at.elapsed() < std::time::Duration::from_secs(PLAZA_INDEX_TTL_SECS) {
        Some(entry.cards.clone())
    } else {
        None
    }
}

/// 失效某来源的缓存（来源更新 / 删除时调用）
pub async fn invalidate_cache(cache: &CardCache, source_id: &str) {
    cache.lock().await.remove(source_id);
}

/// 拉取来源的卡片列表（带短 TTL 缓存；`refresh=true` 强制重新拉取并更新同步元数据）
pub async fn sync_cards(
    plaza: &dyn PlazaClient,
    db: &DatabaseConnection,
    source_id: &str,
    refresh: bool,
    cache: &CardCache,
) -> Result<Vec<CardView>, ModelinferError> {
    if !refresh && let Some(cards) = read_cache(cache, source_id).await {
        return Ok(cards);
    }

    let source = get_source(db, source_id).await?;
    let yaml = plaza.fetch_index(&source.base_url).await?;
    let index = parse_index(&yaml)?;
    let base = source.base_url.clone();

    // 已登记集合
    let registered: Vec<plaza_model::Model> = plaza_model::Entity::find()
        .all(db)
        .await
        .map_err(db_err)?;
    let registered_ids: std::collections::HashSet<String> =
        registered.iter().map(|m| m.id.clone()).collect();

    // 更新同步元数据（仅在真实拉取时）
    let now = time::now_iso();
    let mut am: plaza_source::ActiveModel = source.into();
    am.last_synced_at = Set(Some(now.clone()));
    am.card_count = Set(index.models.len() as i32);
    am.updated_at = Set(now);
    am.update(db).await.map_err(db_err)?;

    let cards: Vec<CardView> = index
        .models
        .into_iter()
        .map(|e| {
            let pk = card_pk(&e.id, &e.version);
            let is_registered = registered_ids.contains(&pk);
            CardView {
                id: e.id,
                name: e.name,
                version: e.version,
                description: e.description,
                category: e.category,
                tags: e.tags,
                icon: if e.icon.trim().is_empty() {
                    String::new()
                } else {
                    resolve_url(&base, &e.icon)
                },
                model: e.model,
                images: e.images,
                manifest: resolve_url(&base, &e.manifest),
                package: resolve_url(&base, &e.package),
                size_bytes: e.size_bytes,
                sha256: e.sha256,
                registered: is_registered,
                plaza_model_id: if is_registered { Some(pk) } else { None },
            }
        })
        .collect();

    cache.lock().await.insert(
        source_id.to_string(),
        CachedCards { fetched_at: std::time::Instant::now(), cards: cards.clone() },
    );
    Ok(cards)
}

/// 拉取某卡片的 manifest 文本
pub async fn fetch_card_manifest(
    plaza: &dyn PlazaClient,
    db: &DatabaseConnection,
    source_id: &str,
    card_rel: &str,
) -> Result<String, ModelinferError> {
    let source = get_source(db, source_id).await?;
    let url = resolve_url(&source.base_url, card_rel);
    plaza.fetch_text(&url).await
}

// ============ 下载登记 ============

/// 下载并登记模型卡片
pub async fn register_model(
    plaza: &dyn PlazaClient,
    db: &DatabaseConnection,
    plaza_dir: &Path,
    req: RegisterReq,
) -> Result<PlazaModelView, ModelinferError> {
    if req.model_id.trim().is_empty() || req.version.trim().is_empty() {
        return Err(ModelinferError::new(codes::PARAM_INVALID, "model_id / version 不能为空"));
    }
    let source = get_source(db, &req.source_id).await?;
    let yaml = plaza.fetch_index(&source.base_url).await?;
    let index = parse_index(&yaml)?;
    let entry = index
        .models
        .into_iter()
        .find(|e| e.id == req.model_id && e.version == req.version)
        .ok_or_else(|| {
            ModelinferError::new(
                codes::NOT_FOUND,
                format!("来源中不存在卡片 {}@{}", req.model_id, req.version),
            )
        })?;

    // 拉取并解析 manifest
    let manifest_url = resolve_url(&source.base_url, &entry.manifest);
    let manifest_text = plaza.fetch_text(&manifest_url).await?;
    let meta = parse_manifest(&manifest_text)?;
    if meta.id != entry.id {
        return Err(ModelinferError::new(
            codes::MANIFEST_INVALID,
            format!("manifest id ({}) 与索引 ({}) 不一致", meta.id, entry.id),
        ));
    }

    // 下载到临时文件
    let dir = store::card_dir(plaza_dir, &entry.id, &entry.version);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建卡片目录失败: {e}")))?;
    let pkg_final = store::package_path(&dir);
    let tmp = dir.join("model.tar.gz.tmp");
    let pkg_url = resolve_url(&source.base_url, &entry.package);
    plaza.download(&pkg_url, &tmp).await?;

    // 校验 sha256（若索引提供）
    let sha256 = store::sha256_file(&tmp)?;
    if let Some(expected) = entry.sha256.as_ref().filter(|s| !s.trim().is_empty()) {
        let expected = expected.trim().to_ascii_lowercase();
        if expected != sha256 {
            let _ = std::fs::remove_file(&tmp);
            return Err(ModelinferError::new(
                codes::CHECKSUM_MISMATCH,
                format!("sha256 不匹配：期望 {expected}，实际 {sha256}"),
            ));
        }
    }

    // 原子改名 + 解压
    std::fs::rename(&tmp, &pkg_final)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("重命名模型包失败: {e}")))?;
    let src_dir = dir.join("src");
    let _ = std::fs::remove_dir_all(&src_dir);
    store::extract_tar_gz(&pkg_final, &src_dir)?;
    let extracted = store::read_manifest(&src_dir)?;
    if extracted.id != entry.id || extracted.model != meta.model {
        return Err(ModelinferError::new(
            codes::MANIFEST_INVALID,
            "解压后的 manifest 与索引/manifest 快照不一致",
        ));
    }

    let size = std::fs::metadata(&pkg_final).map(|m| m.len() as i64).unwrap_or(0);
    let manifest_json = serde_json::to_string(&meta.raw).unwrap_or_else(|_| "{}".to_string());
    let pk = card_pk(&entry.id, &entry.version);
    let now = time::now_iso();

    // upsert
    let existing = plaza_model::Entity::find_by_id(pk.clone())
        .one(db)
        .await
        .map_err(db_err)?;
    let display_name = display_name_of(&meta);
    let model_row = plaza_model::ActiveModel {
        id: Set(pk.clone()),
        model_id: Set(entry.id.clone()),
        model: Set(meta.model.clone()),
        name: Set(display_name),
        version: Set(entry.version.clone()),
        source: Set("plaza".to_string()),
        source_id: Set(source.id.clone()),
        path: Set(src_dir.display().to_string()),
        package_path: Set(pkg_final.display().to_string()),
        package_sha256: Set(sha256),
        manifest_json: Set(manifest_json),
        state: Set("available".to_string()),
        size_bytes: Set(size),
        created_at: Set(existing
            .as_ref()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(|| now.clone())),
        updated_at: Set(now),
    };
    let saved = if existing.is_some() {
        model_row.update(db).await.map_err(db_err)?
    } else {
        model_row.insert(db).await.map_err(db_err)?
    };
    Ok(PlazaModelView::from(saved))
}

/// 从已落盘的本地模型卡片包（外层 `tar.*` 归档）登记为本地模型卡片
///
/// 流程：安全解压外层归档 → 定位 `model.tar.gz`（或含 `manifest.yaml` 的目录并重打包）
/// → 校验 manifest → 落位 `<plaza_dir>/<card_id>/<version>/{model.tar.gz,src/}`
/// → upsert `modelinfer_plaza_models`（`source = "local"`）。
///
/// 同一 `<card_id>@<version>` 再次上传为**幂等覆盖**（保留原 `created_at`）。
/// 解压 / 落位属阻塞 IO，放入 `spawn_blocking`，避免长时间占用异步 worker 线程。
pub async fn register_local_card_package(
    db: &DatabaseConnection,
    plaza_dir: &Path,
    staged_path: &Path,
) -> Result<PlazaModelView, ModelinferError> {
    let work = plaza_dir
        .join("_uploads")
        .join(format!("work-{}", ulid::Ulid::new()));
    std::fs::create_dir_all(&work)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建临时目录失败: {e}")))?;

    let result = match place_local_card(staged_path, plaza_dir, &work).await {
        Ok(placed) => finish_local_card(db, placed).await,
        Err(e) => Err(e),
    };

    // 尽力清理临时目录
    if let Err(e) = std::fs::remove_dir_all(&work) {
        tracing::warn!(path = %work.display(), error = %e, "清理本地上传临时目录失败");
    }
    result
}

/// 已落位的本地卡片（阻塞 IO 段的产物；路径均为最终位置）
#[derive(Debug)]
struct PlacedLocalCard {
    id: String,
    version: String,
    model: String,
    name: String,
    manifest_json: String,
    package_path: String,
    src_path: String,
    package_sha256: String,
    size_bytes: i64,
}

/// 解压 → 定位 → 校验 → 落位（阻塞 IO，经 `spawn_blocking` 调用）
async fn place_local_card(
    staged_path: &Path,
    plaza_dir: &Path,
    work: &Path,
) -> Result<PlacedLocalCard, ModelinferError> {
    let staged = staged_path.to_path_buf();
    let plaza = plaza_dir.to_path_buf();
    let work = work.to_path_buf();
    let handle = tokio::task::spawn_blocking(move || place_local_card_blocking(&staged, &plaza, &work));
    handle
        .await
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("上传处理任务失败: {e}")))?
}

fn place_local_card_blocking(
    staged_path: &Path,
    plaza_dir: &Path,
    work: &Path,
) -> Result<PlacedLocalCard, ModelinferError> {
    // 1. 解压外层归档（按魔数识别 tar / tar.gz / tar.xz）
    let outer = work.join("outer");
    let format = card_package::unpack_archive(staged_path, &outer)?;
    tracing::info!(?format, "本地模型卡片包外层归档已解压");

    // 2. 定位模型定义包
    let source = card_package::locate_card_source(&outer).ok_or_else(|| {
        ModelinferError::new(
            codes::CARD_PACKAGE_INVALID,
            "归档中未找到模型定义包（model.tar.gz）或 manifest.yaml",
        )
    })?;

    // 3. 统一为「卡片源目录 + 模型包文件（位于 work 内）」
    let (src_dir, package_tmp) = match source {
        card_package::CardSource::Package(pkg) => {
            let extracted = work.join("card");
            card_package::unpack_archive(&pkg, &extracted)?;
            (extracted, pkg)
        }
        card_package::CardSource::Dir(dir) => {
            let packed = work.join("model.tar.gz");
            card_package::pack_card_dir(&dir, &packed)?;
            (dir, packed)
        }
    };

    // 4. 解析并校验 manifest
    let meta = store::read_manifest(&src_dir)?;

    // 5. 落位到 <plaza_dir>/<card_id>/<version>/
    let dir = store::card_dir(plaza_dir, &meta.id, &meta.version);
    std::fs::create_dir_all(&dir)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建卡片目录失败: {e}")))?;
    let pkg_final = store::package_path(&dir);
    let pkg_tmp = dir.join("model.tar.gz.tmp");
    std::fs::copy(&package_tmp, &pkg_tmp)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("复制模型包失败: {e}")))?;
    let package_sha256 = store::sha256_file(&pkg_tmp)?;
    std::fs::rename(&pkg_tmp, &pkg_final)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("重命名模型包失败: {e}")))?;

    // 6. 解压最终模型包到 src/ 并复核 manifest 一致
    let src_final = dir.join("src");
    let _ = std::fs::remove_dir_all(&src_final);
    card_package::unpack_archive(&pkg_final, &src_final)?;
    let extracted = store::read_manifest(&src_final)?;
    if extracted.id != meta.id || extracted.model != meta.model {
        return Err(ModelinferError::new(
            codes::MANIFEST_INVALID,
            "解压后的 manifest 与解析结果不一致",
        ));
    }

    let size_bytes = std::fs::metadata(&pkg_final).map(|m| m.len() as i64).unwrap_or(0);
    Ok(PlacedLocalCard {
        id: meta.id.clone(),
        version: meta.version.clone(),
        model: meta.model.clone(),
        name: display_name_of(&meta),
        manifest_json: serde_json::to_string(&meta.raw).unwrap_or_else(|_| "{}".to_string()),
        package_path: pkg_final.display().to_string(),
        src_path: src_final.display().to_string(),
        package_sha256,
        size_bytes,
    })
}

/// 落库（upsert：`source = "local"`，同主键覆盖并保留 `created_at`）
async fn finish_local_card(
    db: &DatabaseConnection,
    placed: PlacedLocalCard,
) -> Result<PlazaModelView, ModelinferError> {
    let pk = card_pk(&placed.id, &placed.version);
    let now = time::now_iso();
    let existing = plaza_model::Entity::find_by_id(pk.clone())
        .one(db)
        .await
        .map_err(db_err)?;
    let row = plaza_model::ActiveModel {
        id: Set(pk),
        model_id: Set(placed.id.clone()),
        model: Set(placed.model.clone()),
        name: Set(placed.name),
        version: Set(placed.version.clone()),
        source: Set("local".to_string()),
        source_id: Set(String::new()),
        path: Set(placed.src_path),
        package_path: Set(placed.package_path),
        package_sha256: Set(placed.package_sha256),
        manifest_json: Set(placed.manifest_json),
        state: Set("available".to_string()),
        size_bytes: Set(placed.size_bytes),
        created_at: Set(existing
            .as_ref()
            .map(|m| m.created_at.clone())
            .unwrap_or_else(|| now.clone())),
        updated_at: Set(now),
    };
    let saved = if existing.is_some() {
        row.update(db).await.map_err(db_err)?
    } else {
        row.insert(db).await.map_err(db_err)?
    };
    Ok(PlazaModelView::from(saved))
}

fn display_name_of(meta: &ManifestMeta) -> String {
    match &meta.name {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Object(map)) => map
            .get("zh-CN")
            .and_then(|v| v.as_str())
            .or_else(|| map.values().find_map(|v| v.as_str()))
            .unwrap_or(&meta.id)
            .to_string(),
        _ => meta.id.clone(),
    }
}

pub async fn list_models(db: &DatabaseConnection) -> Result<Vec<PlazaModelView>, ModelinferError> {
    let rows = plaza_model::Entity::find()
        .order_by_asc(plaza_model::Column::ModelId)
        .all(db)
        .await
        .map_err(db_err)?;
    Ok(rows.into_iter().map(PlazaModelView::from).collect())
}

pub async fn get_model(
    db: &DatabaseConnection,
    id: &str,
) -> Result<plaza_model::Model, ModelinferError> {
    plaza_model::Entity::find_by_id(id.to_string())
        .one(db)
        .await
        .map_err(db_err)?
        .ok_or_else(|| ModelinferError::new(codes::NOT_FOUND, format!("已登记模型不存在: {id}")))
}

pub async fn delete_model(
    db: &DatabaseConnection,
    plaza_dir: &Path,
    id: &str,
) -> Result<(), ModelinferError> {
    let model = get_model(db, id).await?;
    plaza_model::Entity::delete_by_id(id.to_string())
        .exec(db)
        .await
        .map_err(db_err)?;
    // 删除本地目录（best-effort）
    let dir = store::card_dir(plaza_dir, &model.model_id, &model.version);
    let _ = std::fs::remove_dir_all(&dir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 以一次性缓存调用 `sync_cards`（测试辅助，避免每个用例都显式建缓存）
    async fn sync_once(
        plaza: &dyn PlazaClient,
        db: &DatabaseConnection,
        source_id: &str,
    ) -> Vec<CardView> {
        sync_cards(plaza, db, source_id, false, &new_card_cache()).await.unwrap()
    }

    use crate::db;

    const INDEX: &str = r#"
kind: ModelIndex
models:
  - id: com.ictrek.modelinfer.test-mock
    version: 1.0.0
    name: { zh-CN: 模拟模型, en: Mock Model }
    manifest: meta/com.ictrek.modelinfer.test-mock/1.0.0/manifest.yaml
    package: models/com.ictrek.modelinfer.test-mock/1.0.0/model.tar.gz
    sha256: __SHA__
"#;

    fn manifest_text() -> &'static str {
        "id: com.ictrek.modelinfer.test-mock\nversion: 1.0.0\nmodel: Qwen/Qwen3-0.6B\nname: { zh-CN: 模拟 }\n"
    }

    fn make_tar_gz() -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        {
            let mut builder = tar::Builder::new(&mut gz);
            let content = manifest_text();
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "manifest.yaml", content.as_bytes())
                .unwrap();
            builder.finish().unwrap();
        }
        gz.finish().unwrap()
    }

    fn mock_with(bytes: &[u8]) -> MockPlazaClient {
        let sha = store::sha256_hex(bytes);
        let index = INDEX.replace("__SHA__", &sha);
        MockPlazaClient::new()
            .with_index(index)
            .with_manifest(manifest_text())
            .with_package(bytes.to_vec())
    }

    #[tokio::test]
    async fn source_crud_and_validation() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let s = create_source(
            &db,
            SourceCreate { name: "s1".into(), base_url: "http://h:1/store/".into(), enabled: true },
        )
        .await
        .unwrap();
        assert_eq!(s.base_url, "http://h:1/store");
        assert_eq!(list_sources(&db).await.unwrap().len(), 1);

        let err = create_source(
            &db,
            SourceCreate { name: "s2".into(), base_url: "foo".into(), enabled: true },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID);

        delete_source(&db, &s.id).await.unwrap();
        assert_eq!(delete_source(&db, &s.id).await.unwrap_err().code, codes::NOT_FOUND);
    }

    #[tokio::test]
    async fn sync_and_register_roundtrip() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let bytes = make_tar_gz();
        let plaza = mock_with(&bytes);
        let src = create_source(
            &db,
            SourceCreate { name: "s".into(), base_url: "http://h:1".into(), enabled: true },
        )
        .await
        .unwrap();

        let cards = sync_once(&plaza, &db, &src.id).await;
        assert_eq!(cards.len(), 1);
        assert!(!cards[0].registered);
        assert_eq!(cards[0].package, "http://h:1/models/com.ictrek.modelinfer.test-mock/1.0.0/model.tar.gz");
        // 索引未提供 icon 时保持为空，不回落基地址
        assert_eq!(cards[0].icon, "");
        assert_eq!(
            cards[0].name.as_ref().and_then(|v| v.get("zh-CN")).and_then(|v| v.as_str()),
            Some("模拟模型")
        );

        let m = register_model(
            &plaza,
            &db,
            tmp.path(),
            RegisterReq {
                source_id: src.id.clone(),
                model_id: "com.ictrek.modelinfer.test-mock".into(),
                version: "1.0.0".into(),
            },
        )
        .await
        .unwrap();
        assert_eq!(m.model, "Qwen/Qwen3-0.6B");
        assert!(m.manifest_json.contains("Qwen/Qwen3-0.6B"));

        let cards = sync_once(&plaza, &db, &src.id).await;
        assert!(cards[0].registered);

        delete_model(&db, tmp.path(), &m.id).await.unwrap();
        assert!(list_models(&db).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn register_checksum_mismatch() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        let bytes = make_tar_gz();
        let plaza = MockPlazaClient::new()
            .with_index(INDEX.replace("__SHA__", "deadbeef"))
            .with_manifest(manifest_text())
            .with_package(bytes);
        let src = create_source(
            &db,
            SourceCreate { name: "s".into(), base_url: "http://h:1".into(), enabled: true },
        )
        .await
        .unwrap();
        let err = register_model(
            &plaza,
            &db,
            tmp.path(),
            RegisterReq {
                source_id: src.id,
                model_id: "com.ictrek.modelinfer.test-mock".into(),
                version: "1.0.0".into(),
            },
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::CHECKSUM_MISMATCH);
    }

    /// 构造外层 tar（含 `model.tar.gz` + assets/），`model.tar.gz` 由 `inner` 提供
    fn make_outer_tar(inner: &[u8], compression: &str) -> Vec<u8> {
        let raw = {
            let mut buf = Vec::new();
            {
                let mut builder = tar::Builder::new(&mut buf);
                let mut header = tar::Header::new_gnu();
                header.set_size(inner.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder
                    .append_data(&mut header, "model.tar.gz", inner)
                    .unwrap();
                let assets = b"assets";
                let mut h2 = tar::Header::new_gnu();
                h2.set_size(assets.len() as u64);
                h2.set_mode(0o644);
                h2.set_cksum();
                builder
                    .append_data(&mut h2, "assets/amd64/img.tar.gz", &assets[..])
                    .unwrap();
                builder.finish().unwrap();
            }
            buf
        };
        match compression {
            "gz" => {
                use std::io::Write as _;
                let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
                gz.write_all(&raw).unwrap();
                gz.finish().unwrap()
            }
            "xz" => {
                use std::io::Write as _;
                let mut xz = xz2::write::XzEncoder::new(Vec::new(), 6);
                xz.write_all(&raw).unwrap();
                xz.finish().unwrap()
            }
            _ => raw,
        }
    }

    #[tokio::test]
    async fn register_local_card_package_roundtrip_and_overwrite() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::TempDir::new().unwrap();

        let inner = make_tar_gz();
        let outer = make_outer_tar(&inner, "gz");
        let staged = tmp.path().join("model_test.tar.gz");
        std::fs::write(&staged, &outer).unwrap();

        let m = register_local_card_package(&db, tmp.path(), &staged).await.unwrap();
        assert_eq!(m.source, "local");
        assert_eq!(m.source_id, "");
        assert_eq!(m.state, "available");
        assert_eq!(m.model, "Qwen/Qwen3-0.6B");
        assert!(m.package_path.ends_with("model.tar.gz"));
        assert!(std::path::Path::new(&m.package_path).exists());
        assert!(std::path::Path::new(&m.path).join("manifest.yaml").exists());
        assert_eq!(list_models(&db).await.unwrap().len(), 1);

        let created_at = m.created_at.clone();
        // 再次上传（同 id@version）→ 幂等覆盖，保留 created_at
        let outer2 = make_outer_tar(&inner, "xz");
        let staged2 = tmp.path().join("model_test.tar.xz");
        std::fs::write(&staged2, &outer2).unwrap();
        let m2 = register_local_card_package(&db, tmp.path(), &staged2).await.unwrap();
        assert_eq!(m2.id, m.id);
        assert_eq!(m2.created_at, created_at);
        assert_eq!(list_models(&db).await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn register_local_card_package_rejects_missing_package() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let tmp = tempfile::TempDir::new().unwrap();
        // 外层无 model.tar.gz / manifest.yaml
        let outer = make_outer_no_card();
        let staged = tmp.path().join("empty.tar");
        std::fs::write(&staged, &outer).unwrap();
        let err = register_local_card_package(&db, tmp.path(), &staged).await.unwrap_err();
        assert_eq!(err.code, codes::CARD_PACKAGE_INVALID);
    }

    fn make_outer_no_card() -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            let content = b"hello";
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder
                .append_data(&mut header, "readme.txt", &content[..])
                .unwrap();
            builder.finish().unwrap();
        }
        buf
    }

    // ---------- 版本聚合（纯函数） ----------

    fn card_view(id: &str, version: &str, zh: &str, registered: bool, size: i64) -> CardView {
        CardView {
            id: id.into(),
            name: Some(serde_json::json!({ "zh-CN": zh })),
            version: version.into(),
            description: None,
            category: vec!["test".into()],
            tags: vec!["mock".into()],
            icon: String::new(),
            model: "org/weight".into(),
            images: vec![serde_json::json!("registry.example/img:1")],
            manifest: format!("http://h/meta/{id}/{version}/manifest.yaml"),
            package: format!("http://h/models/{id}/{version}/model.tar.gz"),
            size_bytes: Some(size),
            sha256: None,
            registered,
            plaza_model_id: if registered { Some(card_pk(id, version)) } else { None },
        }
    }

    fn model_view(card: &str, version: &str, source: &str, size: i64) -> PlazaModelView {
        PlazaModelView {
            id: card_pk(card, version),
            model_id: card.into(),
            model: "Qwen/Qwen3-0.6B".into(),
            name: "mock".into(),
            version: version.into(),
            source: source.into(),
            source_id: if source == "local" { String::new() } else { "s1".into() },
            path: "/tmp/x/src".into(),
            package_path: "/tmp/x/model.tar.gz".into(),
            package_sha256: String::new(),
            manifest_json: "{}".into(),
            state: "available".into(),
            size_bytes: size,
            created_at: "2026-09-16T00:00:00+08:00".into(),
            updated_at: "2026-09-16T00:00:00+08:00".into(),
        }
    }

    #[test]
    fn compare_versions_desc_pure_and_total() {
        use std::cmp::Ordering;
        // 数值段按数值比较（1.0.10 > 1.0.9）
        assert_eq!(compare_versions_desc("1.0.10", "1.0.9"), Ordering::Less);
        assert_eq!(compare_versions_desc("1.0.9", "1.0.10"), Ordering::Greater);
        assert_eq!(compare_versions_desc("1.1.0", "1.0.10"), Ordering::Less);
        // 段数多者更新
        assert_eq!(compare_versions_desc("1.0.0", "1.0"), Ordering::Less);
        assert_eq!(compare_versions_desc("1.0", "1.0.0"), Ordering::Greater);
        assert_eq!(compare_versions_desc("1.0.0", "1.0.0"), Ordering::Equal);
        // 非法 / 空串兜底：不 panic
        let _ = compare_versions_desc("abc", "1.0.0");
        let _ = compare_versions_desc("", "");

        // 整体降序排列
        let mut v = vec!["1.0.0", "1.0.10", "1.1.0", "1.0.1", "0.9.9"];
        v.sort_by(|a, b| compare_versions_desc(a, b));
        assert_eq!(v, vec!["1.1.0", "1.0.10", "1.0.1", "1.0.0", "0.9.9"]);
    }

    #[test]
    fn group_cards_aggregates_versions_and_uses_latest_metadata() {
        let groups = group_cards(vec![
            card_view("com.ictrek.a", "1.0.0", "甲", true, 100),
            card_view("com.ictrek.a", "1.1.0", "乙", false, 200),
            card_view("com.ictrek.b", "0.1.0", "丙", false, 50),
        ]);
        assert_eq!(groups.len(), 2);
        let a = groups.iter().find(|g| g.id == "com.ictrek.a").unwrap();
        assert_eq!(a.latest_version, "1.1.0");
        assert_eq!(a.versions.len(), 2);
        assert_eq!(a.versions[0].version, "1.1.0");
        assert_eq!(a.versions[1].version, "1.0.0");
        assert_eq!(a.registered_count, 1);
        assert!(a.versions[1].registered);
        assert!(!a.versions[0].registered);
        assert_eq!(a.versions[1].plaza_model_id.as_deref(), Some("com.ictrek.a@1.0.0"));
        // 分组元数据取最新版本
        assert_eq!(
            a.name.as_ref().and_then(|v| v.get("zh-CN")).and_then(|v| v.as_str()),
            Some("乙")
        );
        assert_eq!(a.model, "org/weight");
        assert_eq!(a.images.len(), 1);
        // 不同卡片不合并
        assert!(groups.iter().any(|g| g.id == "com.ictrek.b"));
    }

    #[test]
    fn group_models_aggregates_sources_sizes_and_latest() {
        let groups = group_models(vec![
            model_view("com.ictrek.a", "1.0.0", "plaza", 100),
            model_view("com.ictrek.a", "1.1.0", "local", 200),
            model_view("com.ictrek.b", "2.0.0", "plaza", 7),
        ]);
        assert_eq!(groups.len(), 2);
        let a = groups.iter().find(|g| g.model_id == "com.ictrek.a").unwrap();
        assert_eq!(a.latest_version, "1.1.0");
        assert_eq!(a.total_size_bytes, 300);
        assert_eq!(a.sources, vec!["local".to_string(), "plaza".to_string()]);
        assert!(a.versions[0].latest);
        assert!(!a.versions[1].latest);
        assert_eq!(a.versions[1].model.id, "com.ictrek.a@1.0.0");
        assert_eq!(a.versions[1].model.package_path, "/tmp/x/model.tar.gz");
        assert_eq!(a.model, "Qwen/Qwen3-0.6B");
    }

    #[tokio::test]
    async fn sync_and_group_multi_version_card() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let src = create_source(
            &db,
            SourceCreate { name: "s".into(), base_url: "http://h:1".into(), enabled: true },
        )
        .await
        .unwrap();
        let index = r#"
kind: ModelIndex
models:
  - { id: com.ictrek.modelinfer.test-mock, version: 1.0.0, name: { zh-CN: 甲 }, manifest: meta/a/1.0.0/manifest.yaml, package: models/a/1.0.0/model.tar.gz }
  - { id: com.ictrek.modelinfer.test-mock, version: 1.1.0, name: { zh-CN: 乙 }, manifest: meta/a/1.1.0/manifest.yaml, package: models/a/1.1.0/model.tar.gz }
"#;
        let plaza = MockPlazaClient::new()
            .with_index(index)
            .with_manifest(manifest_text())
            .with_package(make_tar_gz());
        let groups = group_cards(sync_once(&plaza, &db, &src.id).await);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].id, "com.ictrek.modelinfer.test-mock");
        assert_eq!(groups[0].latest_version, "1.1.0");
        assert_eq!(groups[0].versions.len(), 2);
        assert_eq!(groups[0].registered_count, 0);
        assert_eq!(
            groups[0].name.as_ref().and_then(|v| v.get("zh-CN")).and_then(|v| v.as_str()),
            Some("乙")
        );
    }

    // ---------- 列表筛选 / 排序 / 统计（分页配套） ----------

    fn fx_card(id: &str, name: &str, tags: &[&str], registered: usize) -> PlazaCardGroupView {
        PlazaCardGroupView {
            id: id.into(),
            name: Some(serde_json::json!({ "zh-CN": name })),
            description: None,
            category: Vec::new(),
            tags: tags.iter().map(|t| t.to_string()).collect(),
            icon: String::new(),
            model: String::new(),
            images: Vec::new(),
            latest_version: "1.0.0".into(),
            registered_count: registered,
            family: crate::model_family::infer_from_json_name(
                &tags.iter().map(|t| t.to_string()).collect::<Vec<String>>(),
                None,
                Some(id),
                Some(&serde_json::json!(name)),
            )
            .to_string(),
            versions: Vec::new(),
        }
    }

    fn fx_model(
        id: &str,
        weight: &str,
        name: &str,
        tags: &[&str],
        size_bytes: i64,
        version_count: usize,
        latest_created: &str,
    ) -> LocalModelGroupView {
        let tags_v: Vec<String> = tags.iter().map(|t| t.to_string()).collect();
        let family = crate::model_family::infer(&tags_v, Some(weight), Some(id), Some(name)).to_string();
        let versions = (0..version_count)
            .map(|i| LocalModelVersionView {
                model: PlazaModelView {
                    id: format!("{id}@1.0.{i}"),
                    model_id: id.into(),
                    model: weight.into(),
                    name: name.into(),
                    version: format!("1.0.{i}"),
                    source: if i == 0 { "local".into() } else { "plaza".into() },
                    source_id: String::new(),
                    path: String::new(),
                    package_path: String::new(),
                    package_sha256: String::new(),
                    manifest_json: serde_json::json!({ "tags": tags_v }).to_string(),
                    state: "available".into(),
                    size_bytes,
                    created_at: latest_created.into(),
                    updated_at: latest_created.into(),
                },
                latest: i == 0,
            })
            .collect();
        LocalModelGroupView {
            model_id: id.into(),
            name: name.into(),
            model: weight.into(),
            sources: vec!["local".into(), "plaza".into()],
            images: Vec::new(),
            latest_version: "1.0.0".into(),
            total_size_bytes: size_bytes * version_count as i64,
            family,
            versions,
        }
    }

    #[test]
    fn group_cards_and_models_carry_family() {
        let cards = group_cards(vec![
            CardView {
                id: "com.ictrek.qwen3.8-27b-gb10".into(),
                name: None,
                version: "1.0.0".into(),
                description: None,
                category: Vec::new(),
                tags: vec!["qwen".into()],
                icon: String::new(),
                model: String::new(),
                images: Vec::new(),
                manifest: String::new(),
                package: String::new(),
                size_bytes: None,
                sha256: None,
                registered: false,
                plaza_model_id: None,
            },
        ]);
        assert_eq!(cards[0].family, "qwen");

        let models = group_models(vec![PlazaModelView {
            id: "com.ictrek.glm-5.3-flash-gb10@1.0.0".into(),
            model_id: "com.ictrek.glm-5.3-flash-gb10".into(),
            model: "ZhipuAI/GLM-5.3-Flash".into(),
            name: "GLM".into(),
            version: "1.0.0".into(),
            source: "local".into(),
            source_id: String::new(),
            path: String::new(),
            package_path: String::new(),
            package_sha256: String::new(),
            manifest_json: "{}".into(),
            state: "available".into(),
            size_bytes: 1,
            created_at: "2026-01-01T00:00:00+08:00".into(),
            updated_at: "2026-01-01T00:00:00+08:00".into(),
        }]);
        assert_eq!(models[0].family, "glm");
    }

    #[test]
    fn filter_cards_by_keyword_and_registered() {
        let cards = vec![
            fx_card("com.ictrek.deepseek-v4.1-flash-gb10", "DeepSeek 多版本", &["deepseek"], 1),
            fx_card("com.ictrek.qwen3.8-27b-gb10", "Qwen 单节点", &["qwen"], 0),
        ];
        let all = filter_cards(cards.clone(), &CardFilter::default());
        assert_eq!(all.len(), 2);

        // 关键词匹配名称
        let hit = filter_cards(cards.clone(), &CardFilter { keyword: "qwen".into(), registered_only: false });
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].id, "com.ictrek.qwen3.8-27b-gb10");

        // 关键词匹配 tags
        let hit = filter_cards(cards.clone(), &CardFilter { keyword: "deepseek".into(), registered_only: false });
        assert_eq!(hit.len(), 1);

        // 仅看已下载
        let hit = filter_cards(cards, &CardFilter { keyword: String::new(), registered_only: true });
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].registered_count, 1);
    }

    #[test]
    fn filter_models_by_family_source_keyword_and_sort() {
        let models = vec![
            fx_model("com.ictrek.qwen3.8-27b-gb10", "Qwen/Qwen3.8-27B", "Qwen3.8", &["qwen"], 100, 3, "2026-09-24T22:00:00+08:00"),
            fx_model("com.ictrek.deepseek-v4.1-flash-gb10", "deepseek-ai/DeepSeek-V4.1-Flash", "DeepSeek", &["deepseek"], 50, 1, "2026-09-22T21:56:00+08:00"),
        ];
        let all = filter_models(models.clone(), &ModelFilter::default());
        assert_eq!(all.len(), 2);

        let by_family = filter_models(models.clone(), &ModelFilter { family: "qwen".into(), ..Default::default() });
        assert_eq!(by_family.len(), 1);
        assert_eq!(by_family[0].family, "qwen");

        let by_source = filter_models(models.clone(), &ModelFilter { source: "plaza".into(), ..Default::default() });
        assert_eq!(by_source.len(), 2);

        let by_kw = filter_models(models.clone(), &ModelFilter { keyword: "deepseek-ai/".into(), ..Default::default() });
        assert_eq!(by_kw.len(), 1);

        let by_size = filter_models(models.clone(), &ModelFilter { sort: ModelSort::Size, ..Default::default() });
        assert_eq!(by_size[0].family, "qwen");

        let by_versions = filter_models(models.clone(), &ModelFilter { sort: ModelSort::Versions, ..Default::default() });
        assert_eq!(by_versions[0].versions.len(), 3);

        let by_latest = filter_models(models.clone(), &ModelFilter { sort: ModelSort::Latest, ..Default::default() });
        assert_eq!(by_latest[0].family, "qwen");

        // 名称排序（Unicode 码点序）：DeepSeek < Qwen3.8
        let by_name = filter_models(models, &ModelFilter { sort: ModelSort::Name, ..Default::default() });
        assert_eq!(by_name[0].family, "deepseek");
    }

    #[test]
    fn stats_are_computed_over_full_set() {
        let cards = vec![
            fx_card("a", "A", &["qwen"], 2),
            fx_card("b", "B", &["glm"], 1),
        ];
        let s = plaza_card_stats(&cards);
        assert_eq!(s.cards, 2);
        assert_eq!(s.downloaded, 3);

        let models = vec![
            fx_model("m1", "Qwen/Qwen3", "Q", &["qwen"], 10, 2, "2026-01-02T00:00:00+08:00"),
            fx_model("m2", "ZhipuAI/GLM", "G", &["glm"], 20, 1, "2026-01-01T00:00:00+08:00"),
            fx_model("m3", "Qwen/Qwen2", "Q2", &["qwen"], 5, 1, "2026-01-03T00:00:00+08:00"),
        ];
        let s = local_model_stats(&models);
        assert_eq!(s.cards, 3);
        assert_eq!(s.versions, 4);
        assert_eq!(s.size_bytes, 10 * 2 + 20 + 5);
        assert_eq!(s.families, vec!["glm".to_string(), "qwen".to_string()]);
    }

    #[test]
    fn model_sort_parse_falls_back_to_name() {
        assert_eq!(ModelSort::parse("size"), ModelSort::Size);
        assert_eq!(ModelSort::parse("versions"), ModelSort::Versions);
        assert_eq!(ModelSort::parse("latest"), ModelSort::Latest);
        assert_eq!(ModelSort::parse("nope"), ModelSort::Name);
    }
}
