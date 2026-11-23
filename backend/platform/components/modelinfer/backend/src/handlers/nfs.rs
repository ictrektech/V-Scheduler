//! NFS 解析与挂载 HTTP handlers

use std::sync::Arc;

use axum::{
    Json,
    extract::{Query, State},
};
use serde::{Deserialize, Serialize};

use crate::error::{ModelinferError, ResultBody};
use crate::nfs::{self, MountNode, NfsResolveResult, NodeMountResult};
use crate::nodes;
use crate::state::AppState;

#[derive(Debug, Deserialize)]
pub struct MountReq {
    /// 目标节点 ID（缺省 = 全部节点）
    #[serde(default)]
    pub node_ids: Option<Vec<String>>,
    /// 挂载点（缺省 = MODELINFER_DEFAULT_MOUNT_DIR）
    #[serde(default)]
    pub mount_dir: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct MountDirQuery {
    #[serde(default)]
    pub mount_dir: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MountResultVo {
    pub results: Vec<NodeMountResult>,
    pub mount_dir: String,
    pub success_count: usize,
    pub fail_count: usize,
}

async fn resolve(state: &AppState, node_hosts: &[String]) -> Result<NfsResolveResult, ModelinferError> {
    let input = nfs::ResolveInput {
        config_name: nfs::MODEL_HUB_CONFIG,
        fallback_hub_path: &state.model_hub_path,
        nfs_server_override: &state.nfs_server,
        node_hosts,
    };
    nfs::resolve_nfs(state.platform.as_ref(), &input).await
}

async fn load_mount_nodes(
    state: &AppState,
    ids: Option<&[String]>,
) -> Result<Vec<MountNode>, ModelinferError> {
    let models = nodes::load_models(&state.db, ids).await?;
    Ok(models
        .into_iter()
        .map(|m| MountNode {
            id: m.id,
            name: m.name,
            host: m.host,
            port: m.ssh_port as u16,
            user: m.ssh_user,
            auth_method: m.auth_method,
            password: m.ssh_password,
            ssh_key: m.ssh_key,
            sudo_password: m.sudo_password,
            fscache: m.fscache,
        })
        .collect())
}

/// `GET /api/nfs/info`
pub async fn info(State(state): State<Arc<AppState>>) -> Result<Json<ResultBody<NfsResolveResult>>, ModelinferError> {
    let nodes = nodes::list_nodes(&state.db).await?;
    let hosts: Vec<String> = nodes.into_iter().map(|n| n.host).collect();
    let resolved = resolve(&state, &hosts).await?;
    Ok(Json(ResultBody::ok(resolved, state.version)))
}

/// `POST /api/nfs/mount`
pub async fn mount(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MountReq>,
) -> Result<Json<ResultBody<MountResultVo>>, ModelinferError> {
    let ids = req.node_ids.as_deref();
    let mount_nodes = load_mount_nodes(&state, ids).await?;
    let hosts: Vec<String> = mount_nodes.iter().map(|n| n.host.clone()).collect();
    let resolved = resolve(&state, &hosts).await?;
    let mount_dir = req
        .mount_dir
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| state.default_mount_dir.clone());
    let results = nfs::mount_on_nodes(state.ssh.as_ref(), &resolved, &mount_nodes, &mount_dir).await?;
    let success_count = results.iter().filter(|r| r.ok).count();
    let fail_count = results.len() - success_count;
    Ok(Json(ResultBody::ok(
        MountResultVo { results, mount_dir, success_count, fail_count },
        state.version,
    )))
}

/// `POST /api/nfs/umount`
pub async fn umount(
    State(state): State<Arc<AppState>>,
    Json(req): Json<MountReq>,
) -> Result<Json<ResultBody<MountResultVo>>, ModelinferError> {
    let ids = req.node_ids.as_deref();
    let mount_nodes = load_mount_nodes(&state, ids).await?;
    let mount_dir = req
        .mount_dir
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| state.default_mount_dir.clone());
    let results = nfs::umount_on_nodes(state.ssh.as_ref(), &mount_nodes, &mount_dir).await?;
    let success_count = results.iter().filter(|r| r.ok).count();
    let fail_count = results.len() - success_count;
    Ok(Json(ResultBody::ok(
        MountResultVo { results, mount_dir, success_count, fail_count },
        state.version,
    )))
}

/// `GET /api/nfs/status`
pub async fn status(
    State(state): State<Arc<AppState>>,
    Query(q): Query<MountDirQuery>,
) -> Result<Json<ResultBody<MountResultVo>>, ModelinferError> {
    let mount_nodes = load_mount_nodes(&state, None).await?;
    let mount_dir = q
        .mount_dir
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| state.default_mount_dir.clone());
    let results = nfs::status_on_nodes(state.ssh.as_ref(), &mount_nodes, &mount_dir).await?;
    let success_count = results.iter().filter(|r| r.ok).count();
    let fail_count = results.len() - success_count;
    Ok(Json(ResultBody::ok(
        MountResultVo { results, mount_dir, success_count, fail_count },
        state.version,
    )))
}
