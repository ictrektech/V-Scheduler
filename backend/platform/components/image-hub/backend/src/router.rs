//! 路由装配：API + 前端 SPA（静态托管）

use std::path::PathBuf;

use axum::{
    body::Body,
    extract::DefaultBodyLimit,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Router,
};
use tower_http::services::{ServeDir, ServeFile};

use crate::{handlers, state::AppState};

/// 构建应用路由
///
/// 路由结构（挂载到 url_base 下）：
/// - `<url_base>/`                 → 前端 SPA（ServeDir + index.html fallback，hash 路由）
/// - `<url_base>/image-hub.sh`     → 一键客户端脚本下载（显式路由，避免 SPA fallback）
/// - `<url_base>/api/health`       → 健康检查
/// - `<url_base>/api/registry/*`   → 镜像查询 / 删除 / 详情 / 信息 / 镜像源目录 / 远程拉取 / 同步
///   / 克隆 / 改名 / 归档导入
/// - `<url_base>/api/tasks/*`      → 异步任务查询 / 停止 / 清理
/// - `<url_base>/api/settings/*`   → 上游认证与代理设置
/// - `<url_base>/api/network/*`    → 网卡地址列表
///
/// 实现要点：axum 0.8 不再支持对根路径直接 `nest`；内部以相对路径组合
/// （`.nest("/api", api)` + `.fallback_service(serve_dir)`），url_base 非空时
/// 整体 `nest(url_base)` 并补显式尾斜杠 index 路由。
pub fn build_router(state: AppState) -> Router {
    let url_base = state.url_base.clone();
    let frontend_dir = state.frontend_dir.clone();
    let index_path = frontend_dir.join("index.html");

    let api = Router::new()
        .route("/health", get(handlers::health::health))
        .route("/registry/info", get(handlers::images::registry_info))
        .route(
            "/registry/images",
            get(handlers::images::list_images).delete(handlers::images::remove_image),
        )
        .route("/registry/images/detail", get(handlers::images::image_detail))
        .route("/registry/mirrors", get(handlers::mirror::list_mirrors))
        .route(
            "/registry/gc",
            get(handlers::gc::gc_status).put(handlers::gc::update_gc_config),
        )
        .route("/registry/gc/run", post(handlers::gc::run_gc))
        .route("/registry/pull", post(handlers::pull::pull_image))
        .route("/registry/sync", post(handlers::pull::sync_image))
        .route("/registry/clone", post(handlers::copy::clone_image))
        .route("/registry/rename", post(handlers::copy::rename_image))
        // 归档导入：放开默认请求体大小限制（镜像归档远超 2MiB），流式落盘
        .route(
            "/registry/import",
            post(handlers::import::import_archive).layer(DefaultBodyLimit::disable()),
        )
        .route("/tasks", get(handlers::tasks::list_tasks))
        .route(
            "/tasks/{id}",
            get(handlers::tasks::get_task).delete(handlers::tasks::remove_task),
        )
        .route("/tasks/{id}/cancel", post(handlers::tasks::cancel_task))
        .route("/settings", get(handlers::settings::get_settings))
        .route(
            "/settings/registries",
            put(handlers::settings::upsert_registry_credential)
                .delete(handlers::settings::delete_registry_credential),
        )
        .route("/settings/proxy", put(handlers::settings::update_proxy))
        .route("/network/addresses", get(handlers::network::addresses));

    // 前端静态托管：SPA fallback（ServeDir 未命中 → index.html）。
    // 用 fallback() 而非 not_found_service（后者强制 404，破坏 index.html 200）。
    let serve_dir = ServeDir::new(&frontend_dir).fallback(ServeFile::new(index_path.clone()));

    let index_handler = {
        let p = index_path.clone();
        get(move || {
            let p = p.clone();
            async move { serve_index(&p).await }
        })
    };

    // 一键客户端脚本（显式路由；无状态 handler，避免引入顶层 state 依赖）
    let script_handler = {
        let p = frontend_dir.join("image-hub.sh");
        get(move || {
            let p = p.clone();
            async move { handlers::script::serve_script(&p).await }
        })
    };

    let app = Router::new()
        .route("/", index_handler.clone())
        // 一键客户端脚本：显式路由（先于 fallback_service 命中，避免回落 index.html）
        .route("/image-hub.sh", script_handler)
        .nest("/api", api.with_state(state))
        .fallback_service(serve_dir);

    if url_base.is_empty() {
        app
    } else {
        let trailing = format!("{url_base}/");
        Router::new()
            .route(&trailing, index_handler)
            .nest(&url_base, app)
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
    use crate::registry::MockRegistryClient;
    #[allow(unused_imports)]
    use crate::registry::RegistryClient as _;

    fn test_state(url_base: &str) -> AppState {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
        std::fs::write(
            frontend.join("image-hub.sh"),
            "#!/usr/bin/env bash\necho image-hub\n",
        )
        .unwrap();
        // TempDir 返回即 drop：leak 保活（测试进程生命周期内足够）
        let dir = Box::leak(Box::new(tmp)).path().join("dist");
        AppState::for_test(
            dir,
            Arc::new(MockRegistryClient::new()),
            Arc::new(MockPlatformClient::new()),
        )
        .with_url_base(url_base)
    }

    async fn get_json(app: Router, uri: &str) -> (u16, Value) {
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status().as_u16();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        (
            status,
            serde_json::from_slice(&body).unwrap_or(Value::String(
                String::from_utf8_lossy(&body).to_string(),
            )),
        )
    }

    async fn get_text(app: Router, uri: &str) -> (u16, String) {
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = res.status().as_u16();
        let body = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        (status, String::from_utf8_lossy(&body).to_string())
    }

    async fn send_json(app: Router, method: &str, uri: &str, body: Value) -> Value {
        let res = app
            .oneshot(
                axum::http::Request::builder()
                    .method(method)
                    .uri(uri)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(serde_json::to_vec(&body).unwrap()))
                    .unwrap(),
            )
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    }

    #[tokio::test]
    async fn health_ok_root_mount() {
        let app = build_router(test_state(""));
        let (status, v) = get_json(app, "/api/health").await;
        assert_eq!(status, 200);
        assert_eq!(v["code"], 0);
        assert_eq!(v["category"], "ok");
        assert_eq!(v["data"]["status"], "ok");
        assert_eq!(v["data"]["registry_configured"], true);
        assert_eq!(v["data"]["registry_reachable"], true);
    }

    #[tokio::test]
    async fn registry_info_returns_port() {
        let app = build_router(test_state(""));
        let (_, v) = get_json(app, "/api/registry/info").await;
        assert_eq!(v["code"], 0);
        assert_eq!(v["data"]["registry_port"], 19913);
    }

    #[tokio::test]
    async fn images_list_and_delete_roundtrip() {
        // 预置镜像的 state
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
        let dir = Box::leak(Box::new(tmp)).path().join("dist");
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.io/foo/bar", "1.0", "sha256:a");
        mock.seed("docker.io/foo/bar", "latest", "sha256:b");
        let state = AppState::for_test(
            dir,
            Arc::new(mock),
            Arc::new(MockPlatformClient::new()),
        );
        let app = build_router(state);

        let (_, v) = get_json(app.clone(), "/api/registry/images").await;
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(v["data"]["count"], 1);
        assert_eq!(v["data"]["images"][0]["repository"], "docker.io/foo/bar");
        assert_eq!(v["data"]["images"][0]["tags"].as_array().unwrap().len(), 2);
        // 无标签不再有 deletable 标记（列表中的仓库一律可删）
        assert!(v["data"]["images"][0].get("deletable").is_none());

        let v = send_json(
            app.clone(),
            "DELETE",
            "/api/registry/images",
            serde_json::json!({"repository": "docker.io/foo/bar", "tag": "1.0"}),
        )
        .await;
        assert_eq!(v["code"], 0, "{v}");
        assert_eq!(v["data"]["deleted_tags"][0], "1.0");

        // 非法仓库 → 20801
        let v = send_json(
            app,
            "DELETE",
            "/api/registry/images",
            serde_json::json!({"repository": "Bad/Repo", "tag": "x"}),
        )
        .await;
        assert_eq!(v["code"], 20801, "{v}");
    }

    #[tokio::test]
    async fn network_addresses_platform_failure_degrades() {
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
        let dir = Box::leak(Box::new(tmp)).path().join("dist");
        let platform = MockPlatformClient {
            fail: true,
            ..Default::default()
        };
        let state = AppState::for_test(dir, Arc::new(MockRegistryClient::new()), Arc::new(platform));
        let app = build_router(state);
        let (_, v) = get_json(app, "/api/network/addresses").await;
        assert_eq!(v["code"], 0, "{v}");
        assert!(v["data"]["ip_addr"].as_array().unwrap().is_empty());
    }

    async fn poll_task(app: Router, task_id: &str) -> Value {
    for _ in 0..200 {
        let v = get_json(app.clone(), &format!("/api/tasks/{task_id}")).await.1;
        let status = v["data"]["status"].as_str().unwrap_or("");
        if status == "succeeded" || status == "failed" {
            return v;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    get_json(app, &format!("/api/tasks/{task_id}")).await.1
}

#[tokio::test]
async fn pull_starts_task_and_task_is_queryable() {
    let app = build_router(test_state(""));

    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/pull",
        serde_json::json!({
            "image": "docker.m.daocloud.io/library/alpine:3.20",
            "arch": "all"
        }),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("pull-"), "{task_id}");

    // 列表可见
    let list = get_json(app.clone(), "/api/tasks").await.1;
    assert_eq!(list["code"], 0);
    assert!(list["data"]["count"].as_u64().unwrap() >= 1);

    // 任务终态
    let detail = poll_task(app.clone(), &task_id).await;
    assert_eq!(detail["data"]["status"], "succeeded", "{detail}");
    assert_eq!(detail["data"]["arch"], "all");
    assert_eq!(
        detail["data"]["dest"],
        "127.0.0.1:19913/docker.m.daocloud.io/library/alpine:3.20"
    );

    // 清理终态任务
    let removed = send_json(
        app,
        "DELETE",
        &format!("/api/tasks/{task_id}"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(removed["code"], 0);
}

#[tokio::test]
async fn pull_invalid_image_is_20809() {
        let app = build_router(test_state(""));
        let v = send_json(
            app,
            "POST",
            "/api/registry/pull",
            serde_json::json!({"image": "Bad Image!!"}),
        )
        .await;
        assert_eq!(v["code"], 20809, "{v}");
    }

    #[tokio::test]
    async fn mirrors_catalog_endpoint_lists_options() {
        let app = build_router(test_state(""));
        let v = get_json(app, "/api/registry/mirrors").await.1;
        assert_eq!(v["code"], 0, "{v}");
        let mirrors = v["data"]["mirrors"].as_array().unwrap();
        assert_eq!(mirrors.len(), v["data"]["count"].as_u64().unwrap() as usize);
        assert!(mirrors.len() >= 15, "{mirrors:?}");
        // 每项含 upstream_registry / host / name
        assert!(mirrors.iter().all(|m| {
            m["upstream_registry"].as_str().is_some_and(|s| !s.is_empty())
                && m["host"].as_str().is_some_and(|s| !s.is_empty())
                && m["name"].as_str().is_some_and(|s| !s.is_empty())
        }));
        // docker.io 至少含需求给定的 5 个候选
        let docker: Vec<&str> = mirrors
            .iter()
            .filter(|m| m["upstream_registry"] == "docker.io")
            .map(|m| m["host"].as_str().unwrap())
            .collect();
        for host in [
            "docker.m.daocloud.io",
            "docker.1panel.live",
            "docker.1ms.run",
            "docker.xuanyuan.me",
            "dockerproxy.net",
        ] {
            assert!(docker.contains(&host), "docker.io 缺少 {host}: {docker:?}");
        }
    }

    #[tokio::test]
    async fn pull_with_mirror_keeps_upstream_dest() {
        let app = build_router(test_state(""));

        let v = send_json(
            app.clone(),
            "POST",
            "/api/registry/pull",
            serde_json::json!({
                "image": "ubuntu:22.04",
                "arch": "all",
                "mirror": "docker.m.daocloud.io"
            }),
        )
        .await;
        assert_eq!(v["code"], 0, "{v}");
        let task_id = v["data"]["task_id"].as_str().unwrap().to_string();

        let detail = poll_task(app.clone(), &task_id).await;
        assert_eq!(detail["data"]["status"], "succeeded", "{detail}");
        assert_eq!(detail["data"]["mirror"], "docker.m.daocloud.io");
        assert_eq!(detail["data"]["source"], "docker.m.daocloud.io/library/ubuntu:22.04");
        assert_eq!(
            detail["data"]["dest"],
            "127.0.0.1:19913/docker.io/library/ubuntu:22.04"
        );
    }

    #[tokio::test]
    async fn pull_without_mirror_record_is_null() {
        let app = build_router(test_state(""));
        let v = send_json(
            app.clone(),
            "POST",
            "/api/registry/pull",
            serde_json::json!({"image": "ubuntu:22.04", "mirror": "  "}),
        )
        .await;
        assert_eq!(v["code"], 0, "{v}");
        let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
        let detail = poll_task(app, &task_id).await;
        assert!(detail["data"]["mirror"].is_null(), "{detail}");
        assert_eq!(detail["data"]["source"], "docker.io/library/ubuntu:22.04");
    }

    #[tokio::test]
    async fn pull_with_invalid_mirror_is_20801() {
        let app = build_router(test_state(""));
        let v = send_json(
            app,
            "POST",
            "/api/registry/pull",
            serde_json::json!({"image": "ubuntu:22.04", "mirror": "http://bad/mirror"}),
        )
        .await;
        assert_eq!(v["code"], 20801, "{v}");
    }

    #[tokio::test]
    async fn sync_with_mirror_keeps_local_repository_name() {
        // 预置本地仓库 docker.io/library/ubuntu
        let tmp = tempfile::TempDir::new().unwrap();
        let frontend = tmp.path().join("dist");
        std::fs::create_dir_all(&frontend).unwrap();
        std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
        let dir = Box::leak(Box::new(tmp)).path().join("dist");
        let mut mock = MockRegistryClient::new();
        mock.seed("docker.io/library/ubuntu", "22.04", "sha256:u");
        let state = AppState::for_test(dir, Arc::new(mock), Arc::new(MockPlatformClient::new()));
        let app = build_router(state);

        let v = send_json(
            app.clone(),
            "POST",
            "/api/registry/sync",
            serde_json::json!({
                "repository": "docker.io/library/ubuntu",
                "tag": "22.04",
                "mirror": "docker.m.daocloud.io"
            }),
        )
        .await;
        assert_eq!(v["code"], 0, "{v}");
        let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
        let detail = poll_task(app, &task_id).await;
        assert_eq!(detail["data"]["mirror"], "docker.m.daocloud.io");
        assert_eq!(detail["data"]["source"], "docker.m.daocloud.io/library/ubuntu:22.04");
        assert_eq!(
            detail["data"]["dest"],
            "127.0.0.1:19913/docker.io/library/ubuntu:22.04"
        );
    }

#[tokio::test]
async fn unknown_task_is_20808() {
    let app = build_router(test_state(""));
    let v = get_json(app, "/api/tasks/does-not-exist").await.1;
    assert_eq!(v["code"], 20808, "{v}");
}

#[tokio::test]
async fn task_progress_detail_and_timeout_are_exposed() {
    let app = build_router(test_state(""));
    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/pull",
        serde_json::json!({"image": "ubuntu:22.04", "timeout_minutes": 480}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
    let detail = poll_task(app, &task_id).await;
    assert_eq!(detail["data"]["status"], "succeeded", "{detail}");
    assert_eq!(detail["data"]["timeout_minutes"], 480);
    assert_eq!(detail["data"]["cancel_requested"], false);
    // 结构化进度：完成态
    assert_eq!(detail["data"]["progress_detail"]["phase"], "done");
    assert_eq!(detail["data"]["progress_detail"]["percent"], 100);
    assert_eq!(detail["data"]["progress"], "任务完成");
}

#[tokio::test]
async fn pull_timeout_zero_means_unlimited_and_retry_is_validated() {
    let app = build_router(test_state(""));
    // `timeout_minutes = 0` = 无限制（D-IH-44）——合法
    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/pull",
        serde_json::json!({"image": "ubuntu:22.04", "timeout_minutes": 0}),
    )
    .await;
    assert_eq!(v["code"], 0, "timeout=0 应为无限制: {v}");
    // 超时无上限——极大值合法
    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/pull",
        serde_json::json!({"image": "ubuntu:22.04", "timeout_minutes": 100000}),
    )
    .await;
    assert_eq!(v["code"], 0, "超时无上限: {v}");
    // retry_times 越界（0 / 6）→ 20801（D-IH-45）
    for bad in [0u64, 6] {
        let v = send_json(
            app.clone(),
            "POST",
            "/api/registry/pull",
            serde_json::json!({"image": "ubuntu:22.04", "retry_times": bad}),
        )
        .await;
        assert_eq!(v["code"], 20801, "retry_times={bad}: {v}");
    }
}

#[tokio::test]
async fn cancel_running_task_and_boundaries() {
    // 用带延迟的 mock 构造可取消的运行中任务
    let tmp = tempfile::TempDir::new().unwrap();
    let frontend = tmp.path().join("dist");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
    let dir = Box::leak(Box::new(tmp)).path().join("dist");
    let runner = std::sync::Arc::new(crate::skopeo::MockSkopeoRunner::with_delay(10_000));
    let state = AppState::for_test_with(
        dir,
        Arc::new(MockRegistryClient::new()),
        Arc::new(MockPlatformClient::new()),
        Arc::new(crate::settings::MemorySettingsStore::new()),
        runner.clone(),
    );
    let app = build_router(state);

    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/pull",
        serde_json::json!({"image": "ubuntu:22.04", "arch": "all"}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let task_id = v["data"]["task_id"].as_str().unwrap().to_string();

    // 等 skopeo 调用启动
    for _ in 0..200 {
        if !runner.invocations().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }

    let v = send_json(
        app.clone(),
        "POST",
        &format!("/api/tasks/{task_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["data"]["cancel_requested"], true, "{v}");

    let detail = poll_task(app.clone(), &task_id).await;
    assert_eq!(detail["data"]["status"], "cancelled", "{detail}");
    assert_eq!(detail["data"]["progress_detail"]["phase"], "cancelled");

    // 已终态再次取消 → 20801
    let v = send_json(
        app.clone(),
        "POST",
        &format!("/api/tasks/{task_id}/cancel"),
        serde_json::json!({}),
    )
    .await;
    assert_eq!(v["code"], 20801, "{v}");

    // 不存在 → 20808
    let v = send_json(app, "POST", "/api/tasks/nope/cancel", serde_json::json!({})).await;
    assert_eq!(v["code"], 20808, "{v}");
}

#[tokio::test]
async fn settings_registry_and_proxy_crud() {
    let app = build_router(test_state(""));

    let v = get_json(app.clone(), "/api/settings").await.1;
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["registries"].as_array().unwrap().len(), 0);
    assert_eq!(v["data"]["proxy"]["enabled"], false);

    let v = send_json(
        app.clone(),
        "PUT",
        "/api/settings/registries",
        serde_json::json!({"host": "docker.m.daocloud.io", "username": "u", "password": "secret"}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["data"]["registries"][0]["host"], "docker.m.daocloud.io");
    assert_eq!(v["data"]["registries"][0]["has_password"], true);
    assert!(!v.to_string().contains("secret"), "响应不得回显密码: {v}");

    let v = send_json(
        app.clone(),
        "PUT",
        "/api/settings/proxy",
        serde_json::json!({
            "enabled": true,
            "http_proxy": "http://proxy:3128",
            "https_proxy": "http://proxy:3128",
            "no_proxy": "example.com"
        }),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["data"]["enabled"], true);
    assert_eq!(v["data"]["http_proxy"], "http://proxy:3128");

    // 非法代理地址
    let v = send_json(
        app.clone(),
        "PUT",
        "/api/settings/proxy",
        serde_json::json!({"enabled": true, "http_proxy": "proxy:3128"}),
    )
    .await;
    assert_eq!(v["code"], 20801, "{v}");

    let v = send_json(
        app,
        "DELETE",
        "/api/settings/registries",
        serde_json::json!({"host": "docker.m.daocloud.io"}),
    )
    .await;
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["registries"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn image_detail_returns_platforms() {
    use crate::registry::PlatformEntry;

    let tmp = tempfile::TempDir::new().unwrap();
    let frontend = tmp.path().join("dist");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
    let dir = Box::leak(Box::new(tmp)).path().join("dist");

    let mut mock = MockRegistryClient::new();
    mock.seed("docker.m.daocloud.io/library/alpine", "3.20", "sha256:idx");
    mock.seed_platforms(
        "docker.m.daocloud.io/library/alpine",
        "3.20",
        vec![
            PlatformEntry {
                os: Some("linux".into()),
                architecture: Some("amd64".into()),
                variant: None,
                digest: Some("sha256:amd".into()),
                size_bytes: Some(1),
            },
            PlatformEntry {
                os: Some("linux".into()),
                architecture: Some("arm64".into()),
                variant: None,
                digest: Some("sha256:arm".into()),
                size_bytes: Some(2),
            },
        ],
    );
    let state = AppState::for_test(dir, Arc::new(mock), Arc::new(MockPlatformClient::new()));
    let app = build_router(state);

    let v = get_json(
        app.clone(),
        "/api/registry/images/detail?repository=docker.m.daocloud.io/library/alpine",
    )
    .await
    .1;
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["data"]["repository"], "docker.m.daocloud.io/library/alpine");
    assert_eq!(v["data"]["tags"][0]["tag"], "3.20");
    assert_eq!(v["data"]["tags"][0]["platforms"].as_array().unwrap().len(), 2);

    // 不存在仓库 → 20803
    let v = get_json(
        app,
        "/api/registry/images/detail?repository=no/such/repo",
    )
    .await
    .1;
    assert_eq!(v["code"], 20803, "{v}");
}

#[tokio::test]
async fn clone_endpoint_starts_task_and_rejects_occupied_target() {
    let tmp = tempfile::TempDir::new().unwrap();
    let frontend = tmp.path().join("dist");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
    let dir = Box::leak(Box::new(tmp)).path().join("dist");
    let mut mock = MockRegistryClient::new();
    mock.seed("docker.io/library/alpine", "3.19", "sha256:a");
    mock.seed("docker.io/library/alpine", "3.20", "sha256:b");
    // 目标仓库已有 stable 标签 → 占用
    mock.seed("docker.io/local/alpine", "stable", "sha256:c");
    let state = AppState::for_test(dir, Arc::new(mock), Arc::new(MockPlatformClient::new()));
    let app = build_router(state);

    // 单 tag 克隆到空目标
    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/clone",
        serde_json::json!({
            "repository": "docker.io/library/alpine",
            "tag": "3.20",
            "target_repository": "docker.io/local/alpine",
            "target_tag": "v320"
        }),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("clone-"), "{task_id}");
    let detail = poll_task(app.clone(), &task_id).await;
    assert_eq!(detail["data"]["status"], "succeeded", "{detail}");
    assert_eq!(detail["data"]["kind"], "clone");
    assert_eq!(detail["data"]["arch"], "all");

    // 目标占用 → 20801，不创建任务
    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/clone",
        serde_json::json!({
            "repository": "docker.io/library/alpine",
            "tag": "3.19",
            "target_repository": "docker.io/local/alpine",
            "target_tag": "stable"
        }),
    )
    .await;
    assert_eq!(v["code"], 20801, "{v}");

    // 源不存在 → 20803
    let v = send_json(
        app,
        "POST",
        "/api/registry/clone",
        serde_json::json!({
            "repository": "docker.io/library/nope",
            "tag": "1.0",
            "target_repository": "docker.io/local/x"
        }),
    )
    .await;
    assert_eq!(v["code"], 20803, "{v}");
}

#[tokio::test]
async fn rename_endpoint_untags_source_on_success() {
    let tmp = tempfile::TempDir::new().unwrap();
    let frontend = tmp.path().join("dist");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
    let dir = Box::leak(Box::new(tmp)).path().join("dist");
    let mut mock = MockRegistryClient::new();
    // 同仓库两个 tag 指向同一 digest
    mock.seed("docker.io/library/alpine", "3.19", "sha256:same");
    mock.seed("docker.io/library/alpine", "3.20", "sha256:same");
    let registry = Arc::new(mock);
    let state = AppState::for_test(
        dir,
        Arc::clone(&registry) as Arc<dyn crate::registry::RegistryClient>,
        Arc::new(MockPlatformClient::new()),
    );
    let app = build_router(state);

    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/rename",
        serde_json::json!({
            "repository": "docker.io/library/alpine",
            "tag": "3.19",
            "target_tag": "stable"
        }),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("rename-"), "{task_id}");
    let detail = poll_task(app, &task_id).await;
    assert_eq!(detail["data"]["status"], "succeeded", "{detail}");
    // 来源标签被摘除，同 digest 的另一个 tag 保留
    assert_eq!(
        registry.untagged.lock().unwrap().clone(),
        vec![("docker.io/library/alpine".to_string(), "3.19".to_string())]
    );
    assert_eq!(
        registry.list_tags("docker.io/library/alpine").await.unwrap(),
        vec!["3.20"]
    );
}

/// 构造 multipart 请求体
fn multipart_body(boundary: &str, parts: &[(&str, Option<&str>, &str)]) -> Vec<u8> {
    let mut body = Vec::new();
    for (name, filename, value) in parts {
        let disposition = match filename {
            Some(f) => format!(
                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{f}\"\r\nContent-Type: application/octet-stream"
            ),
            None => format!("Content-Disposition: form-data; name=\"{name}\""),
        };
        body.extend_from_slice(format!("--{boundary}\r\n{disposition}\r\n\r\n").as_bytes());
        body.extend_from_slice(value.as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    body
}

async fn send_multipart(app: Router, uri: &str, boundary: &str, body: Vec<u8>) -> Value {
    let res = app
        .oneshot(
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .header(
                    axum::http::header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .unwrap();
    let bytes = axum::body::to_bytes(res.into_body(), 1024 * 64).await.unwrap();
    serde_json::from_slice(&bytes).unwrap_or(Value::Null)
}

#[tokio::test]
async fn import_endpoint_requires_file_and_starts_task() {
    let app = build_router(test_state(""));
    let boundary = "IHBOUNDARY";

    // 缺少 file 字段 → 20801
    let body = multipart_body(boundary, &[("repository", None, "docker.io/library/alpine")]);
    let v = send_multipart(app.clone(), "/api/registry/import", boundary, body).await;
    assert_eq!(v["code"], 20801, "{v}");

    // 提供目标名 + 文件 → 创建导入任务
    let body = multipart_body(
        boundary,
        &[
            ("repository", None, "docker.io/library/alpine"),
            ("tag", None, "stable"),
            ("file", Some("alpine.tar"), "fake-archive-bytes"),
        ],
    );
    let v = send_multipart(app.clone(), "/api/registry/import", boundary, body).await;
    assert_eq!(v["code"], 0, "{v}");
    let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("import-"), "{task_id}");
    let detail = poll_task(app, &task_id).await;
    assert_eq!(detail["data"]["status"], "succeeded", "{detail}");
    assert_eq!(detail["data"]["kind"], "import");
    assert_eq!(
        detail["data"]["dest"],
        "127.0.0.1:19913/docker.io/library/alpine:stable"
    );
}

/// 构造带指定 GC 执行器的 state（Mock registry / platform）
fn test_state_with_gc(gc: Arc<dyn crate::gc::RegistryGcRunner>) -> AppState {
    let tmp = tempfile::TempDir::new().unwrap();
    let frontend = tmp.path().join("dist");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
    let dir = Box::leak(Box::new(tmp)).path().join("dist");
    AppState::for_test_with_gc(
        dir,
        Arc::new(MockRegistryClient::new()),
        Arc::new(MockPlatformClient::new()),
        Arc::new(crate::settings::MemorySettingsStore::new()),
        gc,
    )
}

#[tokio::test]
async fn gc_status_and_config_roundtrip() {
    let app = build_router(test_state_with_gc(Arc::new(
        crate::gc::MockRegistryGcRunner::new(),
    )));

    // 默认：关闭 / 24 小时 / 删除无标签 / 可用
    let v = get_json(app.clone(), "/api/registry/gc").await.1;
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["data"]["enabled"], false);
    assert_eq!(v["data"]["interval_hours"], 24);
    assert_eq!(v["data"]["delete_untagged"], true);
    assert_eq!(v["data"]["available"], true);
    assert!(v["data"]["next_run_at"].is_null());
    assert!(v["data"]["storage_dir"].is_null());

    // 启用 + 自定义间隔 → last_run_at 被置为当前时刻，next_run_at 非空
    let v = send_json(
        app.clone(),
        "PUT",
        "/api/registry/gc",
        serde_json::json!({"enabled": true, "interval_hours": 6, "delete_untagged": false}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    assert_eq!(v["data"]["enabled"], true);
    assert_eq!(v["data"]["interval_hours"], 6);
    assert_eq!(v["data"]["delete_untagged"], false);
    assert!(v["data"]["last_run_at"].is_string(), "{v}");
    assert!(v["data"]["next_run_at"].is_string(), "{v}");

    // 间隔越界 → 20801
    let v = send_json(
        app.clone(),
        "PUT",
        "/api/registry/gc",
        serde_json::json!({"enabled": true, "interval_hours": 0}),
    )
    .await;
    assert_eq!(v["code"], 20801, "{v}");
    let v = send_json(
        app,
        "PUT",
        "/api/registry/gc",
        serde_json::json!({"enabled": true, "interval_hours": 721}),
    )
    .await;
    assert_eq!(v["code"], 20801, "{v}");
}

#[tokio::test]
async fn gc_run_starts_task_and_task_is_queryable() {
    let storage = tempfile::TempDir::new().unwrap();
    let mut runner = crate::gc::MockRegistryGcRunner::new();
    runner.storage_dir = Some(storage.path().to_path_buf());
    let app = build_router(test_state_with_gc(Arc::new(runner)));
    let v = send_json(
        app.clone(),
        "POST",
        "/api/registry/gc/run",
        serde_json::json!({"dry_run": true}),
    )
    .await;
    assert_eq!(v["code"], 0, "{v}");
    let task_id = v["data"]["task_id"].as_str().unwrap().to_string();
    assert!(task_id.starts_with("gc-"), "{task_id}");

    let detail = poll_task(app, &task_id).await;
    assert_eq!(detail["data"]["status"], "succeeded", "{detail}");
    assert_eq!(detail["data"]["kind"], "gc");
    assert_eq!(detail["data"]["gc"]["dry_run"], true);
    assert_eq!(detail["data"]["gc"]["delete_untagged"], true);
}

#[tokio::test]
async fn gc_unavailable_is_20812() {
    let app = build_router(test_state_with_gc(Arc::new(
        crate::gc::MockRegistryGcRunner::unavailable(),
    )));
    // 状态：可用但标注原因
    let v = get_json(app.clone(), "/api/registry/gc").await.1;
    assert_eq!(v["code"], 0);
    assert_eq!(v["data"]["available"], false);
    assert!(v["data"]["unavailable_reason"].is_string());

    // 触发 → 20812
    let v = send_json(
        app,
        "POST",
        "/api/registry/gc/run",
        serde_json::json!({}),
    )
    .await;
    assert_eq!(v["code"], 20812, "{v}");
}

#[tokio::test]
async fn client_script_served_at_root() {
    let app = build_router(test_state(""));
    let (status, body) = get_text(app, "/image-hub.sh").await;
    assert_eq!(status, 200);
    assert!(body.starts_with("#!/usr/bin/env bash"), "{body}");
}

#[tokio::test]
async fn client_script_served_under_url_base() {
    let app = build_router(test_state("/app/com.ictrek.image-hub"));
    let (status, body) = get_text(app, "/app/com.ictrek.image-hub/image-hub.sh").await;
    assert_eq!(status, 200);
    assert!(body.contains("echo image-hub"), "{body}");
}

#[tokio::test]
async fn client_script_missing_is_404_not_html() {
    // 仅有 index.html、无脚本的 dist：显式路由必须 404，不得回落为 HTML
    let tmp = tempfile::TempDir::new().unwrap();
    let frontend = tmp.path().join("dist");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<html>image-hub</html>").unwrap();
    let dir = Box::leak(Box::new(tmp)).path().join("dist");
    let state = AppState::for_test(
        dir,
        Arc::new(MockRegistryClient::new()),
        Arc::new(MockPlatformClient::new()),
    );
    let app = build_router(state);
    let (status, body) = get_text(app, "/image-hub.sh").await;
    assert_eq!(status, 404);
    assert!(!body.contains("<html>"), "不得回落为 HTML：{body}");
}

#[tokio::test]
async fn health_and_index_under_url_base() {
        let app = build_router(test_state("/app/com.ictrek.image-hub"));
        let (status, v) = get_json(app, "/app/com.ictrek.image-hub/api/health").await;
        assert_eq!(status, 200);
        assert_eq!(v["code"], 0);

        let app = build_router(test_state("/app/com.ictrek.image-hub"));
        let (status, body) = get_text(app, "/app/com.ictrek.image-hub/").await;
        assert_eq!(status, 200);
        assert!(body.contains("<html>image-hub</html>"));

        let app = build_router(test_state("/app/com.ictrek.image-hub"));
        let (status, body) = get_text(app, "/app/com.ictrek.image-hub/images").await;
        assert_eq!(status, 200, "SPA fallback 应 200: {body}");
        assert!(body.contains("<html>image-hub</html>"));
    }
}