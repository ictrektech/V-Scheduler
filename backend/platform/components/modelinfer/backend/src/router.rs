//! 路由装配：API + 前端 SPA（静态托管）
//!
//! 内部先以相对路径组合（`.nest("/api", api)` + `.fallback_service(serve_dir)`），
//! url_base 为空时直接作为根路由返回；非空时再整体 `nest(url_base)` 挂前缀，
//! 并补显式 `<url_base>/` 尾斜杠路由返回 index.html（对齐 vos-ceph 模式）。

use std::path::PathBuf;
use std::sync::Arc;

use axum::{
    Router,
    body::Body,
    extract::DefaultBodyLimit,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
};
use tower_http::services::{ServeDir, ServeFile};

use crate::{handlers, state::AppState};

/// 构建应用路由
pub fn build_router(state: AppState) -> Router {
    let url_base = state.url_base.clone();
    let frontend_dir = state.frontend_dir.clone();
    let index_path = frontend_dir.join("index.html");

    let api = Router::new()
        .route("/health", get(handlers::health::health))
        // ---- 节点管理 ----
        .route("/nodes", get(handlers::nodes::list).post(handlers::nodes::create))
        .route(
            "/nodes/{id}",
            get(handlers::nodes::get)
                .put(handlers::nodes::update)
                .delete(handlers::nodes::delete),
        )
        .route("/nodes/{id}/test", post(handlers::nodes::test))
        .route("/nodes/probe", post(handlers::nodes::probe))
        .route("/nodes/{id}/rescan", post(handlers::nodes::rescan))
        // ---- 节点监控（node-exporter 部署 / 状态 / 指标）----
        .route("/nodes/{id}/monitor", get(handlers::monitor::get))
        .route("/nodes/{id}/monitor/deploy", post(handlers::monitor::deploy))
        .route("/nodes/{id}/metrics", get(handlers::monitor::metrics))
        // ---- model-hub 目录 ----
        .route("/model-hub/info", get(handlers::model_hub::info))
        .route("/model-hub/match", get(handlers::model_hub::match_variants))
        .route("/model-hub/tree", get(handlers::model_hub::tree))
// ---- NFS 解析与挂载 ----
        .route("/nfs/info", get(handlers::nfs::info))
        .route("/nfs/mount", post(handlers::nfs::mount))
        .route("/nfs/umount", post(handlers::nfs::umount))
        .route("/nfs/status", get(handlers::nfs::status))
        // ---- 模型广场：来源 / 卡片 / 登记 ----
        .route("/plaza/sources", get(handlers::plaza::list_sources).post(handlers::plaza::create_source))
        .route(
            "/plaza/sources/{id}",
            put(handlers::plaza::update_source).delete(handlers::plaza::delete_source),
        )
        .route("/plaza/sources/{id}/cards", get(handlers::plaza::list_cards))
        .route("/plaza/sources/{id}/manifest", get(handlers::plaza::card_manifest))
        .route("/plaza/models", get(handlers::plaza::list_models).post(handlers::plaza::register_model))
        .route(
            "/plaza/models/upload",
            post(handlers::plaza::upload_local_card)
                .layer(DefaultBodyLimit::max(handlers::plaza::MAX_CARD_PACKAGE_BYTES)),
        )
        .route("/plaza/models/{id}", delete(handlers::plaza::delete_model))
        // ---- 部署配置 ----
        .route("/deployments", get(handlers::deployments::list).post(handlers::deployments::create))
        .route(
            "/deployments/{id}",
            get(handlers::deployments::get)
                .put(handlers::deployments::update)
                .delete(handlers::deployments::delete),
        )
        .route("/deployments/{id}/start", post(handlers::deployments::start))
        .route("/deployments/{id}/stop", post(handlers::deployments::stop))
        .route("/deployments/{id}/metrics", get(handlers::inference::metrics))
        // ---- ComfyUI 实例权重管理 ----
        .route(
            "/deployments/{id}/comfyui/weights",
            get(handlers::comfyui::list_weights).post(handlers::comfyui::add_weight),
        )
        .route(
            "/deployments/{id}/comfyui/weights/{rid}",
            delete(handlers::comfyui::delete_weight),
        )
        // ---- 标准接入配置 + 统一网关 ----
        .route("/deployments/{id}/access", get(handlers::access::get_access))
        .route(
            "/deployments/{id}/vrouter/register",
            post(handlers::access::register_vrouter),
        )
        .route("/vrouter/status", get(handlers::access::vrouter_status))
        // ---- 本地反代（TCP 端口映射到 head vLLM）----
        .route(
            "/deployments/{id}/local-proxy",
            put(handlers::access::update_local_proxy),
        )
        // ---- 运行可观测性 ----
        .route("/deployments/{id}/runs", get(handlers::observability::list_runs))
        .route(
            "/deployments/{id}/runs/{run_id}",
            get(handlers::observability::get_run),
        )
        .route(
            "/deployments/{id}/runs/{run_id}/steps",
            get(handlers::observability::list_steps),
        )
        .route(
            "/deployments/{id}/runs/{run_id}/temporal",
            get(handlers::observability::temporal),
        )
        .route(
            "/deployments/{id}/runs/{run_id}/logs",
            get(handlers::observability::list_logs),
        )
        .route(
            "/deployments/{id}/runs/{run_id}/data",
            get(handlers::observability::list_data),
        )
        .route(
            "/deployments/{id}/containers",
            get(handlers::observability::list_containers),
        )
        .route(
            "/deployments/{id}/nodes/{node_id}/containers/{name}/stream",
            get(handlers::containers::stream_container_logs),
        )
        // ---- 运行时设置 ----
        .route(
            "/settings",
            get(handlers::settings::get).put(handlers::settings::update),
        )
        // 未匹配的 /api/* → 404（不再落到前端 SPA fallback）
        .fallback(|| async { StatusCode::NOT_FOUND });

    // 前端静态托管：SPA fallback（ServeDir 未命中 → index.html，保留 200）
    let serve_dir = ServeDir::new(&frontend_dir).fallback(ServeFile::new(index_path.clone()));

    let index_handler = {
        let p = index_path.clone();
        get(move || {
            let p = p.clone();
            async move { serve_index(&p).await }
        })
    };

    let app = Router::new()
        .route("/", index_handler.clone())
        .nest("/api", api.with_state(Arc::new(state)))
        .fallback_service(serve_dir);

    if url_base.is_empty() {
        app
    } else {
        let trailing = format!("{url_base}/");
        Router::new().route(&trailing, index_handler).nest(&url_base, app)
    }
}

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
    use std::sync::Arc;

    use serde_json::Value;
    use tower::util::ServiceExt;

    use super::*;
    use crate::platform::MockPlatformClient;
    use crate::ssh::MockSshExec;

    async fn test_state(url_base: &str) -> AppState {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>modelinfer</html>").unwrap();
        let dir = Box::leak(Box::new(tmp)).path().join("dist");
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        AppState {
            db,
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: dir,
            url_base: crate::config::normalize_url_base(url_base),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: std::env::temp_dir().join("modelinfer-plaza-test"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
            vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        }
    }

    async fn get_json(app: Router, uri: &str) -> (u16, Value) {
        let res = app
            .oneshot(axum::http::Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .unwrap();
        let status = res.status().as_u16();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(Value::String(String::from_utf8_lossy(&body).into())),
        )
    }

    #[tokio::test]
    async fn health_ok() {
        let app = build_router(test_state("").await);
        let (status, v) = get_json(app, "/api/health").await;
        assert_eq!(status, 200);
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["status"], "ok");
        assert_eq!(v["data"]["db_ready"], true);
    }

    /// 节点被运行配置引用时禁止删除（20712），避免产生悬挂引用
    /// （见 openspec/changes/modelinfer-dangling-node-reference/）
    #[tokio::test]
    async fn node_delete_refused_when_referenced_by_deployment() {
        use axum::extract::{Path, State};
        use sea_orm::{ActiveModelTrait, Set};

        let state = std::sync::Arc::new(test_state("").await);
        let node = crate::nodes::create_node(
            &state.db,
            state.ssh.as_ref(),
            crate::nodes::NodeCreate {
                name: "n1".into(),
                host: "10.0.0.1".into(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "x".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
                hostname: Some("h1".into()),
                socket_ifname: Some("enp1s0".into()),
                ib_hca: Some(String::new()),
                fscache: false,
                node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            },
        )
        .await
        .unwrap();

        // 直插一条 stopped 运行配置，引用该节点
        let now = crate::time::now_iso();
        crate::entities::deployment::ActiveModel {
            id: Set("d-stale".into()),
            name: Set("glm-5.3-flash".into()),
            plaza_model_id: Set("card@1.0.0".into()),
            node_ids_json: Set(serde_json::to_string(&vec![node.id.clone()]).unwrap()),
            head_node_id: Set(node.id.clone()),
            created_at: Set(now.clone()),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&state.db)
        .await
        .unwrap();

        let err = crate::handlers::nodes::delete(State(state.clone()), Path(node.id.clone()))
            .await
            .unwrap_err();
        assert_eq!(err.code, crate::error::codes::NODE_IN_USE);
        assert!(err.msg.contains("glm-5.3-flash"), "msg={}", err.msg);
        // 节点未被删除
        assert!(crate::nodes::find_node(&state.db, &node.id).await.unwrap().is_some());
    }

    /// 运行配置引用已删除节点时：列表可读、access / metrics 降级（不返回 20702）
    #[tokio::test]
    async fn dangling_node_read_endpoints_degrade() {
        use sea_orm::{ActiveModelTrait, Set};

        let state = test_state("").await;
        let now = crate::time::now_iso();
        crate::entities::deployment::ActiveModel {
            id: Set("d-dangling".into()),
            name: Set("glm-5.3-flash".into()),
            plaza_model_id: Set("card@1.0.0".into()),
            node_ids_json: Set(r#"["deleted-node"]"#.into()),
            head_node_id: Set("deleted-node".into()),
            inference_engine: Set("vllm".into()),
            created_at: Set(now.clone()),
            updated_at: Set(now),
            ..Default::default()
        }
        .insert(&state.db)
        .await
        .unwrap();
        let app = build_router(state);

        let (_, v) = get_json(app.clone(), "/api/deployments").await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["data"][0]["missing_node_ids"][0], "deleted-node");
        assert_eq!(v["data"]["data"][0]["nodes"][0]["status"], "missing");

        let (_, v) = get_json(app.clone(), "/api/deployments/d-dangling").await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["missing_node_ids"][0], "deleted-node");

        // 接入配置：host 为空，不报 20702
        let (_, v) = get_json(app.clone(), "/api/deployments/d-dangling/access").await;
        assert_eq!(v["code"], 0, "resp={v}");
        assert_eq!(v["data"]["host"], "");

        // 推理指标：空指标组，不报 20702
        let (_, v) = get_json(app, "/api/deployments/d-dangling/metrics?range=1h").await;
        assert_eq!(v["code"], 0, "resp={v}");
        assert_eq!(v["data"]["supported"], true);
        assert!(v["data"]["instance"].is_null());
        assert_eq!(v["data"]["groups"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn subpath_mount_serves_index_and_api() {
        let app = build_router(test_state("/app/com.ictrek.modelinfer/").await);
        let (status, v) = get_json(app.clone(), "/app/com.ictrek.modelinfer/api/health").await;
        assert_eq!(status, 200);
        assert_eq!(v["code"], 0);

        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/app/com.ictrek.modelinfer/")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 200);
    }

    fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write as _;
        let mut buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            for (name, content) in files {
                let mut header = tar::Header::new_gnu();
                header.set_size(content.len() as u64);
                header.set_mode(0o644);
                header.set_cksum();
                builder.append_data(&mut header, *name, *content).unwrap();
            }
            builder.finish().unwrap();
        }
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
        gz.write_all(&buf).unwrap();
        gz.finish().unwrap()
    }

    fn outer_tar(inner: &[u8]) -> Vec<u8> {
        let mut buf = Vec::new();
        {
            let mut builder = tar::Builder::new(&mut buf);
            let mut header = tar::Header::new_gnu();
            header.set_size(inner.len() as u64);
            header.set_mode(0o644);
            header.set_cksum();
            builder.append_data(&mut header, "model.tar.gz", inner).unwrap();
            builder.finish().unwrap();
        }
        buf
    }

    fn multipart_body(boundary: &str, filename: &str, content: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"file\"; filename=\"{filename}\"\r\n").as_bytes(),
        );
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(content);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        body
    }

    #[tokio::test]
    async fn upload_local_card_package_via_http() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let state = AppState {
            db,
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: frontend.clone(),
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: data_dir.join("plaza"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
                    vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        let inner = tar_gz(&[(
            "manifest.yaml",
            b"id: com.ictrek.modelinfer.test-local\nversion: 1.0.0\nmodel: Qwen/Qwen3-0.6B\n"
                .as_slice(),
        )]);
        let outer = outer_tar(&inner);
        let boundary = "----modelinfer-test-boundary";
        let body = multipart_body(
            boundary,
            "model_com.ictrek.modelinfer.test-local_1.0.0.tar",
            &outer,
        );

        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/plaza/models/upload")
                    .header("content-type", format!("multipart/form-data; boundary={boundary}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["source"], "local");
        assert_eq!(v["data"]["model"], "Qwen/Qwen3-0.6B");
        assert_eq!(v["data"]["id"], "com.ictrek.modelinfer.test-local@1.0.0");

        // 出现在已登记模型列表
        let (_, list) = get_json(app, "/api/plaza/models").await;
        assert_eq!(list["data"]["total"], 1);
    }

    #[tokio::test]
    async fn plaza_cards_are_grouped_by_card_id() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let data_dir = tmp.path().join("data");
        std::fs::create_dir_all(&data_dir).unwrap();
        let db = crate::db::connect("sqlite::memory:").await.unwrap();

        let index = r#"
kind: ModelIndex
models:
  - { id: com.ictrek.a, version: 1.0.0, name: { zh-CN: 甲 }, manifest: meta/a/1.0.0/manifest.yaml, package: models/a/1.0.0/model.tar.gz }
  - { id: com.ictrek.a, version: 1.1.0, name: { zh-CN: 乙 }, manifest: meta/a/1.1.0/manifest.yaml, package: models/a/1.1.0/model.tar.gz }
  - { id: com.ictrek.b, version: 0.1.0, name: { zh-CN: 丙 }, manifest: meta/b/0.1.0/manifest.yaml, package: models/b/0.1.0/model.tar.gz }
"#;
        let state = AppState {
            db,
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new().with_index(index)),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: frontend.clone(),
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: data_dir.join("plaza"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
                    vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        // 建来源
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/plaza/sources")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"s","base_url":"http://h:1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let created: Value = serde_json::from_slice(&body).unwrap();
        let sid = created["data"]["id"].as_str().unwrap().to_string();

        // 卡片列表按 card_id 聚合
        let (_, v) = get_json(app.clone(), &format!("/api/plaza/sources/{sid}/cards")).await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["total"], 2);
        let groups = v["data"]["data"].as_array().unwrap();
        let a = groups.iter().find(|g| g["id"] == "com.ictrek.a").unwrap();
        assert_eq!(a["latest_version"], "1.1.0");
        assert_eq!(a["versions"].as_array().unwrap().len(), 2);
        assert_eq!(a["versions"][0]["version"], "1.1.0");
        assert_eq!(a["versions"][0]["registered"], false);
        assert_eq!(a["name"]["zh-CN"], "乙");
        assert_eq!(a["registered_count"], 0);
        assert!(a["versions"][0]["package"].as_str().unwrap().starts_with("http://h:1/"));

        // 本地模型卡片列表按 model_id 聚合（当前为空）
        let (_, m) = get_json(app, "/api/plaza/models").await;
        assert_eq!(m["code"], 0);
        assert_eq!(m["data"]["total"], 0);
    }

    #[tokio::test]
    async fn nodes_crud_via_http() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let dir = Box::leak(Box::new(tmp)).path().join("dist");
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let state = AppState {
            db,
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: dir,
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: std::env::temp_dir().join("modelinfer-plaza-test"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
                    vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/nodes")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        r#"{"name":"n1","host":"10.0.0.1","ssh_port":22,"ssh_user":"root","ssh_password":"x"}"#,
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["status"], "online");
        assert_eq!(v["data"]["has_password"], true);

        let (_, list) = get_json(app, "/api/nodes").await;
        assert_eq!(list["data"]["total"], 1);
    }

    /// 播种一个运行配置（卡片 + 节点 + 部署配置），返回 (deployment_id, TempDir)
    async fn seed_deployment(db: &sea_orm::DatabaseConnection) -> (String, tempfile::TempDir) {
        use sea_orm::{ActiveModelTrait, Set};
        let now = crate::time::now_iso();
        let manifest = serde_json::json!({
            "id": "com.ictrek.ro-test",
            "version": "1.0.0",
            "model": "org/w",
            "lifecycle": {"start": "./s.sh", "health-probe": "./h.sh"},
        });
        crate::entities::plaza_model::ActiveModel {
            id: Set("com.ictrek.ro-test@1.0.0".into()),
            model_id: Set("com.ictrek.ro-test".into()),
            model: Set("org/w".into()),
            name: Set("ro".into()),
            version: Set("1.0.0".into()),
            source: Set("plaza".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/x".into()),
            package_path: Set("/tmp/x/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
        let node = crate::nodes::create_node(
            db,
            &MockSshExec::new().with_rule("hostname", 0, "h1"),
            crate::nodes::NodeCreate {
                name: "n1".into(),
                host: "10.0.0.1".into(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "x".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
                hostname: Some("h1".into()),
                socket_ifname: Some("eth0".into()),
                ib_hca: Some(String::new()),
                fscache: false,
                node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            },
        )
        .await
        .unwrap()
        .id;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/org/w/current")).unwrap();
        let d = crate::deployments::create_deployment(
            db,
            tmp.path(),
            crate::deployments::DeploymentCreate {
                name: "ro".into(),
                plaza_model_id: "com.ictrek.ro-test@1.0.0".into(),
                hub_source: "hf".into(),
                node_ids: vec![node],
                configs: serde_json::Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap();
        (d.id, tmp)
    }

    #[tokio::test]
    async fn deployment_start_nfs_preflight_rejects_without_side_effects() {
        use sea_orm::EntityTrait;
        use crate::error::codes;
        use crate::platform::{DeviceInfo, NfsShareInfo, VolumeInfo};

        for (scenario, expected) in [
            ("disabled", codes::NFS_SHARE_UNAVAILABLE),
            ("other-volume", codes::NFS_SHARE_UNAVAILABLE),
            ("unmounted", codes::NFS_SHARE_UNAVAILABLE),
            ("platform-error", codes::PLATFORM_CALL_FAILED),
            ("unmapped", codes::NFS_RESOLVE_FAILED),
            ("active", codes::DEPLOYMENT_CONFLICT),
            // 共享通过后才能到达 Temporal 检查；本单测未连接真实 Temporal。
            ("ready", codes::TEMPORAL_FAILED),
        ] {
            let mut state = test_state("").await;
            let (id, hub) = seed_deployment(&state.db).await;
            state.model_hub_dir = hub.path().display().to_string();
            let ssh = Arc::new(MockSshExec::new());
            state.ssh = ssh.clone();
            let mut platform = MockPlatformClient::new();
            platform.app_configs = serde_json::json!({
                "MODELINFER_MODEL_HUB_PATH": "/share/weights/model_hub"
            });
            platform.volumes = vec![VolumeInfo {
                uuid: "weights-volume".into(),
                name: "模型权重盘".into(),
                mount_point: "/share/weights".into(),
                state: "Running".into(),
            }];
            if matches!(scenario, "other-volume" | "unmounted" | "ready") {
                platform.nfs_shares = vec![NfsShareInfo {
                    uuid: "nfs-share".into(),
                    volume_uuid: if scenario == "other-volume" { "other" } else { "weights-volume" }.into(),
                    name: "weights".into(),
                    mount_point: "/share/weights".into(),
                    read_only: true,
                    state: "Running".into(),
                    mounted: scenario != "unmounted",
                }];
            }
            platform.fail = scenario == "platform-error";
            platform.device = DeviceInfo { ip_addr: vec!["10.0.0.9".into()] };
            if scenario == "unmapped" {
                platform.volumes.clear();
            }
            let platform = Arc::new(platform);
            state.platform = platform.clone();
            // 预检拒绝不能取消已有自动重启状态，或清空此前诊断。
            crate::deploy::autorestart::save(
                &state.db,
                &id,
                &crate::deploy::autorestart::RuntimeState {
                    phase: "starting".into(),
                    attempts: 2,
                    last_error: "previous error".into(),
                    ..Default::default()
                },
            ).await.unwrap();
            if scenario == "active" {
                crate::deploy::activities::apply_state(
                    &state.db,
                    &crate::deploy::activities::MarkStateReq {
                        deployment_id: id.clone(),
                        state: Some("running".into()),
                        desired_state: Some("running".into()),
                        health_status: None,
                        last_error: None,
                    },
                ).await.unwrap();
            }
            let db = state.db.clone();
            let before = crate::deployments::get_deployment(&db, &id).await.unwrap();
            let response = build_router(state)
                .oneshot(axum::http::Request::builder()
                    .method("POST")
                    .uri(format!("/api/deployments/{id}/start"))
                    .body(Body::empty()).unwrap())
                .await.unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
            let value: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(value["code"], expected, "{scenario}: {value}");
            assert_eq!(value["category"], "modelinfer");
            assert!(value["data"].is_null());
            if expected == codes::NFS_SHARE_UNAVAILABLE {
                let msg = value["msg"].as_str().unwrap();
                for detail in ["模型权重盘", "weights-volume", "/share/weights/model_hub", "NFS 共享"] {
                    assert!(msg.contains(detail), "{scenario}: {msg}");
                }
            }
            let after = crate::deployments::get_deployment(&db, &id).await.unwrap();
            assert_eq!(after, before, "{scenario}: 预检失败不应改变运行配置");
            assert!(crate::entities::run::Entity::find().all(&db).await.unwrap().is_empty());
            assert!(ssh.recorded_calls().is_empty());
            if scenario == "active" {
                assert!(platform.calls.lock().unwrap().is_empty());
            }
            if scenario == "ready" {
                assert!(platform.calls.lock().unwrap().iter().any(|c| c == "device_info"));
            }
        }
    }

    #[tokio::test]
    async fn resource_preflight_uses_overrides_and_selected_weight_source() {
        use crate::deployments::preflight;
        use crate::images::MockImageHubClient;
        use crate::error::codes;
        let state = test_state("").await;
        let (id, hub_dir) = seed_deployment(&state.db).await;
        let mut d = crate::deployments::get_deployment(&state.db, &id).await.unwrap();
        let ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap();
        let nodes = crate::nodes::load_models(&state.db, Some(&ids)).await.unwrap();
        let ssh = MockSshExec::new().with_rule("uname -m", 0, "aarch64")
            .with_rule("docker image inspect", 1, "");
        let manifest = serde_json::json!({"images": ["reg.example/card:1"]});
        let hub = MockImageHubClient::new().with_existing(&["reg.example/override:2"]);
        d.weight_override_id = "org/override".into();
        d.weight_override_source = "ms".into();
        d.images_override_json = r#"["reg.example/override:2"]"#.into();
        // 另一来源即使已下载，也不能替代所选来源。
        std::fs::create_dir_all(hub_dir.path().join("export/hf/org/override/current")).unwrap();
        let error = preflight::check(&d, &manifest, &nodes, hub_dir.path(), &ssh, &hub, 2)
            .await.unwrap_err();
        assert_eq!(error.code, codes::START_RESOURCES_MISSING);
        assert_eq!(error.data.unwrap(), serde_json::json!({
            "weights": [{"source": "ms", "model_id": "org/override", "overridden": true}],
            "images": [], "images_source": "override",
        }));
        // 默认权重与镜像均缺失，但有效覆盖已就绪，应允许继续启动。
        std::fs::remove_dir_all(hub_dir.path().join("export/hf/org/w")).unwrap();
        std::fs::create_dir_all(hub_dir.path().join("export/ms/org/override/current")).unwrap();
        preflight::check(&d, &manifest, &nodes, hub_dir.path(), &ssh, &hub, 2).await.unwrap();
        assert!(hub.recorded_calls().iter().all(|name| name == "reg.example/override:2"));
        // 清空覆盖后重新检查卡片默认资源，且一次返回所有缺失项。
        d.weight_override_id.clear();
        d.weight_override_source.clear();
        d.images_override_json = "[]".into();
        let error = preflight::check(&d, &manifest, &nodes, hub_dir.path(), &ssh, &hub, 2)
            .await.unwrap_err();
        assert_eq!(error.data.unwrap(), serde_json::json!({
            "weights": [{"source": "hf", "model_id": "org/w", "overridden": false}],
            "images": ["reg.example/card:1"], "images_source": "card",
        }));
        // 无权重、无镜像的卡片不产生虚假的下载要求。
        d.weight_id.clear();
        preflight::check(&d, &serde_json::json!({}), &nodes, hub_dir.path(), &ssh, &hub, 2)
            .await.unwrap();
    }

    #[tokio::test]
    async fn deployment_start_missing_resources_returns_download_list_without_side_effects() {
        use sea_orm::{ActiveModelTrait, EntityTrait, Set};
        use crate::error::codes;
        use crate::platform::{DeviceInfo, NfsShareInfo, VolumeInfo};
        let mut state = test_state("").await;
        let (id, hub_dir) = seed_deployment(&state.db).await;
        state.model_hub_dir = hub_dir.path().display().to_string();
        let mut platform = MockPlatformClient::new();
        platform.app_configs = serde_json::json!({"MODELINFER_MODEL_HUB_PATH": "/share/weights/model_hub"});
        platform.volumes = vec![VolumeInfo {
            uuid: "vol".into(), name: "weights".into(),
            mount_point: "/share/weights".into(), state: "Running".into(),
        }];
        platform.nfs_shares = vec![NfsShareInfo {
            uuid: "share".into(), volume_uuid: "vol".into(), name: "weights".into(),
            mount_point: "/share/weights".into(), read_only: true,
            state: "Running".into(), mounted: true,
        }];
        platform.device = DeviceInfo { ip_addr: vec!["10.0.0.9".into()] };
        state.platform = Arc::new(platform);
        let ssh = Arc::new(MockSshExec::new().with_rule("uname -m", 0, "aarch64")
            .with_rule("docker image inspect", 1, ""));
        state.ssh = ssh.clone();
        // 使用只返回 404 的本地 registry；不接触真实 image-hub。
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        state.image_hub = crate::images::ImageHubConfig::new("127.0.0.1", listener.local_addr().unwrap().port());
        let registry = tokio::spawn(async move {
            axum::serve(listener, Router::new().fallback(|| async { StatusCode::NOT_FOUND })).await.unwrap();
        });
        let current = crate::deployments::get_deployment(&state.db, &id).await.unwrap();
        let mut am: crate::entities::deployment::ActiveModel = current.into();
        am.weight_override_source = Set("ms".into());
        am.weight_override_id = Set("org/missing-override".into());
        am.images_override_json = Set(r#"["reg.example/missing:2", "reg.example/missing:2"]"#.into());
        am.update(&state.db).await.unwrap();
        crate::deploy::autorestart::save(&state.db, &id, &crate::deploy::autorestart::RuntimeState {
            phase: "starting".into(), attempts: 2, last_error: "previous error".into(), ..Default::default()
        }).await.unwrap();
        let db = state.db.clone();
        let before = crate::deployments::get_deployment(&db, &id).await.unwrap();
        let response = build_router(state).oneshot(axum::http::Request::builder()
            .method("POST").uri(format!("/api/deployments/{id}/start"))
            .body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = axum::body::to_bytes(response.into_body(), 64 * 1024).await.unwrap();
        let value: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["code"], codes::START_RESOURCES_MISSING, "{value}");
        assert_eq!(value["data"], serde_json::json!({
            "weights": [{"source": "ms", "model_id": "org/missing-override", "overridden": true}],
            "images": ["reg.example/missing:2"], "images_source": "override",
        }));
        assert_eq!(crate::deployments::get_deployment(&db, &id).await.unwrap(), before);
        assert!(crate::entities::run::Entity::find().all(&db).await.unwrap().is_empty());
        assert!(ssh.recorded_calls().iter().all(|c| c.contains("uname -m") || c.contains("docker image inspect")));
        registry.abort();
    }

    #[tokio::test]
    async fn deployment_auto_restart_fields_and_validation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let (dep_id, hub) = seed_deployment(&db).await;

        let state = AppState {
            db: db.clone(),
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: frontend,
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: hub.path().to_string_lossy().to_string(),
            plaza_dir: tempfile::TempDir::new().unwrap().path().join("plaza"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
            vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        // 默认：关闭 + 300s + 卡片声明了 health-probe
        let (_, v) = get_json(app.clone(), &format!("/api/deployments/{dep_id}")).await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["auto_restart_enabled"], false);
        assert_eq!(v["data"]["auto_restart_fail_secs"], 300);
        assert_eq!(v["data"]["health_probe_declared"], true);
        assert_eq!(v["data"]["auto_restart_attempts"], 0);

        // 预置运行态，验证关闭开关时是否清零
        crate::deploy::autorestart::save(
            &db,
            &dep_id,
            &crate::deploy::autorestart::RuntimeState {
                phase: crate::deploy::autorestart::PHASE_STOPPING.into(),
                failed_since: "2026-10-21T12:00:00+08:00".into(),
                next_at: String::new(),
                attempts: 2,
                last_at: "2026-10-21T12:01:00+08:00".into(),
                last_error: "x".into(),
            },
        )
        .await
        .unwrap();

        // 开启 + 自定义阈值
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/deployments/{dep_id}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"auto_restart_enabled":true,"auto_restart_fail_secs":120}"#))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], 0, "body={v}");
        assert_eq!(v["data"]["auto_restart_enabled"], true);
        assert_eq!(v["data"]["auto_restart_fail_secs"], 120);

        // 非法阈值 → 20748
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/deployments/{dep_id}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"auto_restart_fail_secs":10}"#))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], 20748, "body={v}");

        // 关闭开关 → 运行态清零
        let req = axum::http::Request::builder()
            .method("PUT")
            .uri(format!("/api/deployments/{dep_id}"))
            .header("content-type", "application/json")
            .body(Body::from(r#"{"auto_restart_enabled":false}"#))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["code"], 0, "body={v}");
        assert_eq!(v["data"]["auto_restart_enabled"], false);
        assert_eq!(v["data"]["auto_restart_phase"], "");
        assert_eq!(v["data"]["auto_restart_attempts"], 0);
        assert_eq!(v["data"]["auto_restart_failed_since"], "");
    }

    #[tokio::test]
    async fn observability_endpoints_and_settings() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let (dep_id, _hub) = seed_deployment(&db).await;
        // 创建一条 run + 日志
        let run_id = crate::observability::store::create_run(
            &db,
            &dep_id,
            crate::observability::kind::START,
            "manual",
            "准备",
            "",
        )
        .await
        .unwrap();
        let out = crate::ssh::SshOutput {
            exit_code: 0,
            stdout: "modelinfer:rank=0\n".into(),
            stderr: String::new(),
        };
        let node_ref = crate::observability::NodeRef::new("n1", "n1", "10.0.0.1");
        let req = crate::observability::script_req(
            &run_id,
            &dep_id,
            crate::observability::kind::START,
            "start",
            &node_ref,
            1,
            &out,
            "t0",
            "t1",
        );
        let mut stage = crate::observability::StageGlobal::new();
        crate::observability::record_script(&db, &req, &mut stage).await.unwrap();

        // 一条运行步骤
        let step_id = crate::observability::store::start_step(
            &db,
            &crate::observability::StepStartReq {
                run_id: &run_id,
                deployment_id: &dep_id,
                kind: crate::observability::kind::START,
                step: "stage:start",
                stage: "start",
                title: "执行阶段 start",
                orchestrator: crate::observability::orchestrator::TEMPORAL,
                node_id: "",
                node_name: "",
                node_host: "",
            },
        )
        .await
        .unwrap();
        crate::observability::store::finish_step(
            &db,
            &step_id,
            crate::observability::step_state::SUCCEEDED,
            Some(&serde_json::json!({ "nodes": 1 })),
            "",
        )
        .await
        .unwrap();

        let state = AppState {
            db,
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: frontend.clone(),
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: tempfile::TempDir::new().unwrap().path().join("plaza"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
                    vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        let (_, runs) = get_json(app.clone(), &format!("/api/deployments/{dep_id}/runs")).await;
        assert_eq!(runs["code"], 0);
        assert_eq!(runs["data"]["total"], 1);
        assert_eq!(runs["data"]["data"][0]["kind"], "start");

        let (_, one) =
            get_json(app.clone(), &format!("/api/deployments/{dep_id}/runs/{run_id}")).await;
        assert_eq!(one["code"], 0);
        assert_eq!(one["data"]["node_data"][0]["data"]["rank"], "0");
        // 详情含运行步骤
        assert_eq!(one["data"]["steps"][0]["step"], "stage:start");
        assert_eq!(one["data"]["steps"][0]["state"], "succeeded");

        // 步骤列表端点
        let (_, steps) =
            get_json(app.clone(), &format!("/api/deployments/{dep_id}/runs/{run_id}/steps")).await;
        assert_eq!(steps["code"], 0);
        assert_eq!(steps["data"]["total"], 1);
        assert_eq!(steps["data"]["data"][0]["stage"], "start");

        // Temporal 按需查询：该 run 无 workflow_id → 20721
        let (_, tq) = get_json(
            app.clone(),
            &format!("/api/deployments/{dep_id}/runs/{run_id}/temporal"),
        )
        .await;
        assert_eq!(tq["code"], 20721);

        // 部署事件端点已移除 → 404
        let (ev_status, _) =
            get_json(app.clone(), &format!("/api/deployments/{dep_id}/events")).await;
        assert_eq!(ev_status, 404);

        let (_, logs) = get_json(
            app.clone(),
            &format!("/api/deployments/{dep_id}/runs/{run_id}/logs"),
        )
        .await;
        assert_eq!(logs["code"], 0);
        assert_eq!(logs["data"]["total"], 1);

        // 跨配置访问 → 20702
        let (_, bad) = get_json(app.clone(), &format!("/api/deployments/other/runs/{run_id}")).await;
        assert_eq!(bad["code"], 20702);

        // 容器清单
        let (_, ctn) = get_json(app.clone(), &format!("/api/deployments/{dep_id}/containers")).await;
        assert_eq!(ctn["code"], 0);
        assert_eq!(ctn["data"]["total"], 0);

        // 设置：默认 → 更新 → 读取
        let (_, s0) = get_json(app.clone(), "/api/settings").await;
        assert_eq!(s0["data"]["health_history_keep"], 10);
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri("/api/settings")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"health_history_keep":5,"run_history_keep":30}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let saved: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(saved["code"], 0);
        assert_eq!(saved["data"]["health_history_keep"], 5);
        let (_, s1) = get_json(app, "/api/settings").await;
        assert_eq!(s1["data"]["health_history_keep"], 5);
        assert_eq!(s1["data"]["run_history_keep"], 30);
    }

    #[tokio::test]
    async fn container_log_stream_sse() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let (dep_id, _hub) = seed_deployment(&db).await;
        let node_id: String = serde_json::from_str::<Vec<String>>(
            &crate::deployments::get_deployment(&db, &dep_id)
                .await
                .unwrap()
                .node_ids_json,
        )
        .unwrap()[0]
            .clone();
        // 记录容器清单
        let spec = crate::observability::ContainerSpec {
            name: "test-ctn".into(),
            image: "img".into(),
        };
        crate::observability::store::upsert_containers(
            &db,
            &dep_id,
            "run-1",
            "start",
            &node_id,
            "n1",
            "10.0.0.1",
            std::slice::from_ref(&spec),
        )
        .await
        .unwrap();

        let ssh = Arc::new(MockSshExec::new().with_stream_chunks(vec!["line1\n", "line2\n"]));
        let state = AppState {
            db,
            ssh: ssh.clone(),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: frontend.clone(),
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: tempfile::TempDir::new().unwrap().path().join("plaza"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
                    vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        // 不存在的容器 → 20702
        let (_, bad) = get_json(
            app.clone(),
            &format!("/api/deployments/{dep_id}/nodes/{node_id}/containers/nope/stream"),
        )
        .await;
        assert_eq!(bad["code"], 20702);

        // 正常流式：SSE 包含分块内容
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(format!(
                        "/api/deployments/{dep_id}/nodes/{node_id}/containers/test-ctn/stream?tail=30"
                    ))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status().as_u16(), 200);
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("line1"), "SSE 应包含首块: {text}");
        assert!(text.contains("line2"), "SSE 应包含次块: {text}");
        let calls = ssh.recorded_stream_calls();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].contains("docker logs -f --tail 30 'test-ctn'"));
    }

    #[tokio::test]
    async fn access_endpoint_merges_script_serve_and_vrouter_status() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let (dep_id, _hub) = seed_deployment(&db).await;

        // 默认：平台推断（head 10.0.0.1 / 8000 / /v1 / 模型 w）
        let state = AppState {
            db: db.clone(),
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: frontend.clone(),
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: tempfile::TempDir::new().unwrap().path().join("plaza"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
            prometheus: None,
            version: "0.1.0-test",
        };
        let app = build_router(state);

        let (_, a0) = get_json(app.clone(), &format!("/api/deployments/{dep_id}/access")).await;
        assert_eq!(a0["code"], 0);
        assert_eq!(a0["data"]["host"], "10.0.0.1");
        assert_eq!(a0["data"]["port"], 8000);
        assert_eq!(a0["data"]["url_prefix"], "/v1");
        assert_eq!(a0["data"]["model"], "w");
        assert_eq!(a0["data"]["base_url"], "http://10.0.0.1:8000/v1");
        assert_eq!(a0["data"]["web_url"], "http://10.0.0.1:8000");
        assert_eq!(a0["data"]["local_proxy"]["target"], "10.0.0.1:8000");
        assert!(!a0["data"]["api_key"].as_str().unwrap().is_empty());
        assert_eq!(a0["data"]["sources"]["port"], "platform");

        // 写入一次成功启动 run 的 start 全局对象 `serve` → 覆盖 port/ip
        let run_id = crate::observability::store::create_run(
            &db,
            &dep_id,
            crate::observability::kind::START,
            "manual",
            "启动",
            "",
        )
        .await
        .unwrap();
        crate::observability::store::finalize_stage_global(
            &db,
            &run_id,
            &dep_id,
            crate::observability::kind::START,
            "start",
            &serde_json::json!({"serve": {"port": 9000, "ip": "10.0.0.9", "model": "custom"}}),
            1,
        )
        .await
        .unwrap();
        crate::observability::store::finish_run(
            &db,
            &run_id,
            crate::observability::state::SUCCEEDED,
            "ok",
            "",
        )
        .await
        .unwrap();

        let (_, a1) = get_json(app.clone(), &format!("/api/deployments/{dep_id}/access")).await;
        assert_eq!(a1["data"]["port"], 9000);
        assert_eq!(a1["data"]["host"], "10.0.0.9");
        assert_eq!(a1["data"]["model"], "custom");
        assert_eq!(a1["data"]["sources"]["port"], "script");
        assert_eq!(a1["data"]["sources"]["host"], "script");
        assert_eq!(a1["data"]["sources"]["api_key"], "platform");
        // 访问地址（无前缀）随脚本回传端口变化；反代目标用 head 节点 + 有效端口
        assert_eq!(a1["data"]["web_url"], "http://10.0.0.9:9000");
        assert_eq!(a1["data"]["local_proxy"]["target"], "10.0.0.1:9000");
        assert_eq!(a1["data"]["vrouter"]["state"], "unregistered");

        // 统一网关状态端点（不暴露网关地址）
        let (_, s) = get_json(app, "/api/vrouter/status").await;
        assert_eq!(s["code"], 0);
        assert!(s["data"]["url"].is_null(), "状态不应返回网关地址");
        // 平台无 V-Scheduler / 旧 V-Router 应用 → not installed
        assert_eq!(s["data"]["installed"], false);
    }

    /// 列表服务端分页：四端点分页 / 筛选 / 排序 / 统计 + 广场索引缓存
    #[tokio::test]
    async fn list_pagination_filters_and_stats() {
        use sea_orm::{ActiveModelTrait, Set};

        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        let db = crate::db::connect("sqlite::memory:").await.unwrap();

        // 广场来源索引：2 张卡片（a 两版本 / b 一版本）
        let index = r#"
kind: ModelIndex
models:
  - { id: com.ictrek.a, version: 1.0.0, name: { zh-CN: 甲 }, manifest: meta/a/1.0.0/manifest.yaml, package: models/a/1.0.0/m.tar.gz }
  - { id: com.ictrek.a, version: 1.1.0, name: { zh-CN: 甲新 }, manifest: meta/a/1.1.0/manifest.yaml, package: models/a/1.1.0/m.tar.gz }
  - { id: com.ictrek.b, version: 0.1.0, name: { zh-CN: 乙 }, manifest: meta/b/0.1.0/manifest.yaml, package: models/b/0.1.0/m.tar.gz }
"#;
        let state = AppState {
            db: db.clone(),
            ssh: Arc::new(MockSshExec::new().with_rule("hostname", 0, "h")),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new().with_index(index)),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: frontend.clone(),
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: "/modelhub".into(),
            plaza_dir: tmp.path().join("plaza"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
            vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        // ---- 本地模型卡片：2 张（家族 qwen / glm） ----
        for (id, model, name, tag) in [
            ("com.ictrek.alpha@1.0.0", "org/w", "alpha-qwen", "qwen"),
            ("com.ictrek.beta@1.0.0", "org/w", "beta-glm", "glm"),
        ] {
            let now = crate::time::now_iso();
            let manifest = serde_json::json!({
                "id": id.split('@').next().unwrap(),
                "version": "1.0.0",
                "model": model,
                "tags": [tag],
            });
            crate::entities::plaza_model::ActiveModel {
                id: Set(id.into()),
                model_id: Set(id.split('@').next().unwrap().into()),
                model: Set(model.into()),
                name: Set(name.into()),
                version: Set("1.0.0".into()),
                source: Set("plaza".into()),
                source_id: Set(String::new()),
                path: Set("/tmp/x".into()),
                package_path: Set("/tmp/x/model.tar.gz".into()),
                package_sha256: Set(String::new()),
                manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
                state: Set("available".into()),
                size_bytes: Set(10),
                created_at: Set(now.clone()),
                updated_at: Set(now),
            }
            .insert(&db)
            .await
            .unwrap();
        }

        // ---- 2 个节点 + 2 条运行配置 ----
        let mut dep_names = Vec::new();
        for (i, (host, dep)) in [("10.0.0.1", "alpha-run"), ("10.0.0.2", "beta-run")].iter().enumerate() {
            let node = crate::nodes::create_node(
                &db,
                &MockSshExec::new().with_rule("hostname", 0, "h"),
                crate::nodes::NodeCreate {
                    name: format!("node-{i}"),
                    host: (*host).into(),
                    ssh_port: 22,
                    ssh_user: "root".into(),
                    auth_method: "password".into(),
                    ssh_password: "x".into(),
                    ssh_key: String::new(),
                    sudo_password: String::new(),
                    hostname: Some("h".into()),
                    socket_ifname: Some("eth0".into()),
                    ib_hca: Some(String::new()),
                    fscache: i == 0,
                    node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                    nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
                },
            )
            .await
            .unwrap();
            let hub = tempfile::TempDir::new().unwrap();
            std::fs::create_dir_all(hub.path().join("export/hf/org/w/current")).unwrap();
            let model_id = if i == 0 { "com.ictrek.alpha" } else { "com.ictrek.beta" };
            crate::deployments::create_deployment(
                &db,
                hub.path(),
                crate::deployments::DeploymentCreate {
                    name: (*dep).into(),
                    plaza_model_id: format!("{model_id}@1.0.0"),
                    hub_source: "hf".into(),
                    node_ids: vec![node.id],
                    configs: serde_json::Map::new(),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
            dep_names.push(*dep);
        }

        // ---- 节点分页 ----
        let (_, v) = get_json(app.clone(), "/api/nodes?page_size=1").await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["total"], 2);
        assert_eq!(v["data"]["pages"], 2);
        assert_eq!(v["data"]["page_num"], 1);
        assert_eq!(v["data"]["page_size"], 1);
        assert_eq!(v["data"]["data"].as_array().unwrap().len(), 1);
        assert_eq!(v["data"]["stats"]["total"], 2);
        assert_eq!(v["data"]["stats"]["online"], 2);
        assert_eq!(v["data"]["stats"]["fscache"], 1);

        let (_, v2) = get_json(app.clone(), "/api/nodes?page_size=1&page_num=2&sort=name").await;
        assert_eq!(v2["data"]["page_num"], 2);
        assert_eq!(v2["data"]["data"].as_array().unwrap().len(), 1);
        assert_ne!(v["data"]["data"][0]["id"], v2["data"]["data"][0]["id"]);

        let (_, kw) = get_json(app.clone(), "/api/nodes?keyword=10.0.0.2").await;
        assert_eq!(kw["data"]["total"], 1);
        assert_eq!(kw["data"]["stats"]["total"], 2, "stats 应为未筛选全量");
        assert_eq!(kw["data"]["data"][0]["host"], "10.0.0.2");

        // 非法分页参数归一化
        let (_, bad) = get_json(app.clone(), "/api/nodes?page_num=0&page_size=0").await;
        assert_eq!(bad["data"]["page_num"], 1);
        assert_eq!(bad["data"]["page_size"], 20);

        // ---- 运行配置分页 / 筛选 / 排序 ----
        let (_, d1) = get_json(app.clone(), "/api/deployments?page_size=1&sort=name").await;
        assert_eq!(d1["data"]["total"], 2);
        assert_eq!(d1["data"]["pages"], 2);
        assert_eq!(d1["data"]["data"].as_array().unwrap().len(), 1);
        assert_eq!(d1["data"]["data"][0]["name"], dep_names[0]);
        // 家族随视图返回（alpha=qwen / beta=glm）
        assert_eq!(d1["data"]["data"][0]["family"], "qwen");
        assert_eq!(d1["data"]["stats"]["total"], 2);
        assert_eq!(d1["data"]["stats"]["stopped"], 2);

        let (_, d2) = get_json(app.clone(), "/api/deployments?sort=name&page_num=1&page_size=10").await;
        assert_eq!(d2["data"]["data"][1]["family"], "glm");

        let (_, f) =
            get_json(app.clone(), "/api/deployments?card_id=com.ictrek.beta&state=stopped").await;
        assert_eq!(f["data"]["total"], 1);
        assert_eq!(f["data"]["data"][0]["name"], dep_names[1]);

        let (_, no) = get_json(app.clone(), "/api/deployments?keyword=nothing-matches").await;
        assert_eq!(no["data"]["total"], 0);
        assert_eq!(no["data"]["pages"], 1);
        assert_eq!(no["data"]["stats"]["total"], 2);

        // ---- 本地模型卡片分页 / 家族筛选 / 统计 ----
        let (_, m1) = get_json(app.clone(), "/api/plaza/models?page_size=1&sort=name").await;
        assert_eq!(m1["data"]["total"], 2);
        assert_eq!(m1["data"]["pages"], 2);
        assert_eq!(m1["data"]["data"][0]["family"], "qwen");
        assert_eq!(m1["data"]["stats"]["cards"], 2);
        assert_eq!(m1["data"]["stats"]["versions"], 2);
        assert_eq!(m1["data"]["stats"]["size_bytes"], 20);
        assert_eq!(m1["data"]["stats"]["families"], serde_json::json!(["glm", "qwen"]));

        let (_, mf) = get_json(app.clone(), "/api/plaza/models?family=glm").await;
        assert_eq!(mf["data"]["total"], 1);
        assert_eq!(mf["data"]["data"][0]["family"], "glm");
        assert_eq!(mf["data"]["stats"]["cards"], 2, "stats 应为未筛选全量");

        let (_, ms) = get_json(app.clone(), "/api/plaza/models?sort=size").await;
        assert_eq!(ms["data"]["total"], 2);

        // ---- 广场卡片分页 / 关键词 / 仅看已下载 ----
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/plaza/sources")
                    .header("content-type", "application/json")
                    .body(Body::from(r#"{"name":"s","base_url":"http://h:1"}"#))
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let created: Value = serde_json::from_slice(&body).unwrap();
        let sid = created["data"]["id"].as_str().unwrap().to_string();

        let (_, c1) = get_json(
            app.clone(),
            &format!("/api/plaza/sources/{sid}/cards?page_size=1"),
        )
        .await;
        assert_eq!(c1["data"]["total"], 2);
        assert_eq!(c1["data"]["pages"], 2);
        assert_eq!(c1["data"]["data"].as_array().unwrap().len(), 1);
        assert_eq!(c1["data"]["stats"]["cards"], 2);
        assert_eq!(c1["data"]["stats"]["downloaded"], 0);

        let (_, ck) = get_json(
            app.clone(),
            &format!("/api/plaza/sources/{sid}/cards?keyword=%E4%B9%99"),
        )
        .await;
        assert_eq!(ck["data"]["total"], 1);
        assert_eq!(ck["data"]["data"][0]["id"], "com.ictrek.b");

        let (_, cr) = get_json(
            app.clone(),
            &format!("/api/plaza/sources/{sid}/cards?registered_only=true"),
        )
        .await;
        assert_eq!(cr["data"]["total"], 0);

        // ---- 广场索引 TTL 缓存：分页请求不刷新 last_synced_at ----
        let (_, s1) = get_json(app.clone(), "/api/plaza/sources").await;
        let synced1 = s1["data"]["data"][0]["last_synced_at"].clone();
        let _ = get_json(
            app.clone(),
            &format!("/api/plaza/sources/{sid}/cards?page_num=2&page_size=1"),
        )
        .await;
        let (_, s2) = get_json(app, "/api/plaza/sources").await;
        assert_eq!(
            synced1, s2["data"]["data"][0]["last_synced_at"],
            "TTL 内翻页不应刷新 last_synced_at"
        );
    }

    /// 运行配置推理指标端点：未识别引擎返回空指标组（不报错）；区间非法 / 不存在正确报错
    #[tokio::test]
    async fn deployment_metrics_unsupported_and_errors() {
        let state = test_state("").await;
        let (dep_id, _hub) = seed_deployment(&state.db).await;
        let app = build_router(state);

        let (_, v) = get_json(
            app.clone(),
            &format!("/api/deployments/{dep_id}/metrics?range=1h"),
        )
        .await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["supported"], false);
        assert_eq!(v["data"]["active"], false);
        assert_eq!(v["data"]["engine"], "");
        assert!(v["data"]["groups"].as_array().unwrap().is_empty());
        assert_eq!(v["data"]["range"], "1h");

        let (_, bad) = get_json(
            app.clone(),
            &format!("/api/deployments/{dep_id}/metrics?range=bogus"),
        )
        .await;
        assert_eq!(bad["code"], 20701);

        let (_, nf) = get_json(app, "/api/deployments/nope/metrics").await;
        assert_eq!(nf["code"], 20702);
    }

    /// 已停止的 vLLM 运行配置：仍返回历史指标（`active=false`），供回看（D-MI-SGH-05）
    #[tokio::test]
    async fn deployment_metrics_stopped_returns_history() {
        use sea_orm::{ActiveModelTrait, Set};
        use std::collections::BTreeMap;
        let mut state = test_state("").await;
        let (dep_id, _hub) = seed_deployment(&state.db).await;
        // 置引擎为 vllm 但状态 stopped；prometheus 注入 Mock（up=0，带历史采样点）
        let m = crate::deployments::get_deployment(&state.db, &dep_id).await.unwrap();
        let mut am: crate::entities::deployment::ActiveModel = m.into();
        am.inference_engine = Set("vllm".into());
        am.state = Set("stopped".into());
        am.update(&state.db).await.unwrap();
        let mock = crate::monitor::prometheus::MockPrometheusClient::new()
            .with_series(
                "up{",
                vec![crate::monitor::prometheus::PromSeries {
                    labels: BTreeMap::new(),
                    points: vec![(1_700_000_000.0, 0.0)],
                }],
            )
            .with_series(
                "vllm:num_requests_running",
                vec![crate::monitor::prometheus::PromSeries {
                    labels: BTreeMap::new(),
                    points: vec![(1_700_000_000.0, 1.0), (1_700_000_100.0, 0.0)],
                }],
            );
        state.prometheus = Some(std::sync::Arc::new(mock));

        let app = build_router(state);
        let (_, v) = get_json(app, &format!("/api/deployments/{dep_id}/metrics?range=1h")).await;
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(v["data"]["supported"], true);
        assert_eq!(v["data"]["active"], false, "停止的部署不应活跃");
        assert!(
            !v["data"]["groups"].as_array().unwrap().is_empty(),
            "停止的部署仍应返回历史指标组"
        );
        assert_eq!(v["data"]["last_sample"], 1_700_000_100i64);
        assert!(v["data"]["instance"].as_str().unwrap().ends_with(":8000"));
    }

    /// SGLang 运行配置：使用 SGLang 指标目录（`sglang_*`）与 SGLang job 的 up 判定
    #[tokio::test]
    async fn deployment_metrics_sglang_uses_sglang_catalog() {
        use sea_orm::{ActiveModelTrait, Set};
        use std::collections::BTreeMap;
        let mut state = test_state("").await;
        let (dep_id, _hub) = seed_deployment(&state.db).await;
        let m = crate::deployments::get_deployment(&state.db, &dep_id).await.unwrap();
        let mut am: crate::entities::deployment::ActiveModel = m.into();
        am.inference_engine = Set("sglang".into());
        am.state = Set("running".into());
        am.serve_port = Set(30000);
        am.update(&state.db).await.unwrap();
        let mock = crate::monitor::prometheus::MockPrometheusClient::new()
            .with_series(
                "up{",
                vec![crate::monitor::prometheus::PromSeries {
                    labels: BTreeMap::new(),
                    points: vec![(1_700_000_000.0, 1.0)],
                }],
            )
            .with_series(
                "sglang:num_running_reqs",
                vec![crate::monitor::prometheus::PromSeries {
                    labels: BTreeMap::new(),
                    points: vec![(1_700_000_000.0, 3.0)],
                }],
            );
        state.prometheus = Some(std::sync::Arc::new(mock));
        let app = build_router(state);
        let (_, v) = get_json(app, &format!("/api/deployments/{dep_id}/metrics?range=1h")).await;
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(v["data"]["supported"], true);
        assert_eq!(v["data"]["active"], true);
        let groups = v["data"]["groups"].as_array().unwrap();
        let running = groups
            .iter()
            .find(|g| g["key"] == "sglang_running")
            .expect("缺少 sglang_running");
        assert_eq!(running["source"], "modelinfer-sglang");
        assert!(!groups.iter().any(|g| g["key"].as_str().unwrap_or("").starts_with("vllm_")));
    }

    /// ComfyUI 权重管理端点：列举 / 添加 / 删除 + model-hub tree
    #[tokio::test]
    async fn comfyui_weight_endpoints_roundtrip() {
        let mut state = test_state("").await;
        let (dep_id, hub) = seed_deployment(&state.db).await;
        std::fs::write(hub.path().join("export/hf/org/w/current/a.safetensors"), b"123").unwrap();
        state.model_hub_dir = hub.path().display().to_string();
        let app = build_router(state);

        // tree：命中 1 个文件
        let (_, v) = get_json(app.clone(), "/api/model-hub/tree?source=hf&weight_id=org/w").await;
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(v["data"]["total"], 1);

        // 初始台账为空 + 预设类型非空
        let (_, v) = get_json(app.clone(), &format!("/api/deployments/{dep_id}/comfyui/weights")).await;
        assert_eq!(v["code"], 0, "{v}");
        assert!(v["data"]["weights"].as_array().unwrap().is_empty());
        assert!(v["data"]["types"].as_array().unwrap().len() >= 20);

        // 添加
        let body = serde_json::json!({
            "source": "hf", "weight_id": "org/w",
            "file_path": "a.safetensors", "target_type": "checkpoints"
        });
        let req = axum::http::Request::builder()
            .method("POST")
            .uri(format!("/api/deployments/{dep_id}/comfyui/weights"))
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], 0, "{v}");
        let rid = v["data"]["id"].as_str().unwrap().to_string();
        let link = hub
            .path()
            .join("comfyui")
            .join(&dep_id)
            .join("models/checkpoints/a.safetensors");
        assert_eq!(std::fs::read(&link).unwrap(), b"123", "相对软链应可解析");

        // 删除
        let req = axum::http::Request::builder()
            .method("DELETE")
            .uri(format!("/api/deployments/{dep_id}/comfyui/weights/{rid}"))
            .body(Body::empty())
            .unwrap();
        let res = app.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], 0, "{v}");
        assert!(std::fs::symlink_metadata(&link).is_err());

        // tree 缺失变体 → 20743
        let (_, v) = get_json(app, "/api/model-hub/tree?source=hf&weight_id=org/nope").await;
        assert_eq!(v["code"], 20743, "{v}");
    }

    /// 运行配置覆盖（权重 / 镜像 / 额外环境变量）HTTP 端到端
    #[tokio::test]
    async fn deployment_overrides_via_http() {
        use sea_orm::{ActiveModelTrait, Set};

        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "x").unwrap();
        // model-hub：卡片默认权重 + 覆盖权重（ms）
        let hub = tempfile::TempDir::new().unwrap();
        for (src, id) in [("hf", "org/w"), ("ms", "org/other")] {
            std::fs::create_dir_all(hub.path().join("export").join(src).join(id).join("current"))
                .unwrap();
        }
        let db = crate::db::connect("sqlite::memory:").await.unwrap();
        let now = crate::time::now_iso();
        let manifest = serde_json::json!({
            "id": "com.ictrek.ov-test",
            "version": "1.0.0",
            "model": "org/w",
            "images": ["repo/card-only:1"],
            "configs": [{"name":"MAX","type":"integer","default":1}],
            "lifecycle": {"start": "./s.sh"},
        });
        crate::entities::plaza_model::ActiveModel {
            id: Set("com.ictrek.ov-test@1.0.0".into()),
            model_id: Set("com.ictrek.ov-test".into()),
            model: Set("org/w".into()),
            name: Set("ov".into()),
            version: Set("1.0.0".into()),
            source: Set("local".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/x".into()),
            package_path: Set("/tmp/x/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(&db)
        .await
        .unwrap();
        let node = crate::nodes::create_node(
            &db,
            &MockSshExec::new().with_rule("hostname", 0, "h1"),
            crate::nodes::NodeCreate {
                name: "n1".into(),
                host: "10.0.0.1".into(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "x".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
                hostname: Some("h1".into()),
                socket_ifname: Some("eth0".into()),
                ib_hca: Some(String::new()),
                fscache: false,
                node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            },
        )
        .await
        .unwrap()
        .id;

        let dir = Box::leak(Box::new(tmp)).path().join("dist");
        let state = AppState {
            db,
            ssh: Arc::new(MockSshExec::new()),
            image_hub: Default::default(),
            platform: Arc::new(MockPlatformClient::new()),
            plaza: Arc::new(crate::plaza::MockPlazaClient::new()),
            plaza_card_cache: crate::plaza::new_card_cache(),
            frontend_dir: dir,
            url_base: String::new(),
            token_path: None,
            model_hub_path: String::new(),
            model_hub_dir: hub.path().display().to_string(),
            plaza_dir: std::env::temp_dir().join("modelinfer-plaza-test"),
            temporal: None,
            temporal_task_queue: "modelinfer-tasks".into(),
            health_interval_secs: 30,
            settings_defaults: crate::observability::SettingsDefaults::default(),
            default_mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            monitor: Default::default(),
            plaza_server: String::new(),
            prometheus: None,
            version: "0.1.0-test",
            vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
        };
        let app = build_router(state);

        let body = serde_json::json!({
            "name": "ov",
            "plaza_model_id": "com.ictrek.ov-test@1.0.0",
            "hub_source": "hf",
            "node_ids": [node.clone()],
            "configs": {"MAX": 2},
            "weight_override": {"source": "ms", "model_id": "org/other"},
            "images_override": ["repo/override:2", {"name": "repo/arm:3", "arch": "arm64"}],
            "extra_env": {"DEBUG_MODE": 1}
        });
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/deployments")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], 0, "{v}");
        let id = v["data"]["id"].as_str().unwrap().to_string();
        assert_eq!(v["data"]["weight_id"], "org/w");
        assert_eq!(v["data"]["weight_overridden"], true);
        assert_eq!(v["data"]["effective_weight_id"], "org/other");
        assert_eq!(v["data"]["effective_hub_source"], "ms");
        assert_eq!(v["data"]["images_source"], "override");
        assert_eq!(v["data"]["effective_images"].as_array().unwrap().len(), 2);
        assert_eq!(v["data"]["extra_env"]["DEBUG_MODE"], "1");

        // 部分更新未传权重覆盖时，应保留原有覆盖。
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri(format!("/api/deployments/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(serde_json::json!({"name": "ov-renamed"}).to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(v["data"]["weight_overridden"], true);
        assert_eq!(v["data"]["effective_weight_id"], "org/other");
        assert_eq!(v["data"]["effective_hub_source"], "ms");

        // 与编辑页面“恢复卡片默认”一致：显式 null 清空权重覆盖。
        let update = serde_json::json!({
            "weight_override": null,
            "images_override": [],
            "extra_env": {}
        });
        let res = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("PUT")
                    .uri(format!("/api/deployments/{id}"))
                    .header("content-type", "application/json")
                    .body(Body::from(update.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(v["data"]["weight_override"], Value::Null);
        assert_eq!(v["data"]["weight_overridden"], false);
        assert_eq!(v["data"]["effective_weight_id"], "org/w");
        assert_eq!(v["data"]["effective_hub_source"], "hf");
        assert_eq!(v["data"]["images_source"], "card");
        assert_eq!(v["data"]["effective_images"][0], "repo/card-only:1");
        assert!(v["data"]["extra_env"].as_object().unwrap().is_empty());

        // 模拟保存后重新打开：重新查询持久化记录，而非只检查更新响应。
        let (status, reloaded) = get_json(app.clone(), &format!("/api/deployments/{id}")).await;
        assert_eq!(status, 200);
        assert_eq!(reloaded["code"], 0, "{reloaded}");
        assert_eq!(reloaded["data"]["weight_override"], Value::Null);
        assert_eq!(reloaded["data"]["weight_overridden"], false);
        assert_eq!(reloaded["data"]["effective_weight_id"], "org/w");
        assert_eq!(reloaded["data"]["effective_hub_source"], "hf");
        assert_eq!(reloaded["data"]["effective_hub_rel_path"], "export/hf/org/w/current");

        // 负向：保留前缀的额外环境变量 → 20747
        let bad = serde_json::json!({
            "name": "bad",
            "plaza_model_id": "com.ictrek.ov-test@1.0.0",
            "hub_source": "hf",
            "node_ids": [node],
            "extra_env": {"MODELINFER_SERVE_PORT": "9000"}
        });
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/deployments")
                    .header("content-type", "application/json")
                    .body(Body::from(bad.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["code"], 20747, "{v}");
    }
}
