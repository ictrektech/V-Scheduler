//! 集群 API：状态聚合 / 初始化
//!
//! 契约见 `openspec/changes/cluster-registration-nodes/specs/cluster-registration/spec.md`：
//! - HTTP 200 + ResultBody；参数校验 20501 / 重复初始化 20506 / etcd 故障 20504
//! - etcd 为唯一权威信源；管理面 DB 为补偿记录，失败不阻断（响应带 db_ok 提示）

use axum::extract::State;
use axum::Json;
use chrono::Utc;
use serde_json::{json, Value};

use crate::db;
use crate::error::codes;
use crate::error::{ResultBody, VcmError};
use crate::etcd::dto::{self, ClusterInfo, NodeCredentials, NodeMetadata, NodeRecord};
use crate::state::AppState;

/// 默认节点应用 API 端点（本节点主容器对外地址；可按需覆盖）
pub fn default_api_endpoint(ip: &str) -> String {
    format!("http://{ip}:19906/app/com.ictrek.vos-cluster-manager/")
}

/// RFC3339 当前时间（Asia/Shanghai 约定：UTC 存储，展示层转换）
fn now_rfc3339() -> String {
    Utc::now().to_rfc3339()
}

/// 校验 IPv4 地址
fn validate_ip(s: &str) -> bool {
    s.parse::<std::net::IpAddr>().is_ok()
}

/// 集群状态聚合（业务态返回，etcd 不可达不报错）
pub async fn cluster_status(
    State(state): State<AppState>,
) -> Json<ResultBody<Value>> {
    let version = state.version;
    let client = &state.etcd_cluster_client;

    let etcd_ok = client.ping().await;

    // etcd 不可达 / 未配置时 cluster / infra / nodes 置空（读失败不抛错）
    let (cluster, infra, nodes_count, manager_count, worker_count) = if etcd_ok {
        let cluster = client.get_cluster_info().await.ok().flatten();
        let infra = client.get_infra_status().await.ok().flatten();
        let nodes = client.get_all_node_records().await.ok();
        let nodes_count = nodes.as_ref().map(|v| v.len()).unwrap_or(0);
        // 按角色计数（D4；旧值 master/node 归一化计入 manager/worker）
        let mut manager_count = 0usize;
        let mut worker_count = 0usize;
        if let Some(recs) = nodes.as_ref() {
            for r in recs {
                match dto::role::normalize_legacy(&r.metadata.role).as_str() {
                    dto::role::MANAGER => manager_count += 1,
                    _ => worker_count += 1,
                }
            }
        }
        (cluster, infra, nodes_count, manager_count, worker_count)
    } else {
        (None, None, 0, 0, 0)
    };

    // haproxy 计算视图（二期 S1：仅 etcd 可达时探测；失败/未部署 → null，不阻断聚合）。
    // probe_leader=true：leader 为 503-即回（仅主机不可达时耗 2s/台），概览卡需要展示
    let haproxy_view = if etcd_ok {
        crate::handlers::infra_haproxy::haproxy_status_data(&state, true)
            .await
            .ok()
    } else {
        None
    };
    // temporal 计算视图（二期 S2：本机端口探测，无远程调用，恒快）
    let temporal_view =
        if etcd_ok { Some(crate::handlers::infra_temporal::temporal_status_data(&state).await) } else { None };

    Json(ResultBody::ok(
        json!({
            "etcd_ok": etcd_ok,
            "cluster": cluster,
            "infra": infra,
            "nodes_count": nodes_count,
            "manager_count": manager_count,
            "worker_count": worker_count,
            "haproxy": haproxy_view,
            "temporal": temporal_view,
            "db_ok": state.db_ok(),
        }),
        version,
    ))
}

/// 集群初始化：写 etcd ClusterInfo + 本节点（master）记录 + 管理面注册补偿
pub async fn cluster_init(
    State(state): State<AppState>,
    Json(body): Json<Value>,
) -> Result<Json<ResultBody<Value>>, VcmError> {
    let version = state.version;

    // ---- 参数校验（20501）----
    let cluster_name = body
        .get("cluster_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= 64)
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "cluster_name 必填且长度 ≤64"))?;

    let node_ip = body
        .get("node_ip")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| validate_ip(s))
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "node_ip 必填且为合法 IP 地址"))?;

    let admin_password = body
        .get("admin_password")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty() && s.len() >= 6)
        .ok_or_else(|| VcmError::new(codes::PARAM_INVALID, "admin_password 必填且长度 ≥6"))?;

    let display_name = body
        .get("display_name")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= 64);

    let api_endpoint = body
        .get("api_endpoint")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| default_api_endpoint(&node_ip));

    // ---- 幂等：已初始化 → 20506 ----
    if let Ok(Some(_)) = state.etcd_cluster_client.get_cluster_info().await {
        return Err(VcmError::new(
            codes::EXISTS,
            "集群已初始化（如需重建请先清理 etcd 状态）",
        ));
    }

    // ---- 写 etcd：集群信息 + 本节点（master）记录 ----
    let cluster_id = uuid::Uuid::new_v4().to_string();
    let now = now_rfc3339();
    let member_id = uuid::Uuid::new_v4().to_string();

    let info = ClusterInfo {
        cluster_id: cluster_id.clone(),
        cluster_name: cluster_name.clone(),
        status: dto::status::INITIALIZED.to_string(),
        created_at: now.clone(),
    };

    let self_record = NodeRecord {
        metadata: NodeMetadata {
            member_id: member_id.clone(),
            display_name: display_name.clone().unwrap_or_else(|| node_ip.clone()),
            role: dto::role::MANAGER.to_string(),
            node_ip: node_ip.clone(),
            api_endpoint,
            joined_at: now.clone(),
        },
        credentials: NodeCredentials {
            admin_password,
            updated_at: now,
            pg_superuser_password: None,
            pg_replication_password: None,
        },
    };

    // 写入顺序 + 补偿：info 成功后 node 失败 → 删除 info 回滚（幂等可重试）
    state.etcd_cluster_client.put_cluster_info(&info).await?;
    if let Err(e) = state.etcd_cluster_client.put_node_record(&member_id, &self_record).await {
        let _ = state.etcd_cluster_client.delete_cluster_info().await;
        return Err(e);
    }

    // ---- 管理面补偿记录（db 失败不阻断，仅提示）----
    let db_ok = state.db_ok();
    if let Some(db) = &state.db {
        let _ = db::upsert_cluster_registration(
            db,
            &cluster_id,
            &cluster_name,
            dto::status::INITIALIZED,
            Utc::now().naive_utc(),
        )
        .await;
        let _ = db::insert_operation_log(
            db,
            "cluster_init",
            &cluster_id,
            Some(json!({"cluster_name": cluster_name, "node_ip": node_ip, "db_ok": true})),
        )
        .await;
    }

    tracing::info!(cluster_id = %cluster_id, "集群初始化完成");

    Ok(Json(ResultBody::ok(
        json!({
            "cluster": info,
            "self_member_id": member_id,
            "db_ok": db_ok,
        }),
        version,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_ip_variants() {
        assert!(validate_ip("10.0.0.1"));
        assert!(validate_ip("192.168.1.1"));
        assert!(!validate_ip("256.1.1.1"));
        assert!(!validate_ip("abc"));
        assert!(!validate_ip(""));
    }

    #[test]
    fn default_endpoint_shape() {
        assert_eq!(
            default_api_endpoint("10.0.0.1"),
            "http://10.0.0.1:19906/app/com.ictrek.vos-cluster-manager/"
        );
    }
}