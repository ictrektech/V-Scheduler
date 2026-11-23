//! 镜像源（mirror）域：内置镜像源目录 + 镜像源选择语义的纯函数。
//!
//! - 目录以常量表维护（SSOT），经 `GET /api/registry/mirrors` 暴露给前端
//! - 镜像源**仅作为实际拉取来源**：`<mirror>/<上游 repository>:<tag>`；
//!   推入本地 registry 的目标引用仍由**上游完整镜像名**派生（承接 D-IH-03 名称保留策略，
//!   见 D-IH-32）——例如 `ubuntu:22.04` + `docker.m.daocloud.io` →
//!   源 `docker.m.daocloud.io/library/ubuntu:22.04`、落地 `docker.io/library/ubuntu:22.04`
//! - 路径约定：镜像源仅替换 host，**路径与 tag 原样保留**（与 Docker daemon
//!   `registry-mirrors` 语义一致）
//! - 非法镜像源复用 `20801 PARAM_INVALID`（不新增错误码）

use serde::{Deserialize, Serialize};

use crate::error::ImageHubError;
use crate::settings::{normalize_host, validate_registry_host};
use crate::skopeo::ImageRef;

/// 单个镜像源目录项
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MirrorOption {
    /// 该镜像源对应的上游 registry（前端据此匹配候选）
    pub upstream_registry: String,
    /// 镜像源主机名（可含端口）
    pub host: String,
    /// 展示名（提供方，后端中文名；未知 provider 时前端回退展示）
    pub name: String,
    /// 提供方稳定标识（如 `daocloud` / `1panel` / `1ms` / `xuanyuan` / `dockerproxy`），
    /// 供前端按 `mirror.provider.<id>` 本地化展示名
    pub provider: String,
}

/// 内置镜像源目录（SSOT）：`(上游 registry, 镜像源 host, 展示名, 提供方稳定 id)`。
///
/// 目录变更须同步单测与 `specs/docs/design/image-hub/index.md`。
pub const MIRROR_CATALOG: &[(&str, &str, &str, &str)] = &[
    // docker.io 常见公共加速站
    ("docker.io", "docker.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("docker.io", "docker.1panel.live", "1Panel", "1panel"),
    ("docker.io", "docker.1ms.run", "1ms.run", "1ms"),
    ("docker.io", "docker.xuanyuan.me", "轩辕镜像", "xuanyuan"),
    ("docker.io", "dockerproxy.net", "dockerproxy.net", "dockerproxy"),
    // 各上游 registry 的 DaoCloud 镜像站
    ("docker.elastic.co", "elastic.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("dhi.io", "dhi.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("gcr.io", "gcr.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("ghcr.io", "ghcr.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("k8s.gcr.io", "k8s-gcr.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("registry.k8s.io", "k8s.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("mcr.microsoft.com", "mcr.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("nvcr.io", "nvcr.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    ("quay.io", "quay.m.daocloud.io", "DaoCloud 镜像站", "daocloud"),
    (
        "registry.ollama.ai",
        "ollama.m.daocloud.io",
        "DaoCloud 镜像站",
        "daocloud",
    ),
];

/// 完整镜像源目录（保持常量表顺序）
pub fn mirror_catalog() -> Vec<MirrorOption> {
    MIRROR_CATALOG
        .iter()
        .map(|(upstream, host, name, provider)| MirrorOption {
            upstream_registry: (*upstream).to_string(),
            host: (*host).to_string(),
            name: (*name).to_string(),
            provider: (*provider).to_string(),
        })
        .collect()
}

/// 按上游 registry 匹配候选镜像源（host 归一化后比较；未收录 → 空）
pub fn mirrors_for(registry: &str) -> Vec<MirrorOption> {
    let want = normalize_host(registry);
    mirror_catalog()
        .into_iter()
        .filter(|m| m.upstream_registry == want)
        .collect()
}

/// 归一化并校验镜像源：
/// - `None` / `null` / 空白 → `Ok(None)`（不使用镜像源）
/// - 其余 → `validate_registry_host`（去空白 / 去尾斜杠 / 小写；禁止 `/`），非法 → `20801`
pub fn normalize_mirror(raw: Option<&str>) -> Result<Option<String>, ImageHubError> {
    let Some(value) = raw else {
        return Ok(None);
    };
    if value.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(validate_registry_host(value)?))
}

/// 生效源 registry：提供镜像源时为镜像源 host，否则为上游 registry。
pub fn effective_source_registry(image: &ImageRef, mirror: Option<&str>) -> String {
    match mirror.map(str::trim).filter(|s| !s.is_empty()) {
        Some(m) => normalize_host(m),
        None => image.registry.clone(),
    }
}

/// 实际拉取源引用：`<生效源 registry>/<上游 repository>:<tag>`。
pub fn effective_source_ref(image: &ImageRef, mirror: Option<&str>) -> String {
    format!(
        "{}/{}:{}",
        effective_source_registry(image, mirror),
        image.repository,
        image.tag
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::codes;
    use crate::skopeo::parse_image_ref;

    #[test]
    fn catalog_covers_all_required_upstreams() {
        let groups = |upstream: &str| {
            mirror_catalog()
                .into_iter()
                .filter(|m| m.upstream_registry == upstream)
                .map(|m| m.host)
                .collect::<Vec<_>>()
        };

        let docker = groups("docker.io");
        for host in [
            "docker.m.daocloud.io",
            "docker.1panel.live",
            "docker.1ms.run",
            "docker.xuanyuan.me",
            "dockerproxy.net",
        ] {
            assert!(docker.contains(&host.to_string()), "docker.io 缺少 {host}");
        }
        assert_eq!(docker.len(), 5);

        let expected: [(&str, &str); 10] = [
            ("docker.elastic.co", "elastic.m.daocloud.io"),
            ("dhi.io", "dhi.m.daocloud.io"),
            ("gcr.io", "gcr.m.daocloud.io"),
            ("ghcr.io", "ghcr.m.daocloud.io"),
            ("k8s.gcr.io", "k8s-gcr.m.daocloud.io"),
            ("registry.k8s.io", "k8s.m.daocloud.io"),
            ("mcr.microsoft.com", "mcr.m.daocloud.io"),
            ("nvcr.io", "nvcr.m.daocloud.io"),
            ("quay.io", "quay.m.daocloud.io"),
            ("registry.ollama.ai", "ollama.m.daocloud.io"),
        ];
        for (upstream, host) in expected {
            assert_eq!(
                groups(upstream),
                vec![host.to_string()],
                "{upstream} 的候选镜像源不符"
            );
        }

        // 目录项均含展示名与稳定 provider id，且 host 均为合法 registry host
        for m in mirror_catalog() {
            assert!(!m.name.trim().is_empty(), "缺少展示名: {m:?}");
            assert!(!m.provider.trim().is_empty(), "缺少 provider: {m:?}");
            assert!(validate_registry_host(&m.host).is_ok(), "{m:?}");
        }
        // 同一 provider 的展示名一致
        let mut by_provider = std::collections::BTreeMap::new();
        for m in mirror_catalog() {
            if let Some(prev) = by_provider.insert(m.provider.clone(), m.name.clone()) {
                assert_eq!(prev, m.name, "provider {} 展示名不一致", m.provider);
            }
        }
    }

    #[test]
    fn mirrors_for_only_returns_matching_upstream() {
        let ghcr = mirrors_for("ghcr.io");
        assert_eq!(ghcr.len(), 1);
        assert_eq!(ghcr[0].host, "ghcr.m.daocloud.io");

        // 大小写与空白归一
        let docker = mirrors_for("  Docker.IO ");
        assert_eq!(docker.len(), 5);
        assert!(docker.iter().all(|m| m.upstream_registry == "docker.io"));

        // 未收录上游 → 空
        assert!(mirrors_for("registry.example.com").is_empty());
        assert!(mirrors_for("").is_empty());
    }

    #[test]
    fn normalize_mirror_defaults_and_validation() {
        assert_eq!(normalize_mirror(None).unwrap(), None);
        assert_eq!(normalize_mirror(Some("")).unwrap(), None);
        assert_eq!(normalize_mirror(Some("   ")).unwrap(), None);
        assert_eq!(
            normalize_mirror(Some(" Docker.M.Daocloud.IO/ ")).unwrap(),
            Some("docker.m.daocloud.io".to_string())
        );
        assert_eq!(
            normalize_mirror(Some("harbor.internal:5000")).unwrap(),
            Some("harbor.internal:5000".to_string())
        );

        // 非法（含 scheme / 路径）
        assert_eq!(
            normalize_mirror(Some("http://bad/mirror")).unwrap_err().code,
            codes::PARAM_INVALID
        );
        assert_eq!(
            normalize_mirror(Some("docker.io/foo")).unwrap_err().code,
            codes::PARAM_INVALID
        );
    }

    #[test]
    fn effective_source_ref_keeps_upstream_repository() {
        let image = parse_image_ref("ubuntu:22.04").unwrap();
        assert_eq!(image.registry, "docker.io");
        assert_eq!(
            effective_source_ref(&image, Some("docker.m.daocloud.io")),
            "docker.m.daocloud.io/library/ubuntu:22.04"
        );
        assert_eq!(
            effective_source_ref(&image, None),
            "docker.io/library/ubuntu:22.04"
        );
        assert_eq!(
            effective_source_ref(&image, Some("  ")),
            "docker.io/library/ubuntu:22.04"
        );
        assert_eq!(
            effective_source_registry(&image, Some("DOCKER.m.daocloud.io")),
            "docker.m.daocloud.io"
        );

        // 非 docker.io 上游
        let ghcr = parse_image_ref("ghcr.io/foo/bar:1.0").unwrap();
        assert_eq!(
            effective_source_ref(&ghcr, Some("ghcr.m.daocloud.io")),
            "ghcr.m.daocloud.io/foo/bar:1.0"
        );
        // 落地目标仍为上游完整名（与 mirror 无关）
        assert_eq!(ghcr.dest_ref("127.0.0.1:19913"), "127.0.0.1:19913/ghcr.io/foo/bar:1.0");
    }

    #[test]
    fn image_ref_does_not_carry_mirror() {
        // 名称保留策略：mirror 绝不进入 ImageRef（落地仓库名恒为上游完整名）
        let image = parse_image_ref("ubuntu:22.04").unwrap();
        assert_eq!(image.upstream_repository(), "docker.io/library/ubuntu");
        let _ = effective_source_ref(&image, Some("docker.m.daocloud.io"));
        assert_eq!(image.upstream_repository(), "docker.io/library/ubuntu");
    }
}