//! registry 域：Docker Registry HTTP API v2 客户端与管理逻辑。
//!
//! - 数据面：官方 registry（Docker Distribution）HTTP v2 API，无认证
//! - 抽象 `RegistryClient`：生产 `RealRegistryClient`（reqwest），单测 `MockRegistryClient`
//! - 删除语义：registry 仅支持按 digest 删除 manifest；删除 tag = HEAD 取 digest 再 DELETE；
//!   删除整仓 = 枚举全部 tag 逐个删（删除后 blob 由 registry GC 回收，本组件不触发 GC）
//! - 摘标签语义（`untag_manifest`，D-IH-51）：`DELETE /v2/<name>/manifests/<tag>` **仅摘除该 tag**
//!   （实测保留同 digest 的其它 tag 与 manifest revision），改名单 tag 依赖该语义；
//!   `DELETE .../manifests/<digest>` 会移除**所有**指向该 digest 的 tag，二者不可混用

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ImageHubError, codes};

/// registry manifest 相关请求的 Accept 头（覆盖 docker v2 / OCI / manifest list）
const MANIFEST_ACCEPT: &str = concat!(
    "application/vnd.docker.distribution.manifest.v2+json, ",
    "application/vnd.docker.distribution.manifest.list.v2+json, ",
    "application/vnd.oci.image.manifest.v1+json, ",
    "application/vnd.oci.image.index.v1+json, ",
    "application/vnd.docker.distribution.manifest.v1+json"
);

/// Docker manifest list 的 media type
pub const DOCKER_INDEX_MEDIA_TYPE: &str =
    "application/vnd.docker.distribution.manifest.list.v2+json";
/// OCI image index 的 media type
pub const OCI_INDEX_MEDIA_TYPE: &str = "application/vnd.oci.image.index.v1+json";

/// manifest list / image index 的平台条目（多架构镜像）
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlatformEntry {
    pub os: Option<String>,
    pub architecture: Option<String>,
    pub variant: Option<String>,
    pub digest: Option<String>,
    pub size_bytes: Option<u64>,
}

impl PlatformEntry {
    /// `os/architecture[/variant]`（缺失时返回 `unknown`）
    pub fn platform_string(&self) -> String {
        let os = self.os.as_deref().unwrap_or("unknown");
        let arch = self.architecture.as_deref().unwrap_or("unknown");
        match self.variant.as_deref() {
            Some(v) if !v.is_empty() => format!("{os}/{arch}/{v}"),
            _ => format!("{os}/{arch}"),
        }
    }
}

/// 单个标签的摘要信息
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageTag {
    pub tag: String,
    pub digest: Option<String>,
    /// 镜像层大小合计（字节；尽力而为，解析失败为 None）
    pub size_bytes: Option<u64>,
    /// 镜像创建时间（RFC3339；来自 config blob，尽力而为）
    pub created: Option<String>,
    pub media_type: Option<String>,
    /// 多架构平台条目（单平台 manifest 为空）
    #[serde(default)]
    pub platforms: Vec<PlatformEntry>,
}

/// 一个仓库（repository）及其标签
///
/// 说明：无标签仓库不会出现在列表中（见 `collect_images`），故无需「是否可删」标记——
/// 列表中的每个仓库都可删除。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryImages {
    /// registry 中的仓库名（保留上游完整名称，如 `docker.io/foo/bar`）
    pub repository: String,
    pub tags: Vec<ImageTag>,
}

/// 删除结果
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteResult {
    pub repository: String,
    pub deleted_tags: Vec<String>,
}

/// 仓库详情（单仓库的标签列表）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryDetail {
    pub repository: String,
    pub tags: Vec<ImageTag>,
}

/// manifest 解析结果（纯数据，便于单测）
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManifestInfo {
    pub digest: Option<String>,
    pub size_bytes: Option<u64>,
    pub created: Option<String>,
    pub media_type: Option<String>,
    pub platforms: Vec<PlatformEntry>,
    /// **单平台** image manifest 的平台信息（来自 config blob 的 `os`/`architecture`/`variant`，
    /// 尽力而为；由 `describe_tag` 合成为单条平台条目，D-IH-48）
    pub platform: Option<PlatformEntry>,
    /// 该 manifest 为 manifest list / image index 时引用的**全部**子清单 digest
    /// （取自 body 的原始 `manifests[].digest`，**不过滤** attestation——删除需一并清理）
    pub child_digests: Vec<String>,
}

/// registry 数据面抽象
#[async_trait]
pub trait RegistryClient: Send + Sync {
    /// `GET /v2/_catalog` → 仓库名列表
    async fn list_repositories(&self) -> Result<Vec<String>, ImageHubError>;
    /// `GET /v2/<name>/tags/list` → 标签列表（404 / null → 空）
    async fn list_tags(&self, repository: &str) -> Result<Vec<String>, ImageHubError>;
    /// `HEAD/GET /v2/<name>/manifests/<ref>` → digest（+ 尽力而为的元信息）
    async fn describe_manifest(
        &self,
        repository: &str,
        reference: &str,
    ) -> Result<ManifestInfo, ImageHubError>;
    /// `DELETE /v2/<name>/manifests/<digest>`
    async fn delete_manifest(&self, repository: &str, digest: &str) -> Result<(), ImageHubError>;
    /// `DELETE /v2/<name>/manifests/<tag>` —— **仅摘除标签**（不改动其它指向同一 digest 的 tag）
    ///
    /// 用于改名（D-IH-51）：按 digest 删除会移除同仓库内**所有**指向该 digest 的 tag，
    /// 故改名单 tag 时必须按 tag 引用删除。
    async fn untag_manifest(&self, repository: &str, tag: &str) -> Result<(), ImageHubError>;
    /// `GET /v2/` 探活（health 用）
    async fn ping(&self) -> bool;
}

// ==================== 纯函数解析 ====================

/// 解析 `_catalog` 响应体 → 仓库名列表
pub fn parse_repositories(body: &Value) -> Vec<String> {
    body.get("repositories")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 `tags/list` 响应体 → 标签列表（tags 可为 null）
pub fn parse_tags(body: &Value) -> Vec<String> {
    body.get("tags")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// 解析 manifest 响应体 → ManifestInfo（digest 来自响应头，body 提供 size/mediaType）
///
/// - image manifest：size = Σ layers[].size（+ config.size），created 需再拉 config blob（此处不拉）
/// - index（manifest list）：size = Σ manifests[].size，created 无
pub fn parse_manifest(body: &Value, digest: Option<String>, content_type: Option<String>) -> ManifestInfo {
    let media_type = body
        .get("mediaType")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or(content_type);

    // image manifest：layers
    let layers_size: Option<u64> = body
        .get("layers")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|l| l.get("size").and_then(Value::as_u64)).sum());

    let config_size = body
        .get("config")
        .and_then(|c| c.get("size"))
        .and_then(Value::as_u64);

    // 说明：manifest list / index 的 `manifests[].size` 是子 manifest 大小（约 KB 级），
    // 并非镜像大小，故索引的 size_bytes 置为 None（由详情接口按平台条目补齐真实大小）。
    let size_bytes = layers_size.map(|l| l + config_size.unwrap_or(0));

    ManifestInfo {
        digest,
        size_bytes,
        created: None,
        media_type,
        platforms: parse_platforms(body),
        platform: None,
        child_digests: parse_child_digests(body),
    }
}

/// 平台条目是否为 manifest list / image index 声明？（纯函数；决定是否逐平台补齐大小）
pub fn is_index_media_type(media_type: Option<&str>) -> bool {
    matches!(
        media_type,
        Some(mt) if mt == DOCKER_INDEX_MEDIA_TYPE || mt == OCI_INDEX_MEDIA_TYPE
    )
}

/// 非空字符串字段（空白串视为缺失）
fn non_empty_str(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 从镜像 **config blob** 解析平台信息（纯函数）：
/// 取顶层 `os` / `architecture` / `variant`；`os` 与 `architecture` 均缺失 → `None`
/// （`digest` / `size_bytes` 由调用方按该 manifest 自身补全）
pub fn parse_config_platform(config: &Value) -> Option<PlatformEntry> {
    let os = non_empty_str(config.get("os"));
    let architecture = non_empty_str(config.get("architecture"));
    if os.is_none() && architecture.is_none() {
        return None;
    }
    Some(PlatformEntry {
        os,
        architecture,
        variant: non_empty_str(config.get("variant")),
        digest: None,
        size_bytes: None,
    })
}

/// 解析 manifest list / image index 的 `manifests[]` → **全部**子清单 digest（保持出现顺序，去重）
///
/// 与 `parse_platforms` 的区别：此处**不**过滤 `unknown/unknown` 的 attestation 条目——
/// 它们同样是经 digest 推送的未打标签清单，删除时须一并清理（见 D-IH-26）。
pub fn parse_child_digests(body: &Value) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if let Some(a) = body.get("manifests").and_then(Value::as_array) {
        for m in a {
            if let Some(d) = m.get("digest").and_then(Value::as_str)
                && !out.iter().any(|x| x == d)
            {
                out.push(d.to_string());
            }
        }
    }
    out
}

/// 解析 manifest list / image index 的 `manifests[]` → 平台条目列表
///
/// - 单平台 image manifest 无 `manifests` 字段 → 空列表
/// - 仅保留含 `platform` 的条目（排除 attestation（unknown/unknown）等）
pub fn parse_platforms(body: &Value) -> Vec<PlatformEntry> {
    body.get("manifests")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|m| {
                    let platform = m.get("platform")?;
                    // 过滤 attestation / 未知平台条目（`unknown/unknown`）
                    let os = platform.get("os").and_then(Value::as_str).unwrap_or("");
                    let arch = platform
                        .get("architecture")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    if os == "unknown" && arch == "unknown" {
                        return None;
                    }
                    Some(PlatformEntry {
                        os: platform
                            .get("os")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        architecture: platform
                            .get("architecture")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        variant: platform
                            .get("variant")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                        digest: m.get("digest").and_then(Value::as_str).map(str::to_string),
                        size_bytes: m.get("size").and_then(Value::as_u64),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// 提取 config blob digest（image manifest 的 `config.digest`）
pub fn config_digest(body: &Value) -> Option<String> {
    body.get("config")
        .and_then(|c| c.get("digest"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// 从 config blob（镜像配置 JSON）提取 `created`
pub fn config_created(body: &Value) -> Option<String> {
    body.get("created")
        .and_then(Value::as_str)
        .map(str::to_string)
}

// ==================== 校验 ====================

/// 校验仓库名：仅允许小写字母数字与 `._-/`，禁止 `..` 与空段
pub fn validate_repository(repo: &str) -> Result<(), ImageHubError> {
    let trimmed = repo.trim();
    if trimmed.is_empty()
        || trimmed.contains("..")
        || !trimmed.chars().all(|c| {
            c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-' | '/')
        })
    {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            format!("非法仓库名: {repo}"),
        ));
    }
    Ok(())
}

/// 校验标签：仅允许字母数字与 `._-`，长度 1..=128
pub fn validate_tag(tag: &str) -> Result<(), ImageHubError> {
    let t = tag.trim();
    if t.is_empty()
        || t.len() > 128
        || !t
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        return Err(ImageHubError::new(
            codes::PARAM_INVALID,
            format!("非法标签: {tag}"),
        ));
    }
    Ok(())
}

// ==================== 管理逻辑 ====================

/// 枚举全部镜像（仓库 → 标签 → 摘要），单仓库失败不阻断整体
///
/// **只返回至少有一个 tag 的仓库**：registry 删除 manifest 后会保留仓库目录、`_catalog`
/// 仍会列出该仓库且 `tags/list` 返回 null；这类无标签仓库已无法经 v2 API 做有效运维
/// （不能枚举其残留 manifest digest），故不在列表中展示（见 D-IH-29）。
pub async fn collect_images(
    client: &dyn RegistryClient,
) -> Result<Vec<RepositoryImages>, ImageHubError> {
    let repos = client.list_repositories().await?;
    let mut out = Vec::with_capacity(repos.len());
    for repo in repos {
        let tags = match client.list_tags(&repo).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(repository = %repo, err = %e.msg, "列标签失败，跳过");
                Vec::new()
            }
        };
        if tags.is_empty() {
            tracing::warn!(
                repository = %repo,
                "仓库无标签，不在列表中展示（registry 中可能仍有残留 manifest/目录，需 GC 或后续版本处理）"
            );
            continue;
        }
        let mut image_tags = Vec::with_capacity(tags.len());
        for tag in tags {
            image_tags.push(describe_tag(client, &repo, tag).await);
        }
        out.push(RepositoryImages {
            repository: repo,
            tags: image_tags,
        });
    }
    Ok(out)
}

/// 描述单个标签（解析失败降级为空元信息，不阻断）
async fn describe_tag(client: &dyn RegistryClient, repository: &str, tag: String) -> ImageTag {
    match client.describe_manifest(repository, &tag).await {
        Ok(info) => {
            // 单平台 image manifest：以 config blob 的平台信息合成**单条**平台条目，
            // 使前端与消费方统一读 `platforms`（D-IH-48）；digest / size 取自该 tag 自身
            let mut platforms = info.platforms;
            if platforms.is_empty()
                && let Some(mut platform) = info.platform
            {
                platform.digest = info.digest.clone();
                platform.size_bytes = info.size_bytes;
                platforms = vec![platform];
            }
            ImageTag {
                tag,
                digest: info.digest,
                size_bytes: info.size_bytes,
                created: info.created,
                media_type: info.media_type,
                platforms,
            }
        }
        Err(e) => {
            tracing::warn!(repository = %repository, tag = %tag, err = %e.msg, "解析 manifest 失败");
            ImageTag {
                tag,
                digest: None,
                size_bytes: None,
                created: None,
                media_type: None,
                platforms: Vec::new(),
            }
        }
    }
}

/// 枚举单个仓库的标签详情（含多架构平台条目）
///
/// 仓库不存在 / 无标签 → 20803；仓库名非法 → 20801
pub async fn collect_repository(
    client: &dyn RegistryClient,
    repository: &str,
) -> Result<RepositoryDetail, ImageHubError> {
    validate_repository(repository)?;
    let tags = client.list_tags(repository).await?;
    if tags.is_empty() {
        return Err(ImageHubError::new(
            codes::MANIFEST_NOT_FOUND,
            format!("仓库 {repository} 不存在或无标签"),
        ));
    }
    let mut out = Vec::with_capacity(tags.len());
    for tag in tags {
        let mut tag_info = describe_tag(client, repository, tag).await;
        enrich_platform_sizes(client, repository, &mut tag_info).await;
        out.push(tag_info);
    }
    Ok(RepositoryDetail {
        repository: repository.to_string(),
        tags: out,
    })
}

/// 平台条目补齐上限（防止超大 index 触发过多子 manifest 请求）
pub const MAX_ENRICH_PLATFORMS: usize = 24;

/// 是否需要为平台条目**逐个补齐**真实大小（纯函数，D-IH-49）：
/// - 无平台条目 / 条目过多 → 否
/// - manifest list / image index → 是（其条目的 `size` 只是子 manifest 大小）
/// - 单平台 tag 的合成条目（唯一条目 digest 即该 tag digest）→ 否（大小已是该 tag 真实大小，
///   避免为此多发一次请求）
/// - media type 缺失（无法判断）且非合成条目 → 沿用旧行为（是）
pub fn should_enrich_platform_sizes(tag: &ImageTag) -> bool {
    if tag.platforms.is_empty() || tag.platforms.len() > MAX_ENRICH_PLATFORMS {
        return false;
    }
    if is_index_media_type(tag.media_type.as_deref()) {
        return true;
    }
    let synthesized_single = tag.platforms.len() == 1
        && tag.platforms[0].digest.is_some()
        && tag.platforms[0].digest == tag.digest;
    !synthesized_single
}

/// 为多架构 tag 的平台条目补齐真实镜像大小（拉取子 manifest 的 layers + config 合计）；
/// 全部平台均成功时，把 tag 的 size_bytes 更新为各平台合计。
async fn enrich_platform_sizes(client: &dyn RegistryClient, repository: &str, tag: &mut ImageTag) {
    if !should_enrich_platform_sizes(tag) {
        return;
    }
    let mut all_known = true;
    for entry in tag.platforms.iter_mut() {
        match entry.digest.clone() {
            Some(digest) => match client.describe_manifest(repository, &digest).await {
                Ok(info) => match info.size_bytes {
                    Some(size) => entry.size_bytes = Some(size),
                    None => all_known = false,
                },
                Err(_) => all_known = false,
            },
            None => all_known = false,
        }
    }
    if all_known {
        let total: u64 = tag.platforms.iter().filter_map(|p| p.size_bytes).sum();
        tag.size_bytes = Some(total);
    }
}

/// 删除镜像：`tag = Some` 删单标签（含其 index 子清单）；`tag = None` 删整仓全部标签
///
/// 核心语义（见 D-IH-26 ~ D-IH-28）：
/// - 目标为 manifest list / image index 时**级联删除**其引用的全部子清单（含 attestation）；
/// - 单标签删除时保留仍被同仓库其它 tag 引用的子清单（无法判定时保守地不删子清单）；
/// - 所有 digest **先收集后删除**（删掉顶层 manifest 后 tag 消失，无法再解析其子清单）；
/// - digest 去重；目标已不存在（404）视为成功继续。
pub async fn delete_image(
    client: &dyn RegistryClient,
    repository: &str,
    tag: Option<&str>,
) -> Result<DeleteResult, ImageHubError> {
    validate_repository(repository)?;
    match tag {
        Some(t) => {
            validate_tag(t)?;
            delete_single_tag(client, repository, t).await
        }
        None => delete_repository(client, repository).await,
    }
}

/// 删除单标签：顶层 manifest + 其子清单（保留仍被其它 tag 引用的子清单）
async fn delete_single_tag(
    client: &dyn RegistryClient,
    repository: &str,
    tag: &str,
) -> Result<DeleteResult, ImageHubError> {
    // 先收集：删除顶层 manifest 会使 tag 消失，此后无法再解析其子清单
    let info = client.describe_manifest(repository, tag).await?;
    let digest = info.digest.ok_or_else(|| {
        ImageHubError::new(
            codes::MANIFEST_NOT_FOUND,
            format!("未取到 {repository}:{tag} 的 digest"),
        )
    })?;

    let mut targets: Vec<String> = vec![digest];
    if !info.child_digests.is_empty() {
        match referenced_child_digests(client, repository, tag).await {
            Some(keep) => {
                targets.extend(info.child_digests.iter().filter(|d| !keep.contains(*d)).cloned());
            }
            None => {
                tracing::warn!(
                    repository = %repository,
                    tag = %tag,
                    "无法判定其它标签引用的子清单，保守地仅删除顶层 manifest"
                );
            }
        }
    }

    delete_digests(client, repository, &targets).await?;
    Ok(DeleteResult {
        repository: repository.to_string(),
        deleted_tags: vec![tag.to_string()],
    })
}

/// 汇总同仓库**其它** tag 引用的子清单 digest；任一标签解析失败返回 `None`（调用方保守处理）
async fn referenced_child_digests(
    client: &dyn RegistryClient,
    repository: &str,
    exclude_tag: &str,
) -> Option<std::collections::HashSet<String>> {
    let tags = match client.list_tags(repository).await {
        Ok(t) => t,
        Err(_) => return None,
    };
    let mut keep = std::collections::HashSet::new();
    for t in tags {
        if t == exclude_tag {
            continue;
        }
        match client.describe_manifest(repository, &t).await {
            Ok(info) => keep.extend(info.child_digests),
            Err(e) => {
                tracing::warn!(repository = %repository, tag = %t, err = %e.msg, "解析标签失败");
                return None;
            }
        }
    }
    Some(keep)
}

/// 删除整仓：收集全部 tag 的顶层 digest 与子清单 digest，合并去重后删除
async fn delete_repository(
    client: &dyn RegistryClient,
    repository: &str,
) -> Result<DeleteResult, ImageHubError> {
    let tags = client.list_tags(repository).await?;
    if tags.is_empty() {
        return Err(ImageHubError::new(
            codes::MANIFEST_NOT_FOUND,
            format!("仓库 {repository} 无标签可删除（可能需 registry GC）"),
        ));
    }

    let mut targets: Vec<String> = Vec::new();
    for t in &tags {
        let info = client.describe_manifest(repository, t).await?;
        let digest = info.digest.ok_or_else(|| {
            ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("未取到 {repository}:{t} 的 digest"),
            )
        })?;
        targets.push(digest);
        targets.extend(info.child_digests);
    }

    delete_digests(client, repository, &targets).await?;
    Ok(DeleteResult {
        repository: repository.to_string(),
        deleted_tags: tags,
    })
}

/// 逐个删除 digest：去重；目标已不存在（404 / 20803）视为成功并继续
async fn delete_digests(
    client: &dyn RegistryClient,
    repository: &str,
    targets: &[String],
) -> Result<(), ImageHubError> {
    let mut seen = std::collections::HashSet::new();
    for digest in targets {
        if !seen.insert(digest.as_str()) {
            continue;
        }
        match client.delete_manifest(repository, digest).await {
            Ok(()) => {}
            Err(e) if e.code == codes::MANIFEST_NOT_FOUND => {
                tracing::warn!(repository = %repository, digest = %digest, "manifest 已不存在，跳过");
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

// ==================== 生产实现 ====================

/// 生产 registry 客户端（reqwest + rustls）
pub struct RealRegistryClient {
    base_url: String,
    client: reqwest::Client,
}

impl RealRegistryClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(3))
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            client,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }
}

#[async_trait]
impl RegistryClient for RealRegistryClient {
    async fn list_repositories(&self) -> Result<Vec<String>, ImageHubError> {
        let url = self.url("/v2/_catalog?n=1000");
        let resp = self.client.get(&url).send().await.map_err(|e| {
            ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("请求 {url} 失败: {e}"))
        })?;
        if !resp.status().is_success() {
            return Err(ImageHubError::new(
                codes::REGISTRY_UNREACHABLE,
                format!("{url} 返回 {}", resp.status()),
            ));
        }
        let body: Value = resp.json().await.map_err(|e| {
            ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("解析 {url} 失败: {e}"))
        })?;
        Ok(parse_repositories(&body))
    }

    async fn list_tags(&self, repository: &str) -> Result<Vec<String>, ImageHubError> {
        let url = self.url(&format!("/v2/{repository}/tags/list"));
        let resp = self.client.get(&url).send().await.map_err(|e| {
            ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("请求 {url} 失败: {e}"))
        })?;
        // 无标签仓库可能 404（MANIFEST_UNKNOWN），视为空列表
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        if !resp.status().is_success() {
            return Err(ImageHubError::new(
                codes::REGISTRY_UNREACHABLE,
                format!("{url} 返回 {}", resp.status()),
            ));
        }
        let body: Value = resp.json().await.map_err(|e| {
            ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("解析 {url} 失败: {e}"))
        })?;
        Ok(parse_tags(&body))
    }

    async fn describe_manifest(
        &self,
        repository: &str,
        reference: &str,
    ) -> Result<ManifestInfo, ImageHubError> {
        let url = self.url(&format!("/v2/{repository}/manifests/{reference}"));
        let resp = self
            .client
            .get(&url)
            .header(reqwest::header::ACCEPT, MANIFEST_ACCEPT)
            .send()
            .await
            .map_err(|e| {
                ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("请求 {url} 失败: {e}"))
            })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("{repository}:{reference} 不存在"),
            ));
        }
        if !resp.status().is_success() {
            return Err(ImageHubError::new(
                codes::REGISTRY_UNREACHABLE,
                format!("{url} 返回 {}", resp.status()),
            ));
        }
        let digest = resp
            .headers()
            .get("docker-content-digest")
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string);
        let body: Value = resp.json().await.map_err(|e| {
            ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("解析 {url} 失败: {e}"))
        })?;
        let mut info = parse_manifest(&body, digest, content_type);
        // 尽力而为：拉一次 config blob，同时取 created 与单平台信息（os/architecture/variant）
        if let Some(cfg) = config_digest(&body)
            && let Ok(meta) = self.fetch_config_meta(repository, &cfg).await
        {
            info.created = meta.created;
            info.platform = meta.platform;
        }
        Ok(info)
    }

    async fn delete_manifest(&self, repository: &str, digest: &str) -> Result<(), ImageHubError> {
        let url = self.url(&format!("/v2/{repository}/manifests/{digest}"));
        let resp = self.client.delete(&url).send().await.map_err(|e| {
            ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("请求 {url} 失败: {e}"))
        })?;
        if resp.status().is_success() {
            return Ok(());
        }
        // 404：目标 manifest 已不存在（并发删除 / 共享 digest / 历史残留），调用方按幂等处理
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("{repository}@{digest} 已不存在"),
            ));
        }
        Err(ImageHubError::new(
            codes::DELETE_FAILED,
            format!("删除 {repository}@{digest} 失败：{}", resp.status()),
        ))
    }

    async fn untag_manifest(&self, repository: &str, tag: &str) -> Result<(), ImageHubError> {
        let url = self.url(&format!("/v2/{repository}/manifests/{tag}"));
        let resp = self.client.delete(&url).send().await.map_err(|e| {
            ImageHubError::new(codes::REGISTRY_UNREACHABLE, format!("请求 {url} 失败: {e}"))
        })?;
        // registry 按 tag 删除实现为「解析 tag 后摘除标签」，成功返回 202
        if resp.status().is_success() {
            return Ok(());
        }
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("{repository}:{tag} 已不存在"),
            ));
        }
        Err(ImageHubError::new(
            codes::DELETE_FAILED,
            format!("摘除标签 {repository}:{tag} 失败：{}", resp.status()),
        ))
    }

    async fn ping(&self) -> bool {
        matches!(
            self.client.get(self.url("/v2/")).send().await,
            Ok(r) if r.status().is_success()
        )
    }
}

/// config blob 中我们关心的元信息
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigMeta {
    pub created: Option<String>,
    pub platform: Option<PlatformEntry>,
}

impl RealRegistryClient {
    /// 拉取 config blob 并提取 created 与平台字段（一次请求；失败返回默认值，不阻断）
    async fn fetch_config_meta(
        &self,
        repository: &str,
        config_digest: &str,
    ) -> Result<ConfigMeta, ImageHubError> {
        let url = self.url(&format!("/v2/{repository}/blobs/{config_digest}"));
        let resp = match self.client.get(&url).send().await {
            Ok(r) if r.status().is_success() => r,
            _ => return Ok(ConfigMeta::default()),
        };
        match resp.json::<Value>().await {
            Ok(body) => Ok(ConfigMeta {
                created: config_created(&body),
                platform: parse_config_platform(&body),
            }),
            Err(_) => Ok(ConfigMeta::default()),
        }
    }
}

// ==================== 测试替身 ====================

/// 内存 mock：以 `(repository, tag) -> digest` 映射模拟 registry
#[derive(Default)]
pub struct MockRegistryClient {
    pub repositories: Vec<String>,
    /// `repository -> tags`（Mutex：`untag_manifest` 需在 `&self` 下摘标签）
    pub tags: std::sync::Mutex<std::collections::HashMap<String, Vec<String>>>,
    pub digests: std::collections::HashMap<(String, String), String>,
    /// 预置的多架构平台条目（key = (repository, reference)）
    pub platforms: std::collections::HashMap<(String, String), Vec<PlatformEntry>>,
    /// 预置的单平台信息（key = (repository, reference)；模拟来自 config blob）
    pub single_platform: std::collections::HashMap<(String, String), PlatformEntry>,
    /// 预置的 media type（key = (repository, reference)；缺省为单平台 image manifest）
    pub media_types: std::collections::HashMap<(String, String), String>,
    /// 预置的子清单 digest（key = (repository, reference)）；删除时会级联删除
    pub children: std::collections::HashMap<(String, String), Vec<String>>,
    pub deleted: std::sync::Mutex<Vec<(String, String)>>,
    /// `untag_manifest` 调用记录（改名语义断言用）
    pub untagged: std::sync::Mutex<Vec<(String, String)>>,
    /// `describe_manifest` 调用记录（断言「不额外发请求」用）
    pub describe_calls: std::sync::Mutex<Vec<(String, String)>>,
    pub reachable: bool,
}

impl MockRegistryClient {
    pub fn new() -> Self {
        Self {
            reachable: true,
            ..Default::default()
        }
    }

    pub fn seed(&mut self, repository: &str, tag: &str, digest: &str) {
        if !self.repositories.iter().any(|r| r == repository) {
            self.repositories.push(repository.to_string());
        }
        self.tags
            .lock()
            .unwrap()
            .entry(repository.to_string())
            .or_default()
            .push(tag.to_string());
        self.digests
            .insert((repository.to_string(), tag.to_string()), digest.to_string());
    }

    /// 仅登记标签（不登记 digest），供「标签存在但无法解析 manifest」的降级场景
    pub fn seed_tag_only(&mut self, repository: &str, tag: &str) {
        if !self.repositories.iter().any(|r| r == repository) {
            self.repositories.push(repository.to_string());
        }
        self.tags
            .lock()
            .unwrap()
            .entry(repository.to_string())
            .or_default()
            .push(tag.to_string());
    }

    /// 为某 tag 预置多架构平台条目（同时把其 media type 置为 OCI image index）
    pub fn seed_platforms(&mut self, repository: &str, tag: &str, entries: Vec<PlatformEntry>) {
        self.platforms
            .insert((repository.to_string(), tag.to_string()), entries);
        self.media_types.insert(
            (repository.to_string(), tag.to_string()),
            OCI_INDEX_MEDIA_TYPE.to_string(),
        );
    }

    /// 为某 reference 预置单平台信息（模拟 config blob 的 os/architecture/variant）
    pub fn seed_single_platform(
        &mut self,
        repository: &str,
        reference: &str,
        platform: PlatformEntry,
    ) {
        self.single_platform
            .insert((repository.to_string(), reference.to_string()), platform);
    }

    /// 预置某 reference 的 media type
    pub fn seed_media_type(&mut self, repository: &str, reference: &str, media_type: &str) {
        self.media_types.insert(
            (repository.to_string(), reference.to_string()),
            media_type.to_string(),
        );
    }

    /// 仅登记 manifest 引用（不新增 tag），供平台条目大小补齐等场景
    pub fn seed_manifest(&mut self, repository: &str, reference: &str, digest: &str) {
        self.digests
            .insert((repository.to_string(), reference.to_string()), digest.to_string());
    }

    /// 为某 tag / reference 预置 index 子清单 digest（删除时会被级联删除）
    pub fn seed_children(&mut self, repository: &str, reference: &str, children: Vec<String>) {
        self.children
            .insert((repository.to_string(), reference.to_string()), children);
    }

    /// `describe_manifest` 的调用次数（按 reference 计数）
    pub fn describe_call_count(&self) -> usize {
        self.describe_calls.lock().unwrap().len()
    }

    /// mock 已登记的 `(repository, digest)` 集合（判定 digest 是否存在）
    ///
    /// 仅含经 `seed` / `seed_manifest` 显式登记的 manifest digest；测试中需要「子清单存在」
    /// 时须用 `seed_manifest` 登记，否则删除会得到 404（供幂等分支测试）。
    fn known_digests(&self) -> std::collections::HashSet<(String, String)> {
        self.digests
            .iter()
            .map(|((r, _), d)| (r.clone(), d.clone()))
            .collect()
    }
}

#[async_trait]
impl RegistryClient for MockRegistryClient {
    async fn list_repositories(&self) -> Result<Vec<String>, ImageHubError> {
        if !self.reachable {
            return Err(ImageHubError::new(codes::REGISTRY_UNREACHABLE, "mock registry 不可达"));
        }
        Ok(self.repositories.clone())
    }

    async fn list_tags(&self, repository: &str) -> Result<Vec<String>, ImageHubError> {
        Ok(self
            .tags
            .lock()
            .unwrap()
            .get(repository)
            .cloned()
            .unwrap_or_default())
    }

    async fn describe_manifest(
        &self,
        repository: &str,
        reference: &str,
    ) -> Result<ManifestInfo, ImageHubError> {
        self.describe_calls
            .lock()
            .unwrap()
            .push((repository.to_string(), reference.to_string()));
        // 已摘除的标签不再可解析（模拟真实 registry 的 untag 后表现）
        if self
            .untagged
            .lock()
            .unwrap()
            .iter()
            .any(|(r, t)| r == repository && t == reference)
        {
            return Err(ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("{repository}:{reference} 不存在"),
            ));
        }
        match self.digests.get(&(repository.to_string(), reference.to_string())) {
            Some(d) => Ok(ManifestInfo {
                digest: Some(d.clone()),
                size_bytes: Some(1024),
                created: Some("2026-09-14T00:00:00Z".to_string()),
                media_type: Some(
                    self.media_types
                        .get(&(repository.to_string(), reference.to_string()))
                        .cloned()
                        .unwrap_or_else(|| {
                            "application/vnd.oci.image.manifest.v1+json".to_string()
                        }),
                ),
                platforms: self
                    .platforms
                    .get(&(repository.to_string(), reference.to_string()))
                    .cloned()
                    .unwrap_or_default(),
                platform: self
                    .single_platform
                    .get(&(repository.to_string(), reference.to_string()))
                    .cloned(),
                child_digests: self
                    .children
                    .get(&(repository.to_string(), reference.to_string()))
                    .cloned()
                    .unwrap_or_default(),
            }),
            None => Err(ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("{repository}:{reference} 不存在"),
            )),
        }
    }

    async fn delete_manifest(&self, repository: &str, digest: &str) -> Result<(), ImageHubError> {
        // 模拟真实 registry：目标 manifest 不存在时返回 404（供幂等分支测试）
        if !self.known_digests().contains(&(repository.to_string(), digest.to_string())) {
            return Err(ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("{repository}@{digest} 已不存在"),
            ));
        }
        // 模拟真实 registry：按 digest 删除会摘除**所有**指向该 digest 的标签
        // （与 `untag_manifest` 仅摘单个标签形成对照，见 D-IH-51）
        let affected: Vec<String> = self
            .digests
            .iter()
            .filter(|((repo, _), d)| repo == repository && d.as_str() == digest)
            .map(|((_, tag), _)| tag.clone())
            .collect();
        self.deleted
            .lock()
            .unwrap()
            .push((repository.to_string(), digest.to_string()));
        if !affected.is_empty()
            && let Some(list) = self.tags.lock().unwrap().get_mut(repository)
        {
            list.retain(|t| !affected.contains(t));
        }
        Ok(())
    }

    async fn untag_manifest(&self, repository: &str, tag: &str) -> Result<(), ImageHubError> {
        // 模拟真实 registry：按 tag 删除仅摘除该标签（保留同 digest 的其它 tag 与 revision）
        let present = self
            .tags
            .lock()
            .unwrap()
            .get(repository)
            .is_some_and(|list| list.iter().any(|t| t == tag));
        if !present {
            return Err(ImageHubError::new(
                codes::MANIFEST_NOT_FOUND,
                format!("{repository}:{tag} 已不存在"),
            ));
        }
        self.untagged
            .lock()
            .unwrap()
            .push((repository.to_string(), tag.to_string()));
        if let Some(list) = self.tags.lock().unwrap().get_mut(repository) {
            list.retain(|t| t != tag);
        }
        Ok(())
    }

    async fn ping(&self) -> bool {
        self.reachable
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_catalog_and_tags() {
        let cat = json!({"repositories": ["docker.io/foo/bar", "library/nginx"]});
        assert_eq!(
            parse_repositories(&cat),
            vec!["docker.io/foo/bar", "library/nginx"]
        );
        let t = json!({"name": "docker.io/foo/bar", "tags": ["1.0", "latest"]});
        assert_eq!(parse_tags(&t), vec!["1.0", "latest"]);
        let empty = json!({"name": "x", "tags": null});
        assert!(parse_tags(&empty).is_empty());
    }

    #[test]
    fn parse_image_manifest_size() {
        let m = json!({
            "mediaType": "application/vnd.docker.distribution.manifest.v2+json",
            "config": {"digest": "sha256:cfg", "size": 100},
            "layers": [{"size": 200}, {"size": 300}]
        });
        let info = parse_manifest(&m, Some("sha256:top".into()), None);
        assert_eq!(info.size_bytes, Some(600));
        assert_eq!(info.digest.as_deref(), Some("sha256:top"));
        assert_eq!(config_digest(&m).as_deref(), Some("sha256:cfg"));
    }

    #[test]
    fn parse_index_manifest_size_is_unknown() {
        let m = json!({
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [{"size": 10}, {"size": 20}]
        });
        let info = parse_manifest(&m, Some("sha256:idx".into()), None);
        // 索引的子 manifest 大小不是镜像大小 → size 未知
        assert_eq!(info.size_bytes, None);
        assert!(config_digest(&m).is_none());
        // 无 platform 的条目被忽略
        assert!(info.platforms.is_empty());
        // 条目无 digest → 子清单为空
        assert!(info.child_digests.is_empty());
    }

    #[test]
    fn parse_platforms_from_index() {
        let m = json!({
            "mediaType": "application/vnd.oci.image.index.v1+json",
            "manifests": [
                {"digest": "sha256:a", "size": 100,
                 "platform": {"os": "linux", "architecture": "amd64"}},
                {"digest": "sha256:b", "size": 200,
                 "platform": {"os": "linux", "architecture": "arm64", "variant": "v8"}},
                {"digest": "sha256:c", "size": 5,
                 "platform": {"os": "unknown", "architecture": "unknown"}},
                {"digest": "sha256:d", "size": 4, "annotations": {"vnd.docker.reference.type": "attestation-manifest"}}
            ]
        });
        let platforms = parse_platforms(&m);
        assert_eq!(platforms.len(), 2, "attestation / unknown 平台条目被忽略");
        assert_eq!(platforms[0].platform_string(), "linux/amd64");
        assert_eq!(platforms[0].digest.as_deref(), Some("sha256:a"));
        assert_eq!(platforms[0].size_bytes, Some(100));
        assert_eq!(platforms[1].platform_string(), "linux/arm64/v8");
        // 子清单 digest：**不过滤** attestation / unknown（删除时须一并清理）
        assert_eq!(
            parse_child_digests(&m),
            vec!["sha256:a", "sha256:b", "sha256:c", "sha256:d"]
        );
        // 单平台 manifest 无 manifests 字段
        assert!(parse_platforms(&json!({"layers": []})).is_empty());
        assert!(parse_child_digests(&json!({"layers": []})).is_empty());
    }

    #[tokio::test]
    async fn collect_repository_returns_tags_and_platforms() {
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.m.daocloud.io/library/alpine", "3.20", "sha256:idx");
        mock.seed_manifest(
            "docker.m.daocloud.io/library/alpine",
            "sha256:amd",
            "sha256:amd",
        );
        mock.seed_manifest(
            "docker.m.daocloud.io/library/alpine",
            "sha256:arm",
            "sha256:arm",
        );
        mock.seed_platforms(
            "docker.m.daocloud.io/library/alpine",
            "3.20",
            vec![
                PlatformEntry {
                    os: Some("linux".into()),
                    architecture: Some("amd64".into()),
                    variant: None,
                    digest: Some("sha256:amd".into()),
                    size_bytes: None,
                },
                PlatformEntry {
                    os: Some("linux".into()),
                    architecture: Some("arm64".into()),
                    variant: None,
                    digest: Some("sha256:arm".into()),
                    size_bytes: None,
                },
            ],
        );

        let detail = collect_repository(&mock, "docker.m.daocloud.io/library/alpine")
            .await
            .unwrap();
        assert_eq!(detail.repository, "docker.m.daocloud.io/library/alpine");
        assert_eq!(detail.tags.len(), 1);
        assert_eq!(detail.tags[0].tag, "3.20");
        assert_eq!(detail.tags[0].platforms.len(), 2);
        // 平台大小由子 manifest 补齐（mock 固定 1024），tag 合计 = 2048
        assert_eq!(detail.tags[0].platforms[0].size_bytes, Some(1024));
        assert_eq!(detail.tags[0].size_bytes, Some(2048));

        // 不存在仓库 → 20803
        let err = collect_repository(&mock, "no/such/repo").await.unwrap_err();
        assert_eq!(err.code, codes::MANIFEST_NOT_FOUND);
        // 非法仓库名 → 20801
        let err = collect_repository(&mock, "Bad/Repo").await.unwrap_err();
        assert_eq!(err.code, codes::PARAM_INVALID);
    }

    #[test]
    fn parse_config_platform_and_index_media_type() {
        // 完整：os + architecture + variant
        let full = parse_config_platform(&json!({
            "os": "linux", "architecture": "arm64", "variant": "v8"
        }))
        .unwrap();
        assert_eq!(full.os.as_deref(), Some("linux"));
        assert_eq!(full.architecture.as_deref(), Some("arm64"));
        assert_eq!(full.variant.as_deref(), Some("v8"));
        assert!(full.digest.is_none() && full.size_bytes.is_none());

        // 无 variant（如 amd64）
        let no_variant =
            parse_config_platform(&json!({"os": "linux", "architecture": "amd64"})).unwrap();
        assert_eq!(no_variant.variant, None);

        // 缺 os/architecture → None
        assert!(parse_config_platform(&json!({})).is_none());
        assert!(parse_config_platform(&json!({"variant": "v8"})).is_none());
        // 空串 / 空白串视为缺失
        assert!(parse_config_platform(&json!({"os": "", "architecture": "  "})).is_none());
        assert_eq!(
            parse_config_platform(&json!({"os": "linux", "architecture": " "}))
                .unwrap()
                .architecture,
            None
        );

        // index media type 判定
        assert!(is_index_media_type(Some(DOCKER_INDEX_MEDIA_TYPE)));
        assert!(is_index_media_type(Some(OCI_INDEX_MEDIA_TYPE)));
        assert!(!is_index_media_type(Some(
            "application/vnd.oci.image.manifest.v1+json"
        )));
        assert!(!is_index_media_type(Some(
            "application/vnd.docker.distribution.manifest.v2+json"
        )));
        assert!(!is_index_media_type(None));
    }

    #[test]
    fn should_enrich_platform_sizes_rules() {
        let single = ImageTag {
            tag: "3.20".into(),
            digest: Some("sha256:self".into()),
            size_bytes: Some(1024),
            created: None,
            media_type: Some("application/vnd.oci.image.manifest.v1+json".into()),
            platforms: vec![PlatformEntry {
                os: Some("linux".into()),
                architecture: Some("arm64".into()),
                variant: None,
                digest: Some("sha256:self".into()),
                size_bytes: Some(1024),
            }],
        };
        assert!(!should_enrich_platform_sizes(&single), "单平台合成条目无需补齐");

        let mut index = single.clone();
        index.digest = Some("sha256:idx".into());
        index.media_type = Some(OCI_INDEX_MEDIA_TYPE.into());
        index.platforms[0].digest = Some("sha256:child".into());
        assert!(should_enrich_platform_sizes(&index), "index 需逐平台补齐");

        let mut unknown_media = index.clone();
        unknown_media.media_type = None;
        assert!(should_enrich_platform_sizes(&unknown_media), "media type 未知沿用旧行为");

        let mut no_platforms = index.clone();
        no_platforms.platforms.clear();
        assert!(!should_enrich_platform_sizes(&no_platforms));
    }

    #[tokio::test]
    async fn single_platform_tag_gets_platform_entry_without_extra_request() {
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.io/library/ubuntu", "22.04", "sha256:u2204");
        mock.seed_single_platform(
            "docker.io/library/ubuntu",
            "22.04",
            PlatformEntry {
                os: Some("linux".into()),
                architecture: Some("arm64".into()),
                variant: None,
                digest: None,
                size_bytes: None,
            },
        );

        let detail = collect_repository(&mock, "docker.io/library/ubuntu")
            .await
            .unwrap();
        let tag = &detail.tags[0];
        assert_eq!(tag.platforms.len(), 1, "单平台 tag 合成 1 条平台条目");
        let p = &tag.platforms[0];
        assert_eq!(p.os.as_deref(), Some("linux"));
        assert_eq!(p.architecture.as_deref(), Some("arm64"));
        assert_eq!(p.variant, None);
        assert_eq!(p.digest.as_deref(), Some("sha256:u2204"), "digest 为 tag 自身");
        assert_eq!(p.size_bytes, Some(1024), "大小为该 tag 的真实大小");
        assert_eq!(tag.size_bytes, Some(1024), "tag 大小不被覆盖为合计");
        assert_eq!(
            mock.describe_call_count(),
            1,
            "单平台 tag 只应有一次 manifest 请求（不做逐平台补齐）"
        );
    }

    #[tokio::test]
    async fn single_platform_tag_without_platform_fields_stays_empty() {
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.io/library/ubuntu", "24.04", "sha256:u2404");
        // 未预置单平台信息 → config 缺 os/architecture（降级）
        let detail = collect_repository(&mock, "docker.io/library/ubuntu")
            .await
            .unwrap();
        assert!(detail.tags[0].platforms.is_empty());
        assert_eq!(detail.tags[0].digest.as_deref(), Some("sha256:u2404"));
        assert_eq!(mock.describe_call_count(), 1);
    }

    #[tokio::test]
    async fn index_tag_still_enriches_platform_sizes() {
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.io/library/ubuntu", "24.04", "sha256:idx24");
        mock.seed_manifest("docker.io/library/ubuntu", "sha256:amd", "sha256:amd");
        mock.seed_manifest("docker.io/library/ubuntu", "sha256:arm", "sha256:arm");
        mock.seed_platforms(
            "docker.io/library/ubuntu",
            "24.04",
            vec![
                PlatformEntry {
                    os: Some("linux".into()),
                    architecture: Some("amd64".into()),
                    variant: None,
                    digest: Some("sha256:amd".into()),
                    size_bytes: None,
                },
                PlatformEntry {
                    os: Some("linux".into()),
                    architecture: Some("arm64".into()),
                    variant: None,
                    digest: Some("sha256:arm".into()),
                    size_bytes: None,
                },
            ],
        );

        let detail = collect_repository(&mock, "docker.io/library/ubuntu")
            .await
            .unwrap();
        let tag = &detail.tags[0];
        assert_eq!(tag.platforms.len(), 2);
        assert_eq!(tag.platforms[0].size_bytes, Some(1024));
        assert_eq!(tag.platforms[1].size_bytes, Some(1024));
        assert_eq!(tag.size_bytes, Some(2048), "index 的 tag 大小为各平台合计");
        // 1 次顶层 + 2 次子 manifest
        assert_eq!(mock.describe_call_count(), 3);
    }

    #[test]
    fn validate_repository_rules() {
        assert!(validate_repository("docker.io/foo/bar").is_ok());
        assert!(validate_repository("").is_err());
        assert!(validate_repository("Foo/Bar").is_err());
        assert!(validate_repository("../etc/passwd").is_err());
    }

    #[test]
    fn validate_tag_rules() {
        assert!(validate_tag("latest").is_ok());
        assert!(validate_tag("v1.2.3-rc1").is_ok());
        assert!(validate_tag("").is_err());
        assert!(validate_tag("bad tag").is_err());
    }

    #[tokio::test]
    async fn collect_and_delete_images() {
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.io/foo/bar", "1.0", "sha256:a");
        mock.seed("docker.io/foo/bar", "latest", "sha256:b");
        mock.seed("empty/repo", "stale", "sha256:c");

        let images = collect_images(&mock).await.unwrap();
        assert_eq!(images.len(), 2);
        assert_eq!(images[0].repository, "docker.io/foo/bar");
        assert_eq!(images[0].tags.len(), 2);

        // 删单 tag
        let res = delete_image(&mock, "docker.io/foo/bar", Some("1.0")).await.unwrap();
        assert_eq!(res.deleted_tags, vec!["1.0"]);
        // 删整仓
        let res = delete_image(&mock, "empty/repo", None).await.unwrap();
        assert_eq!(res.deleted_tags, vec!["stale"]);
        // 非法仓库
        assert!(delete_image(&mock, "Bad/Repo", None).await.is_err());
    }

    #[tokio::test]
    async fn collect_images_hides_untagged_repository() {
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.io/foo/bar", "1.0", "sha256:a");
        // 无标签仓库：registry 中仍有目录/残留 manifest，但 tags/list 为空
        mock.repositories.push("test.mirror/alpine".to_string());
        mock.seed_manifest("test.mirror/alpine", "sha256:orphan", "sha256:orphan");

        let images = collect_images(&mock).await.unwrap();
        assert_eq!(images.len(), 1, "无标签仓库不出现在列表中");
        assert_eq!(images[0].repository, "docker.io/foo/bar");
    }

    /// `docker.m.daocloud.io/library/alpine:3.20` 形态：index + 平台/attestation 子清单
    fn seed_index_image(mock: &mut MockRegistryClient, repo: &str, tag: &str, digest: &str) {
        mock.seed(repo, tag, digest);
        for (d, _platform) in [
            ("sha256:amd", "linux/amd64"),
            ("sha256:arm", "linux/arm64"),
            ("sha256:att", "unknown/unknown"),
        ] {
            mock.seed_manifest(repo, d, d);
        }
        mock.seed_children(
            repo,
            tag,
            vec!["sha256:amd".into(), "sha256:arm".into(), "sha256:att".into()],
        );
    }

    #[tokio::test]
    async fn delete_single_tag_cascades_index_children() {
        let repo = "docker.m.daocloud.io/library/alpine";
        let mut mock = MockRegistryClient::new();
        seed_index_image(&mut mock, repo, "3.20", "sha256:idx");

        let res = delete_image(&mock, repo, Some("3.20")).await.unwrap();
        assert_eq!(res.deleted_tags, vec!["3.20"]);
        let deleted = mock.deleted.lock().unwrap().clone();
        assert_eq!(
            deleted,
            vec![
                (repo.to_string(), "sha256:idx".to_string()),
                (repo.to_string(), "sha256:amd".to_string()),
                (repo.to_string(), "sha256:arm".to_string()),
                (repo.to_string(), "sha256:att".to_string()),
            ],
            "顶层 index 与全部子清单（含 attestation）均被删除"
        );
    }

    #[tokio::test]
    async fn delete_single_tag_keeps_children_referenced_by_other_tags() {
        let repo = "docker.io/foo/multi";
        let mut mock = MockRegistryClient::new();
        mock.seed(repo, "A", "sha256:idx-a");
        mock.seed(repo, "B", "sha256:idx-b");
        mock.seed_manifest(repo, "sha256:shared", "sha256:shared");
        mock.seed_manifest(repo, "sha256:only-a", "sha256:only-a");
        mock.seed_children(repo, "A", vec!["sha256:shared".into(), "sha256:only-a".into()]);
        mock.seed_children(repo, "B", vec!["sha256:shared".into()]);

        delete_image(&mock, repo, Some("A")).await.unwrap();
        let deleted = mock.deleted.lock().unwrap().clone();
        let digests: Vec<&str> = deleted.iter().map(|(_, d)| d.as_str()).collect();
        assert!(digests.contains(&"sha256:idx-a"));
        assert!(digests.contains(&"sha256:only-a"));
        assert!(!digests.contains(&"sha256:shared"), "被 tag B 引用的子清单必须保留");
    }

    #[tokio::test]
    async fn delete_single_tag_conservative_when_other_tag_unresolvable() {
        let repo = "docker.io/foo/multi";
        let mut mock = MockRegistryClient::new();
        mock.seed(repo, "A", "sha256:idx-a");
        // tag B 存在但 manifest 无法解析（未登记 digest）
        mock.seed_tag_only(repo, "B");
        mock.seed_manifest(repo, "sha256:c", "sha256:c");
        mock.seed_children(repo, "A", vec!["sha256:c".into()]);

        let res = delete_image(&mock, repo, Some("A")).await.unwrap();
        assert_eq!(res.deleted_tags, vec!["A"]);
        let deleted = mock.deleted.lock().unwrap().clone();
        assert_eq!(
            deleted,
            vec![(repo.to_string(), "sha256:idx-a".to_string())],
            "无法判定其它标签引用时保守地仅删除顶层 manifest"
        );
    }

    #[tokio::test]
    async fn delete_repository_dedupes_shared_digests() {
        let repo = "docker.io/foo/dup";
        let mut mock = MockRegistryClient::new();
        // A / B 指向同一 index digest，并共享同一子清单
        mock.seed(repo, "A", "sha256:idx");
        mock.seed(repo, "B", "sha256:idx");
        mock.seed_manifest(repo, "sha256:shared", "sha256:shared");
        mock.seed_children(repo, "A", vec!["sha256:shared".into()]);
        mock.seed_children(repo, "B", vec!["sha256:shared".into()]);

        let res = delete_image(&mock, repo, None).await.unwrap();
        assert_eq!(res.deleted_tags, vec!["A", "B"]);
        let deleted = mock.deleted.lock().unwrap().clone();
        assert_eq!(deleted.iter().filter(|(_, d)| d == "sha256:idx").count(), 1);
        assert_eq!(deleted.iter().filter(|(_, d)| d == "sha256:shared").count(), 1);
    }

    #[tokio::test]
    async fn delete_ignores_missing_manifest_as_idempotent() {
        let repo = "docker.io/foo/gone";
        let mut mock = MockRegistryClient::new();
        mock.seed(repo, "1.0", "sha256:idx");
        // 子清单未登记 → mock 返回 404（模拟并发删除 / 历史残留）
        mock.seed_children(repo, "1.0", vec!["sha256:missing".into()]);

        let res = delete_image(&mock, repo, Some("1.0")).await.unwrap();
        assert_eq!(res.deleted_tags, vec!["1.0"]);
        let deleted = mock.deleted.lock().unwrap().clone();
        assert_eq!(deleted, vec![(repo.to_string(), "sha256:idx".to_string())]);
    }

    #[tokio::test]
    async fn unreachable_registry_maps_to_20802() {
        let mock = MockRegistryClient {
            reachable: false,
            ..Default::default()
        };
        let err = collect_images(&mock).await.unwrap_err();
        assert_eq!(err.code, codes::REGISTRY_UNREACHABLE);
    }

    #[tokio::test]
    async fn untag_manifest_removes_only_that_tag() {
        let repo = "docker.io/library/alpine";
        let mut mock = MockRegistryClient::new();
        // 两个 tag 指向同一 digest（改名场景的核心风险）
        mock.seed(repo, "3.19", "sha256:same");
        mock.seed(repo, "stable", "sha256:same");

        mock.untag_manifest(repo, "3.19").await.unwrap();

        assert_eq!(mock.list_tags(repo).await.unwrap(), vec!["stable"]);
        assert_eq!(
            mock.untagged.lock().unwrap().clone(),
            vec![(repo.to_string(), "3.19".to_string())]
        );
        // 摘除的 tag 不再可解析，另一个仍可解析（digest 未被删除）
        assert_eq!(
            mock.describe_manifest(repo, "3.19").await.unwrap_err().code,
            codes::MANIFEST_NOT_FOUND
        );
        assert_eq!(
            mock.describe_manifest(repo, "stable").await.unwrap().digest.as_deref(),
            Some("sha256:same")
        );
        // 未登记 digest 的删除不会误报成功
        assert_eq!(
            mock.untag_manifest(repo, "nope").await.unwrap_err().code,
            codes::MANIFEST_NOT_FOUND
        );
    }
}