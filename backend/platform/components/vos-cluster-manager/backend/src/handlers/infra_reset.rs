//! 基础设施数据清理（成员加入/移除失败残留兜底）
//!
//! `POST /api/infra/reset_data`：删除本应用在**本节点**的 projects 数据目录下
//! `cluster-etcd-node-*` / `cluster-pg-node-*`（成员编排失败后残留的 etcd/pg
//! 数据——平台 remove 不删卷（Q3），失败实例的容器 Restarting + 污染数据目录
//! 会永久阻塞后续 join；join_infra 反向清理时远端调用本端点）。
//!
//! 目录布局约定（本应用 package 定义，见 projects/*/docker-compose.yml）：
//! 宿主机 `<storage>/apps/<app_id>/data/projects/<compose>`；应用 compose 已把该
//! `projects` 目录挂载为 [`PROJECTS_ROOT`]。
//!
//! ⚠️ 历史坑（2026-10-30 实机）：早期实现只扫 `/share/*/apps/<app>/data/projects`，
//! 而主容器并未挂载 `/share` → `read_dir` 直接失败 → 返回 `removed_dirs: []` 且
//! 调用方打印"已清理"，实为**静默无操作**；残留 etcd WAL 会让再次 join 必然失败。

use axum::extract::State;
use axum::Json;
use serde_json::{json, Value};

use crate::error::{ResultBody, VcmError};
use crate::state::AppState;

/// 子项目数据根（应用 compose 挂载点；宿主机为 `<storage>/apps/<app>/data/projects`）
const PROJECTS_ROOT: &str = "/var/lib/vos-projects";

/// 集群成员子项目目录命名（仅这两类由本应用负责清理）
fn is_cluster_member_dir(name: &str) -> bool {
    name.starts_with("cluster-etcd-node-") || name.starts_with("cluster-pg-node-")
}

/// 删除 `root` 下的集群成员数据目录，返回被删除的目录名（不存在 / 不可读 → 空）
fn reset_projects_under(root: &std::path::Path) -> Vec<String> {
    let mut removed = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return removed;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let is_dir = entry.metadata().map(|m| m.is_dir()).unwrap_or(false);
        if !is_cluster_member_dir(&name) || !is_dir {
            continue;
        }
        match std::fs::remove_dir_all(entry.path()) {
            Ok(()) => removed.push(name),
            Err(e) => tracing::warn!(dir = %entry.path().display(), err = %e, "reset_data 删除失败"),
        }
    }
    removed
}

/// POST /api/infra/reset_data —— 删除本节点集群子项目数据目录（幂等；不存在即成功）
pub async fn reset_data(
    State(state): State<AppState>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;

    let mut removed = reset_projects_under(std::path::Path::new(PROJECTS_ROOT));

    // 兼容旧部署（compose 未挂载数据根时）：按宿主机 /share 布局再扫一遍；
    // 容器内不可见时 `read_dir("/share")` 失败即跳过（不报错）。
    if let Ok(volumes) = std::fs::read_dir("/share") {
        for vol in volumes.flatten() {
            let projects = vol
                .path()
                .join("apps")
                .join(crate::projects::APP_ID)
                .join("data")
                .join("projects");
            removed.extend(reset_projects_under(&projects));
        }
    }

    tracing::info!(
        removed = removed.len(),
        root = PROJECTS_ROOT,
        "infra reset_data 完成"
    );
    Ok(Json(ResultBody::ok(
        json!({"reset": true, "removed_dirs": removed}),
        version,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::projects;

    #[test]
    fn app_id_matches_package() {
        // reset_data 扫描路径依赖应用 id 常量，与平台注入一致
        assert_eq!(projects::APP_ID, "com.ictrek.vos-cluster-manager");
    }

    #[test]
    fn reset_removes_only_cluster_member_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let root = tmp.path();
        for d in [
            "cluster-etcd-node-0",
            "cluster-pg-node-0",
            "cluster-haproxy-192-168-100-232",
            "other-app-data",
        ] {
            std::fs::create_dir_all(root.join(d)).unwrap();
        }
        std::fs::write(root.join("cluster-etcd-node-9"), "not a dir").unwrap();

        let mut removed = reset_projects_under(root);
        removed.sort();
        assert_eq!(removed, vec!["cluster-etcd-node-0", "cluster-pg-node-0"]);
        assert!(!root.join("cluster-etcd-node-0").exists());
        assert!(!root.join("cluster-pg-node-0").exists());
        assert!(root.join("cluster-haproxy-192-168-100-232").exists());
        assert!(root.join("other-app-data").exists());
        assert!(root.join("cluster-etcd-node-9").exists());
    }

    #[test]
    fn reset_is_noop_when_root_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(reset_projects_under(&tmp.path().join("absent")).is_empty());
    }
}