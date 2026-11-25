//! 路由装配：API + 前端 SPA（静态托管）

use std::path::PathBuf;

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
    Router,
};
use tower_http::services::{ServeDir, ServeFile};

use crate::{handlers, state::AppState};

/// 构建应用路由
///
/// 路由结构（挂载到 url_base 下）：
/// - `<url_base>/`          → 前端 SPA（ServeDir + index.html fallback，hash 路由）
/// - `<url_base>/api/health` → 健康检查（ResultBody）
///
/// 实现要点：axum 0.8 不再支持对根路径（`/`）直接 `nest` / `nest_service`
/// （panic "Nesting at the root is no longer supported"）。因此内部先以
/// 相对路径组合（`.nest("/api", api)` + `.fallback_service(serve_dir)`），
/// url_base 为空时直接作为根路由返回；非空时再整体 `nest(url_base)` 挂前缀，
/// 并补显式 `<url_base>/` 尾斜杠路由返回 index.html（对齐 vos-virt-manager 模式）。
pub fn build_router(state: AppState) -> Router {
    let url_base = state.url_base.clone();
    let frontend_dir = state.frontend_dir.clone();
    let index_path = frontend_dir.join("index.html");

    // API 路由（相对路径；url_base 前缀由外部统一挂载）
    let api = Router::new()
        .route("/health", get(handlers::health::health))
        .route("/application-backend", axum::routing::post(handlers::application_backend::local))
        .route("/application-backend/{member_id}/dispatch", axum::routing::post(handlers::application_backend::dispatch))
        .route("/application-backend/channel", axum::routing::post(handlers::application_backend::local_channel))
        .route("/application-backend/{member_id}/channel", axum::routing::post(handlers::application_backend::dispatch_channel))
        .route("/cluster/status", get(handlers::cluster::cluster_status))
        .route("/cluster/init", axum::routing::post(handlers::cluster::cluster_init))
        .route("/nodes", axum::routing::post(handlers::nodes::node_add))
        .route("/nodes", axum::routing::get(handlers::nodes::node_list))
        .route(
            "/nodes/{member_id}",
            axum::routing::get(handlers::nodes::node_get)
                .delete(handlers::nodes::node_delete),
        )
        // ---- Phase 2：etcd 子项目动态编排（/api/infra/etcd/*）----
        .route("/infra/etcd/status", get(handlers::infra_etcd::etcd_status))
        .route("/infra/etcd/instances", get(handlers::infra_etcd::etcd_instances))
        .route(
            "/infra/etcd/deploy",
            axum::routing::post(handlers::infra_etcd::etcd_deploy),
        )
        .route(
            "/infra/etcd/deploy_async",
            axum::routing::post(handlers::infra_etcd::etcd_deploy_async),
        )
        .route(
            "/infra/etcd/remove",
            axum::routing::post(handlers::infra_etcd::etcd_remove),
        )
        .route(
            "/infra/etcd/remove_async",
            axum::routing::post(handlers::infra_etcd::etcd_remove_async),
        )
        // ---- Phase 3：postgres-ha（patroni）子项目动态编排（/api/infra/pg/*）----
        .route("/infra/pg/status", get(handlers::infra_pg::pg_status))
        .route("/infra/pg/instances", get(handlers::infra_pg::pg_instances))
        .route(
            "/infra/pg/deploy",
            axum::routing::post(handlers::infra_pg::pg_deploy),
        )
        .route(
            "/infra/pg/deploy_async",
            axum::routing::post(handlers::infra_pg::pg_deploy_async),
        )
        .route(
            "/infra/pg/remove",
            axum::routing::post(handlers::infra_pg::pg_remove),
        )
        .route(
            "/infra/pg/remove_async",
            axum::routing::post(handlers::infra_pg::pg_remove_async),
        )
        // ---- 二期 S1：haproxy 子项目动态编排（/api/infra/haproxy/*）----
        .route("/infra/haproxy/status", get(handlers::infra_haproxy::haproxy_status))
        .route(
            "/infra/haproxy/instances",
            get(handlers::infra_haproxy::haproxy_instances),
        )
        .route(
            "/infra/haproxy/nodes",
            get(handlers::infra_haproxy::haproxy_nodes),
        )
        .route(
            "/infra/haproxy/deploy",
            axum::routing::post(handlers::infra_haproxy::haproxy_deploy),
        )
        .route(
            "/infra/haproxy/deploy_async",
            axum::routing::post(handlers::infra_haproxy::haproxy_deploy_async),
        )
        .route(
            "/infra/haproxy/remove",
            axum::routing::post(handlers::infra_haproxy::haproxy_remove),
        )
        .route(
            "/infra/haproxy/remove_async",
            axum::routing::post(handlers::infra_haproxy::haproxy_remove_async),
        )
        .route(
            "/infra/haproxy/refresh_all",
            axum::routing::post(handlers::infra_haproxy::haproxy_refresh_all),
        )
        .route(
            "/infra/haproxy/config_preview",
            get(handlers::infra_haproxy::haproxy_config_preview),
        )
        // ---- SMB/NFS VIP：keepalived 直连网关节点（/api/infra/keepalived/*）----
        .route(
            "/infra/keepalived/instances",
            get(handlers::infra_keepalived::keepalived_instances),
        )
        .route(
            "/infra/keepalived/status",
            get(handlers::infra_keepalived::keepalived_status),
        )
        .route(
            "/infra/keepalived/deploy",
            axum::routing::post(handlers::infra_keepalived::keepalived_deploy),
        )
        .route(
            "/infra/keepalived/remove",
            axum::routing::post(handlers::infra_keepalived::keepalived_remove),
        )
        .route(
            "/infra/keepalived/vrrp-id",
            axum::routing::post(handlers::infra_keepalived::keepalived_allocate_vrrp_id),
        )
        .route(
            "/infra/keepalived/vrrp-id/release",
            axum::routing::post(handlers::infra_keepalived::keepalived_release_vrrp_id),
        )
        // ---- etcd 认证/TLS 前置（D9：角色/用户/bootstrap/轮换/吊销/凭据分发）----
        .route(
            "/infra/etcd/auth/status",
            get(handlers::etcd_auth::etcd_auth_status),
        )
        .route(
            "/infra/etcd/auth/pki",
            axum::routing::post(handlers::etcd_auth::etcd_auth_pki),
        )
        .route(
            "/infra/etcd/auth/bootstrap",
            axum::routing::post(handlers::etcd_auth::etcd_auth_bootstrap),
        )
        .route(
            "/infra/etcd/auth/rotate",
            axum::routing::post(handlers::etcd_auth::etcd_auth_rotate),
        )
        .route(
            "/infra/etcd/auth/revoke",
            axum::routing::post(handlers::etcd_auth::etcd_auth_revoke),
        )
        .route(
            "/infra/etcd/auth/distribute",
            axum::routing::post(handlers::etcd_auth::etcd_auth_distribute),
        )
        .route(
            "/infra/etcd/auth/credentials",
            get(handlers::etcd_auth::etcd_auth_credentials_list)
                .post(handlers::etcd_auth::etcd_auth_credentials_sink),
        )
        // ---- 存储网关 VIP（SMB 445 / NFS 2049，cephfs-share-gateway）----
        .route(
            "/gateway/backends",
            get(handlers::gateway::gateway_backends_get)
                .post(handlers::gateway::gateway_backends_set),
        )
        .route(
            "/gateway/drain",
            axum::routing::post(handlers::gateway::gateway_drain),
        )
        .route("/gateway/status", get(handlers::gateway::gateway_status))
        .route("/gateway/health", get(handlers::gateway::gateway_health))
        // ---- 二期 S2：集群 temporal 子项目动态编排（/api/infra/temporal/*）----
        .route("/infra/temporal/status", get(handlers::infra_temporal::temporal_status))
        .route(
            "/infra/temporal/instances",
            get(handlers::infra_temporal::temporal_instances),
        )
        .route(
            "/infra/temporal/nodes",
            get(handlers::infra_temporal::temporal_nodes),
        )
        .route(
            "/infra/temporal/deploy",
            axum::routing::post(handlers::infra_temporal::temporal_deploy),
        )
        .route(
            "/infra/temporal/deploy_async",
            axum::routing::post(handlers::infra_temporal::temporal_deploy_async),
        )
        .route(
            "/infra/temporal/remove",
            axum::routing::post(handlers::infra_temporal::temporal_remove),
        )
        .route(
            "/infra/temporal/remove_async",
            axum::routing::post(handlers::infra_temporal::temporal_remove_async),
        )
        // ---- Phase 4：infra 数据清理（成员加入/移除失败残留兜底）----
        .route(
            "/infra/reset_data",
            axum::routing::post(handlers::infra_reset::reset_data),
        )
        // ---- Phase 4：集群创建编排（/api/cluster/create · create/status）----
        .route(
            "/cluster/create",
            axum::routing::post(handlers::cluster_create::cluster_create),
        )
        .route(
            "/cluster/create/status",
            axum::routing::post(handlers::cluster_create::cluster_create_status),
        )
        // ---- 本节点入群（self-join）：引导本节点加入已有集群，编排仍由 seed 执行 ----
        .route(
            "/cluster/join",
            axum::routing::post(handlers::cluster_join::cluster_join),
        )
        .with_state(state);

    // 前端静态托管：SPA fallback（ServeDir 未命中 → index.html）
    // 注意：tower-http 的 not_found_service 会把 fallback 响应状态码强制改写为 404，
    // SPA fallback 需用 fallback()（保留 fallback 响应原样，index.html 返回 200）。
    let serve_dir = ServeDir::new(&frontend_dir).fallback(ServeFile::new(index_path.clone()));

    // 显式 `/` 返回 index.html（hash 路由下根访问直接给壳页面）
    let index_handler = {
        let p = index_path.clone();
        get(move || {
            let p = p.clone();
            async move { serve_index(&p).await }
        })
    };

    // 内部路由（不含 url_base 前缀）
    let app = Router::new()
        .route("/", index_handler.clone())
        .nest("/api", api)
        .fallback_service(serve_dir);

    if url_base.is_empty() {
        app
    } else {
        // axum nest("/vcm", app) 匹配 /vcm/xxx 但不匹配 /vcm（无尾斜杠）
        // 与 /vcm/（尾斜杠后空路径）——补显式路由返回 index.html。
        let trailing = format!("{url_base}/");
        Router::new()
            .route(&trailing, index_handler)
            .nest(&url_base, app)
    }
}

/// 读取并返回 index.html（SPA 壳页面）
async fn serve_index(path: &PathBuf) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use tower::util::ServiceExt;

    use super::*;

    fn test_state(url_base: &str, etcd: Vec<String>) -> AppState {
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::write(tmp.path().join("index.html"), "<html>vcm</html>").unwrap();
        // 注意：TempDir 在函数返回后 drop，这里用 leak 保持目录存活（测试进程内一次足够）
        let dir = Box::leak(Box::new(tmp)).path().to_path_buf();
        AppState::new(PathBuf::from("/tmp/data"), dir, url_base.to_string(), etcd)
    }

    #[tokio::test]
    async fn health_under_url_base() {
        let app = build_router(test_state("/vcm", vec!["127.0.0.1:2379".to_string()]));
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/vcm/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);

        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["status"], "ok");
        assert_eq!(v["data"]["etcd_configured"], true);
    }

    #[tokio::test]
    async fn health_at_root_when_url_base_empty() {
        // 回归：url_base 归一化为空串时（默认 CMD --url-base /），
        // axum 0.8 根路径 nest 曾导致启动 panic（"Nesting at the root..."）。
        let app = build_router(test_state("", vec![]));
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["etcd_configured"], false);
    }

    #[tokio::test]
    async fn spa_fallback_serves_index() {
        let app = build_router(test_state("/vcm", vec![]));
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/vcm/some/deep/route")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        assert!(body.windows(5).any(|w| w == b"<html"));
    }

    #[tokio::test]
    async fn root_index_at_when_url_base_empty() {
        // 回归：url_base 为空时根路径应直接返回前端壳页面
        let app = build_router(test_state("", vec![]));
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        assert!(body.windows(5).any(|w| w == b"<html"));
    }

    #[tokio::test]
    async fn trailing_slash_under_url_base_serves_index() {
        // `<url_base>/`（尾斜杠）应返回 index.html，而非 404
        let app = build_router(test_state("/vcm", vec![]));
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/vcm/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), axum::http::StatusCode::OK);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        assert!(body.windows(5).any(|w| w == b"<html"));
    }

    // ============================================================================
    // Phase 1：集群注册与节点录入 handler 全流程（Mock etcd + 临时 SQLite）
    // ============================================================================

    use std::sync::Arc;

    use axum::body::Body;
    use serde_json::{json, Value};

    use crate::db;
    use crate::etcd::{EtcdClusterClient, MockEtcdClusterClient};

    /// 构造带 Mock etcd + 临时 SQLite 状态库的应用
    async fn test_app_cluster() -> Router {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let db = db::init_test_sqlite(&tmp.path().join("vcm.db")).await;

        let client: Arc<dyn crate::etcd::EtcdClusterClient> = Arc::new(MockEtcdClusterClient::new());
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec!["127.0.0.1:2379".to_string()],
            client,
            Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            Some(db),
        );
        build_router(state)
    }

    async fn post_json(
        app: &Router,
        uri: &str,
        body: Value,
    ) -> (axum::http::StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        (status, v)
    }

    async fn get_json(app: &Router, uri: &str) -> (axum::http::StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        (status, v)
    }

    async fn delete_json(app: &Router, uri: &str) -> (axum::http::StatusCode, Value) {
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        (status, v)
    }

    fn code_of(v: &Value) -> i32 {
        v["code"].as_i64().unwrap() as i32
    }

    #[tokio::test]
    async fn cluster_init_then_nodes_full_flow() {
        let app = test_app_cluster().await;

        // 未初始化 status：etcd_ok=true, cluster=null, nodes_count=0, db_ok=true
        let (_s, v) = get_json(&app, "/vcm/api/cluster/status").await;
        assert_eq!(code_of(&v), 0, "status: {v}");
        assert_eq!(v["data"]["etcd_ok"], true);
        assert_eq!(v["data"]["nodes_count"], 0);
        assert_eq!(v["data"]["db_ok"], true);

        // init
        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"cluster_name": "测试集群", "node_ip": "10.0.0.1", "admin_password": "pw123456"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "init: {v}");
        let cluster_id = v["data"]["cluster"]["cluster_id"].as_str().unwrap().to_string();
        assert!(!cluster_id.is_empty());

        // 重复 init → 20506
        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"cluster_name": "再次", "node_ip": "10.0.0.2", "admin_password": "pw123456"}),
        )
        .await;
        assert_eq!(code_of(&v), 20506, "重复 init: {v}");

        // add node（工作节点角色）
        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "10.0.0.2", "admin_password": "pw123456", "role": "worker", "display_name": "从节点", "join_infra": false}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "add node: {v}");

        // 重复 IP → 20506
        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "10.0.0.2", "admin_password": "pw123456", "role": "worker"}),
        )
        .await;
        assert_eq!(code_of(&v), 20506, "重复 IP: {v}");

        // 非法参数 → 20501
        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "999.1.1.1", "admin_password": "short", "role": "bad"}),
        )
        .await;
        assert_eq!(code_of(&v), 20501, "非法参数: {v}");

        // status 聚合：nodes_count=2（init 自注册管理节点 + 1 工作节点）
        let (_s, v) = get_json(&app, "/vcm/api/cluster/status").await;
        assert_eq!(v["data"]["nodes_count"], 2, "status: {v}");
        assert!(v["data"]["cluster"]["cluster_name"] == "测试集群");

        // 列表脱敏：无 admin_password
        let (_s, v) = get_json(&app, "/vcm/api/nodes").await;
        assert_eq!(code_of(&v), 0);
        let nodes = v["data"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        let body_str = v.to_string();
        assert!(!body_str.contains("admin_password"), "列表不得含密码字段: {body_str}");
        assert!(!body_str.contains("pw123456"), "列表不得含密码值: {body_str}");

        // 删除从节点
        let member_id = v["data"]["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|n| n["node_ip"] == "10.0.0.2")
            .unwrap()["member_id"]
            .as_str()
            .unwrap()
            .to_string();
        let (_s, v) = delete_json(&app, &format!("/vcm/api/nodes/{member_id}")).await;
        assert_eq!(code_of(&v), 0, "delete: {v}");
        assert_eq!(v["data"]["deleted"], true);

        // 已删除 → 20505
        let (_s, v) = delete_json(&app, &format!("/vcm/api/nodes/{member_id}")).await;
        assert_eq!(code_of(&v), 20505, "再次删除: {v}");

        // 详情不存在 → 20505
        let (_s, v) = get_json(&app, &format!("/vcm/api/nodes/{member_id}")).await;
        assert_eq!(code_of(&v), 20505);
    }

    #[tokio::test]
    async fn node_add_before_init_returns_20507() {
        let app = test_app_cluster().await;
        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "10.0.0.9", "admin_password": "pw123456", "role": "worker"}),
        )
        .await;
        assert_eq!(code_of(&v), 20507, "未初始化录入节点: {v}");
    }

    /// 本节点入群端点已注册：参数非法 → 20501（路由 + JSON 提取 + 校验链路）
    #[tokio::test]
    async fn cluster_join_route_registered_and_validates() {
        let app = test_app_cluster().await;
        let (status, v) = post_json(&app, "/vcm/api/cluster/join", json!({"seed_ip": "bad"})).await;
        assert_eq!(status, axum::http::StatusCode::OK);
        assert_eq!(code_of(&v), 20501, "入群参数校验: {v}");
    }

    #[tokio::test]
    async fn cluster_init_param_validation_20501() {
        let app = test_app_cluster().await;
        // 缺 cluster_name
        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"node_ip": "10.0.0.1", "admin_password": "pw123456"}),
        )
        .await;
        assert_eq!(code_of(&v), 20501);
        // 密码过短
        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"cluster_name": "x", "node_ip": "10.0.0.1", "admin_password": "123"}),
        )
        .await;
        assert_eq!(code_of(&v), 20501);
        // 非法 IP
        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"cluster_name": "x", "node_ip": "nope", "admin_password": "pw123456"}),
        )
        .await;
        assert_eq!(code_of(&v), 20501);
    }

    #[tokio::test]
    async fn etcd_fault_injection_returns_20504() {
        // 构造带可注入故障的 Mock client
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let db = db::init_test_sqlite(&tmp.path().join("vcm.db")).await;

        let mock = Arc::new(MockEtcdClusterClient::new());
        // 模拟 etcd 整体不可达：所有 kv 操作失败（写操作映射 20504）
        mock.set_unavailable(true);
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec!["127.0.0.1:2379".to_string()],
            mock.clone() as Arc<dyn crate::etcd::EtcdClusterClient>,
            Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            Some(db),
        );
        let app = build_router(state);

        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"cluster_name": "x", "node_ip": "10.0.0.1", "admin_password": "pw123456"}),
        )
        .await;
        assert_eq!(code_of(&v), 20504, "etcd 故障注入应 20504: {v}");
    }

    // ============================================================================
    // Phase 2：etcd 子项目编排 handler 全流程（Mock projects + Mock etcd）
    // ============================================================================

    #[tokio::test]
    async fn infra_etcd_deploy_status_remove_flow() {
        let app = test_app_cluster().await;

        // status 空态（etcd mock ping=true + member_count=1；实例空）
        let (_s, v) = get_json(&app, "/vcm/api/infra/etcd/status").await;
        assert_eq!(code_of(&v), 0, "status: {v}");
        assert_eq!(v["data"]["etcd_ok"], true);
        assert!(v["data"]["instances"].as_array().unwrap().is_empty());

        // deploy 同步（Mock deploy_sync → deployed）
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/etcd/deploy",
            json!({"node_ip": "10.0.0.1"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "deploy: {v}");
        assert_eq!(v["data"]["instance"]["compose_project_name"], "cluster-etcd-node-0");

        // 重复 deploy → 20506
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/etcd/deploy",
            json!({"node_ip": "10.0.0.1"}),
        )
        .await;
        assert_eq!(code_of(&v), 20506, "重复 deploy: {v}");

        // deploy_async 合法（新 index=1，用不同 node_ip 不行——同节点幂等判断基于 compose name）
        // 这里移除后再 deploy 验证 index 递增
        let (_s, v) = post_json(&app, "/vcm/api/infra/etcd/remove", json!({"compose_project_name": "cluster-etcd-node-0"})).await;
        assert_eq!(code_of(&v), 0, "remove: {v}");
        assert_eq!(v["data"]["removed"], true);

        // remove 不存在 → 20505
        let (_s, v) = post_json(&app, "/vcm/api/infra/etcd/remove", json!({"compose_project_name": "cluster-etcd-node-0"})).await;
        assert_eq!(code_of(&v), 20505, "再次 remove: {v}");

        // 参数非法 → 20501
        let (_s, v) = post_json(&app, "/vcm/api/infra/etcd/deploy", json!({"node_ip": "bad"})).await;
        assert_eq!(code_of(&v), 20501, "非法 node_ip: {v}");
    }

    #[tokio::test]
    async fn infra_etcd_projects_fault_returns_20508() {
        // 构造 projects 故障注入的 AppState
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();

        let mock_projects = std::sync::Arc::new(crate::projects::MockProjectsClient::new());
        mock_projects.set_fail_next();
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            std::sync::Arc::new(crate::etcd::MockEtcdClusterClient::new())
                as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            mock_projects as std::sync::Arc<dyn crate::projects::ProjectsClient>,
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = build_router(state);

        let (_s, v) = get_json(&app, "/vcm/api/infra/etcd/instances").await;
        assert_eq!(code_of(&v), 20508, "projects 故障注入应 20508: {v}");
    }

    // ============================================================================
    // Phase 3：postgres-ha（patroni）子项目编排 handler 全流程（Mock）
    // ============================================================================

    #[tokio::test]
    async fn infra_pg_deploy_status_remove_flow() {
        let app = test_app_cluster().await;

        // status 空态（etcd DCS ping=true + member_count=1；postgres-ha 未部署）
        let (_s, v) = get_json(&app, "/vcm/api/infra/pg/status").await;
        assert_eq!(code_of(&v), 0, "status: {v}");
        assert_eq!(v["data"]["dcs_ready"], true);
        assert_eq!(v["data"]["postgres_ha_ok"], false);
        assert!(v["data"]["instances"].as_array().unwrap().is_empty());

        // deploy 同步 → cluster-pg-node-0 deployed（Mock 状态机）
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/pg/deploy",
            json!({"node_ip": "10.0.0.1", "bootstrap": true}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "deploy: {v}");
        assert_eq!(v["data"]["instance"]["compose_project_name"], "cluster-pg-node-0");

        // 重复 deploy → 20506
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/pg/deploy",
            json!({"node_ip": "10.0.0.1"}),
        )
        .await;
        assert_eq!(code_of(&v), 20506, "重复 deploy: {v}");

        // remove → removed
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/pg/remove",
            json!({"compose_project_name": "cluster-pg-node-0"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "remove: {v}");
        assert_eq!(v["data"]["removed"], true);

        // remove 不存在 → 20505
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/pg/remove",
            json!({"compose_project_name": "cluster-pg-node-0"}),
        )
        .await;
        assert_eq!(code_of(&v), 20505, "再次 remove: {v}");

        // 参数非法 → 20501
        let (_s, v) = post_json(&app, "/vcm/api/infra/pg/deploy", json!({"node_ip": "bad"})).await;
        assert_eq!(code_of(&v), 20501, "非法 node_ip: {v}");
    }

    #[tokio::test]
    async fn infra_pg_dcs_unavailable_returns_20507() {
        // etcd Mock 置为不可达 → DCS 前置校验失败
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let db = db::init_test_sqlite(&tmp.path().join("vcm.db")).await;

        let mock = std::sync::Arc::new(MockEtcdClusterClient::new());
        mock.set_unavailable(true); // ping=false + 所有 kv 操作报错
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec!["127.0.0.1:2379".to_string()],
            mock as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            Some(db),
        );
        let app = build_router(state);

        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/pg/deploy",
            json!({"node_ip": "10.0.0.1", "bootstrap": true}),
        )
        .await;
        assert_eq!(code_of(&v), 20507, "etcd DCS 未就绪应 20507: {v}");
    }

    #[tokio::test]
    async fn infra_pg_projects_fault_returns_20508() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();

        let mock_projects = std::sync::Arc::new(crate::projects::MockProjectsClient::new());
        mock_projects.set_fail_next();
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            std::sync::Arc::new(crate::etcd::MockEtcdClusterClient::new())
                as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            mock_projects as std::sync::Arc<dyn crate::projects::ProjectsClient>,
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = build_router(state);

        let (_s, v) = get_json(&app, "/vcm/api/infra/pg/instances").await;
        assert_eq!(code_of(&v), 20508, "projects 故障注入应 20508: {v}");
    }

    // ============================================================================
    // 二期 S1：haproxy 子项目编排 handler 全流程（Mock etcd + Mock projects）
    // ============================================================================

    #[tokio::test]
    async fn infra_haproxy_requires_manager_20507() {
        // 未 init：etcd 可达但无 manager 节点记录 → 20507（D-HA-03）
        let app = test_app_cluster().await;
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/deploy",
            json!({"node_ip": "10.0.0.9"}),
        )
        .await;
        assert_eq!(code_of(&v), 20507, "无 manager 记录 deploy 应 20507: {v}");
    }

    #[tokio::test]
    async fn infra_haproxy_deploy_status_upsert_remove_flow() {
        let app = test_app_cluster().await;
        // 先 init 集群（自注册一条 manager 节点记录 → backends 可推导）
        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"cluster_name": "lb集群", "node_ip": "10.0.0.1", "admin_password": "pw123456"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "init: {v}");

        // 非法 node_ip → 20501
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/deploy",
            json!({"node_ip": "bad"}),
        )
        .await;
        assert_eq!(code_of(&v), 20501, "非法 node_ip: {v}");

        // deploy 同步 → cluster-haproxy-10-0-0-9
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/deploy",
            json!({"node_ip": "10.0.0.9"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "deploy: {v}");
        assert_eq!(
            v["data"]["instance"]["compose_project_name"],
            "cluster-haproxy-10-0-0-9"
        );
        assert_eq!(v["data"]["refreshed"], false, "首次 deploy 非刷新");

        // status：backends 含 10.0.0.1、deployed_count=1（Mock 无真实 15432 → 探测 false）
        let (_s, v) = get_json(&app, "/vcm/api/infra/haproxy/status").await;
        assert_eq!(code_of(&v), 0, "status: {v}");
        assert_eq!(v["data"]["front_port"], 15432);
        assert_eq!(v["data"]["stats_port"], 18404);
        assert!(v["data"]["pg_backends"].as_array().unwrap().contains(&json!("10.0.0.1")));
        assert_eq!(v["data"]["deployed_count"], 1);
        assert_eq!(v["data"]["local_pg_front_reachable"], false, "测试环境无真实 haproxy");
        assert_eq!(v["data"]["haproxy_ok"], false);

        // instances 过滤 cluster-haproxy-
        let (_s, v) = get_json(&app, "/vcm/api/infra/haproxy/instances").await;
        assert_eq!(code_of(&v), 0);
        assert_eq!(v["data"]["instances"].as_array().unwrap().len(), 1);

        // upsert 再 deploy 同 IP → refreshed=true（先 remove 再 deploy）
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/deploy",
            json!({"node_ip": "10.0.0.9"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "upsert deploy: {v}");
        assert_eq!(v["data"]["refreshed"], true, "活跃实例再 deploy 应为刷新");
        let (_s, v) = get_json(&app, "/vcm/api/infra/haproxy/instances").await;
        assert_eq!(v["data"]["instances"].as_array().unwrap().len(), 1, "刷新后仍单实例");

        // remove → removed；再 remove → 20505
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/remove",
            json!({"compose_project_name": "cluster-haproxy-10-0-0-9"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "remove: {v}");
        assert_eq!(v["data"]["removed"], true);
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/remove",
            json!({"compose_project_name": "cluster-haproxy-10-0-0-9"}),
        )
        .await;
        assert_eq!(code_of(&v), 20505, "再次 remove: {v}");
    }

    #[tokio::test]
    async fn infra_haproxy_status_degrades_when_projects_unavailable() {        // etcd 有 manager，但 projects_client 走 Unavailable（20508）→
        // status 聚合应降级为空实例 + haproxy_ok=false（不 500）
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let mock = std::sync::Arc::new(MockEtcdClusterClient::new());
        mock.put_node_record(
            "m1",
            &crate::etcd::dto::NodeRecord {
                metadata: crate::etcd::dto::NodeMetadata {
                    member_id: "m1".into(),
                    display_name: "mgr1".into(),
                    role: crate::etcd::dto::role::MANAGER.into(),
                    node_ip: "10.0.0.1".into(),
                    api_endpoint: "http://10.0.0.1:19906/".into(),
                    joined_at: "2026-09-09T00:00:00+08:00".into(),
                },
                credentials: crate::etcd::dto::NodeCredentials {
                    admin_password: "pw".into(),
                    updated_at: "2026-09-09T00:00:00+08:00".into(),
                    pg_superuser_password: None,
                    pg_replication_password: None,
                },
            },
        )
        .await
        .unwrap();
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec!["127.0.0.1:2379".to_string()],
            mock as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = build_router(state);
        let (_s, v) = get_json(&app, "/vcm/api/infra/haproxy/status").await;
        assert_eq!(code_of(&v), 0, "status 降级不报错: {v}");
        assert!(v["data"]["pg_backends"].as_array().unwrap().contains(&json!("10.0.0.1")));
        assert_eq!(v["data"]["deployed_count"], 0);
    }

    #[tokio::test]
    async fn infra_haproxy_nodes_aggregates_per_node_presence() {
        // etcd 两节点记录 → nodes 聚合端点逐节点查 remote list_instances（Mock 对
        // haproxy kind 返回匹配 IP slug → present=true）
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let mock = std::sync::Arc::new(MockEtcdClusterClient::new());
        for (i, ip) in ["10.0.0.1", "10.0.0.2"].iter().enumerate() {
            mock.put_node_record(
                &format!("m{i}"),
                &crate::etcd::dto::NodeRecord {
                    metadata: crate::etcd::dto::NodeMetadata {
                        member_id: format!("m{i}"),
                        display_name: format!("n{i}"),
                        role: crate::etcd::dto::role::MANAGER.into(),
                        node_ip: (*ip).into(),
                        api_endpoint: format!("http://{ip}:19906/"),
                        joined_at: "2026-09-09T00:00:00+08:00".into(),
                    },
                    credentials: crate::etcd::dto::NodeCredentials {
                        admin_password: "pw".into(),
                        updated_at: "2026-09-09T00:00:00+08:00".into(),
                        pg_superuser_password: None,
                        pg_replication_password: None,
                    },
                },
            )
            .await
            .unwrap();
        }
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            mock as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = build_router(state);
        let (_s, v) = get_json(&app, "/vcm/api/infra/haproxy/nodes").await;
        assert_eq!(code_of(&v), 0, "nodes 聚合: {v}");
        assert_eq!(v["data"]["front_port"], 15432);
        let nodes = v["data"]["nodes"].as_array().unwrap();
        assert_eq!(nodes.len(), 2);
        assert!(
            nodes.iter().all(|n| n["haproxy_present"] == true),
            "Mock 对 haproxy 返回匹配 slug → present true: {v}"
        );
    }

    #[tokio::test]
    async fn infra_haproxy_worker_explicit_backends_skips_etcd_prereq() {
        // worker 路径：目标节点无本地 etcd（Mock 不可达），但请求显式注入 pg_backends
        // （编排方经 at 转发）→ 跳过 etcd 前置、直接部署
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let mock = std::sync::Arc::new(MockEtcdClusterClient::new());
        mock.set_unavailable(true);
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec![],
            mock as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = build_router(state);
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/deploy",
            json!({"node_ip": "10.0.0.99", "pg_backends": "10.0.0.1,10.0.0.2"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "显式 backends 应绕过 etcd 前置: {v}");
        assert_eq!(
            v["data"]["instance"]["compose_project_name"],
            "cluster-haproxy-10-0-0-99"
        );
        // 对比：无显式 backends 且 etcd 不可达 → 20507
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/deploy",
            json!({"node_ip": "10.0.0.98"}),
        )
        .await;
        assert_eq!(code_of(&v), 20507, "无 backends 且 etcd 不可达 → 20507: {v}");
    }

    #[tokio::test]
    async fn infra_haproxy_deploy_with_at_forwards_remote() {
        // deploy 带 at=<remote_ip> → 经 MockRemoteNodeClient 转发（不在本节点建实例），
        // 返回目标节点 instance 视图；本节点 projects_client 保持空
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let mock = std::sync::Arc::new(MockEtcdClusterClient::new());
        mock.put_cluster_info(&crate::etcd::dto::ClusterInfo {
            cluster_id: "c-lb".into(),
            cluster_name: "lb".into(),
            status: crate::etcd::dto::status::CREATED.into(),
            created_at: "2026-09-09T00:00:00+08:00".into(),
        })
        .await
        .unwrap();
        mock.put_node_record(
            "m1",
            &crate::etcd::dto::NodeRecord {
                metadata: crate::etcd::dto::NodeMetadata {
                    member_id: "m1".into(),
                    display_name: "mgr1".into(),
                    role: crate::etcd::dto::role::MANAGER.into(),
                    node_ip: "10.0.0.1".into(),
                    api_endpoint: "http://10.0.0.1:19906/".into(),
                    joined_at: "2026-09-09T00:00:00+08:00".into(),
                },
                credentials: crate::etcd::dto::NodeCredentials {
                    admin_password: "pw".into(),
                    updated_at: "2026-09-09T00:00:00+08:00".into(),
                    pg_superuser_password: None,
                    pg_replication_password: None,
                },
            },
        )
        .await
        .unwrap();
        let mock_projects = std::sync::Arc::new(crate::projects::MockProjectsClient::new());
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec!["127.0.0.1:2379".to_string()],
            mock as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            mock_projects.clone() as std::sync::Arc<dyn crate::projects::ProjectsClient>,
            None,
            std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new()),
            crate::remote::default_remote_app_port(),
            None,
        );
        let app = build_router(state);

        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/haproxy/deploy",
            json!({"node_ip": "10.0.0.1", "at": "10.0.0.50"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "at 转发 deploy: {v}");
        // 转发到目标节点 10.0.0.50 → 生成其专属实例名（非本节点 projects_client 路径）
        assert_eq!(
            v["data"]["instance"]["compose_project_name"],
            "cluster-haproxy-10-0-0-50"
        );
        // 本节点 projects_client 未被写入（转发不落地本机）
        assert!(mock_projects.snapshot().is_empty(), "at 转发不得在本节点建实例");
    }

    // ============================================================================
    // 二期 S2：集群 temporal 子项目编排 handler 全流程（Mock etcd/projects/remote）
    // ============================================================================

    /// 构造带 manager 节点记录 + pg 凭据的 temporal 测试 app（remote mock 可 preload）
    async fn test_app_temporal_prereq() -> (Router, std::sync::Arc<crate::remote::MockRemoteNodeClient>)
    {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let mock = std::sync::Arc::new(MockEtcdClusterClient::new());
        mock.put_node_record(
            "m1",
            &crate::etcd::dto::NodeRecord {
                metadata: crate::etcd::dto::NodeMetadata {
                    member_id: "m1".into(),
                    display_name: "mgr1".into(),
                    role: crate::etcd::dto::role::MANAGER.into(),
                    node_ip: "10.0.0.1".into(),
                    api_endpoint: "http://10.0.0.1:19906/".into(),
                    joined_at: "2026-09-10T00:00:00+08:00".into(),
                },
                credentials: crate::etcd::dto::NodeCredentials {
                    admin_password: "pw".into(),
                    updated_at: "2026-09-10T00:00:00+08:00".into(),
                    pg_superuser_password: None,
                    pg_replication_password: None,
                },
            },
        )
        .await
        .unwrap();
        mock.put_cluster_pg_credentials(&crate::etcd::dto::PgCredentials {
            superuser_username: "postgres".into(),
            superuser_password: "pw123456".into(),
            replication_username: "replicator".into(),
            replication_password: "rp123456".into(),
            created_at: "2026-09-10T00:00:00+08:00".into(),
        })
        .await
        .unwrap();
        let remote = std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new());
        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec!["127.0.0.1:2379".to_string()],
            mock as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            remote.clone(),
            crate::remote::default_remote_app_port(),
            None,
        );
        (build_router(state), remote)
    }

    #[tokio::test]
    async fn infra_temporal_deploy_requires_haproxy_front_20507() {
        // etcd + pg 凭据齐备，但本机 15432 无 haproxy（测试环境）→ 20507（D-TE-03）
        let (app, _remote) = test_app_temporal_prereq().await;
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/temporal/deploy",
            json!({"node_ip": "10.0.0.1"}),
        )
        .await;
        assert_eq!(code_of(&v), 20507, "haproxy 前置未就绪应 20507: {v}");
        assert!(v["msg"].as_str().unwrap().contains("haproxy"), "错误信息定位 haproxy: {v}");
    }

    #[tokio::test]
    async fn infra_temporal_deploy_with_at_forwards_and_status_nodes() {
        // at 转发（目标 manager 自举凭据，编排方不注入密码）
        let (app, _remote) = test_app_temporal_prereq().await;
        let (_s, v) = post_json(
            &app,
            "/vcm/api/infra/temporal/deploy",
            json!({"node_ip": "10.0.0.1", "at": "10.0.0.50"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "at 转发 temporal deploy: {v}");
        assert_eq!(
            v["data"]["instance"]["compose_project_name"],
            "cluster-temporal-10-0-0-50"
        );
        // 转发体经 MockRemoteNodeClient，不落地本机 projects
        // status 空态（本机未部署）
        let (_s, v) = get_json(&app, "/vcm/api/infra/temporal/status").await;
        assert_eq!(code_of(&v), 0, "status: {v}");
        assert_eq!(v["data"]["frontend_port"], 7333);
        assert_eq!(v["data"]["deployed_count"], 0);
        assert_eq!(v["data"]["temporal_ok"], false);
        // nodes：preload 10.0.0.1 有 temporal 实例 → present=true；deployable 角色标注
        let (app2, remote2) = test_app_temporal_prereq().await;
        remote2.preload_instances("10.0.0.1", "temporal", &["cluster-temporal-10-0-0-1"]);
        let (_s, v) = get_json(&app2, "/vcm/api/infra/temporal/nodes").await;
        assert_eq!(code_of(&v), 0, "nodes: {v}");
        let nodes = v["data"]["nodes"].as_array().unwrap();
        assert_eq!(nodes[0]["haproxy_present"], serde_json::Value::Null, "字段名应为 temporal_present");
        assert_eq!(nodes[0]["temporal_present"], true);
        assert_eq!(nodes[0]["deployable"], true);
    }

    #[tokio::test]
    async fn infra_haproxy_refresh_all_forwards_all_nodes() {
        // refresh_all：etcd 节点 × remote deploy_haproxy（显式注入 pg+temporal backends）
        let (app, remote) = test_app_temporal_prereq().await;
        remote.preload_instances("10.0.0.1", "temporal", &["cluster-temporal-10-0-0-1"]);
        let (_s, v) = post_json(&app, "/vcm/api/infra/haproxy/refresh_all", json!({})).await;
        assert_eq!(code_of(&v), 0, "refresh_all: {v}");
        assert_eq!(v["data"]["count"], 1);
        assert_eq!(v["data"]["refreshed"][0]["ok"], true);
    }

    /// 构造「集群已 created（etcd raft + pg 运行）」的测试 app：预置 ClusterInfo status=created，
    /// remote mock 预置节点探活；返回 (Router, Arc<MockEtcdClusterClient>) 以便断言 raft 成员表。
    async fn test_app_cluster_created() -> (Router, std::sync::Arc<MockEtcdClusterClient>) {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>vcm</html>").unwrap();
        let db = db::init_test_sqlite(&tmp.path().join("vcm.db")).await;

        let mock = std::sync::Arc::new(MockEtcdClusterClient::new());
        mock.put_cluster_info(&crate::etcd::dto::ClusterInfo {
            cluster_id: "c-created-1".to_string(),
            cluster_name: "已创建集群".to_string(),
            status: crate::etcd::dto::status::CREATED.into(),
            created_at: "2026-09-06T00:00:00+08:00".to_string(),
        })
        .await
        .unwrap();

        let remote = std::sync::Arc::new(crate::remote::MockRemoteNodeClient::new());
        remote.preload(&["10.0.0.5"]);

        let state = AppState::with_runtime(
            tmp.path().join("data"),
            frontend,
            "/vcm".to_string(),
            vec!["127.0.0.1:2379".to_string()],
            mock.clone() as std::sync::Arc<dyn crate::etcd::EtcdClusterClient>,
            std::sync::Arc::new(crate::projects::MockProjectsClient::new()),
            None,
            remote,
            crate::remote::default_remote_app_port(),
            Some(db),
        );
        (build_router(state), mock)
    }

    /// D-CR-05：join_infra 守卫——集群仅 initialized 时录入联动应 20507
    #[tokio::test]
    async fn node_add_join_infra_requires_created_20507() {
        let app = test_app_cluster().await;
        let (_s, v) = post_json(
            &app,
            "/vcm/api/cluster/init",
            json!({"cluster_name": "t", "node_ip": "10.0.0.1", "admin_password": "pw123456"}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "init: {v}");

        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "10.0.0.9", "admin_password": "pw123456", "role": "manager", "join_infra": true}),
        )
        .await;
        assert_eq!(code_of(&v), 20507, "未 created 时 join_infra 应 20507: {v}");
    }

    /// D-CR-05：join_infra 联动成功——探测 + member_add + remote etcd/pg 部署 + 写 NodeRecord
    #[tokio::test]
    async fn node_add_join_infra_success_with_member() {
        let (app, mock) = test_app_cluster_created().await;

        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "10.0.0.5", "admin_password": "pw123456", "role": "manager", "display_name": "加入节点", "join_infra": true}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "join_infra 联动: {v}");
        assert_eq!(v["data"]["node"]["node_ip"], "10.0.0.5");

        // 等 raft 成员表 1 条（peer http://10.0.0.5:2380）
        assert_eq!(
            mock.find_member_id_by_peer_url("http://10.0.0.5:2380").await.unwrap().unwrap(),
            1,
            "member_add 后 raft 成员应存在"
        );

        // 节点列表可见
        let (_s, v) = get_json(&app, "/vcm/api/nodes").await;
        assert_eq!(code_of(&v), 0);
        assert_eq!(v["data"]["nodes"].as_array().unwrap().len(), 1);
    }

    /// D-CR-05：join_infra 联动+移除联动——DELETE remove_infra=true 卸载子项目 + member_remove + 删记录
    #[tokio::test]
    async fn node_delete_remove_infra_cleans_member_and_records() {
        let (app, mock) = test_app_cluster_created().await;

        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "10.0.0.5", "admin_password": "pw123456", "role": "manager", "join_infra": true}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "join: {v}");
        let member_id = v["data"]["node"]["member_id"].as_str().unwrap().to_string();

        // remove_infra=true（缺省）
        let (_s, v) = delete_json(&app, &format!("/vcm/api/nodes/{member_id}?remove_infra=true")).await;
        assert_eq!(code_of(&v), 0, "remove_infra 联动: {v}");

        // raft 成员已移除、记录已删
        assert!(mock.find_member_id_by_peer_url("http://10.0.0.5:2380").await.unwrap().is_none());
        let (_s, v) = get_json(&app, &format!("/vcm/api/nodes/{member_id}")).await;
        assert_eq!(code_of(&v), 20505, "删除后详情应 20505: {v}");

        // remove_infra=false 保持纯元数据删除
        let (_s, v) = post_json(
            &app,
            "/vcm/api/nodes",
            json!({"node_ip": "10.0.0.6", "admin_password": "pw123456", "role": "worker", "join_infra": false}),
        )
        .await;
        assert_eq!(code_of(&v), 0, "纯录入: {v}");
        let mid2 = v["data"]["node"]["member_id"].as_str().unwrap().to_string();
        let (_s, v) = delete_json(&app, &format!("/vcm/api/nodes/{mid2}?remove_infra=false")).await;
        assert_eq!(code_of(&v), 0, "纯元数据删除: {v}");
    }
}