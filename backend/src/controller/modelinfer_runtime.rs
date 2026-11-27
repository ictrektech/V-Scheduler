use std::{sync::OnceLock, time::Duration};

use axum::{body::Body, extract::Request, http::header};

use super::*;

const UNAVAILABLE: Error = Error(StatusCode::SERVICE_UNAVAILABLE, "INFERENCE_RUNTIME_UNAVAILABLE");

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .build()
            .expect("local inference client")
    })
}

pub(super) async fn health(State(app): State<App>, headers: HeaderMap) -> Api<Json<Value>> {
    require(&auth(&app, &headers).await?, "admin")?;
    if !crate::inference_runtime::enabled() {
        return Ok(ok(json!({ "enabled": false, "ready": false })));
    }

    let health = client()
        .get(format!("{}/api/health", crate::inference_runtime::origin()))
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .map_err(|_| UNAVAILABLE)?;
    if !health.status().is_success() {
        return Err(UNAVAILABLE);
    }
    let runtime: Value = health.json().await.map_err(|_| UNAVAILABLE)?;
    let ready = runtime["code"] == 0 && runtime["data"]["db_ready"] == true;
    if !ready {
        return Err(UNAVAILABLE);
    }
    Ok(ok(json!({
        "enabled": true,
        "ready": true,
        "version": runtime["data"]["version"].as_str(),
    })))
}

pub(super) async fn service_health(State(app): State<App>, request: Request) -> Api<Response> {
    require(&auth(&app, request.headers()).await?, "admin")?;
    if !crate::inference_runtime::enabled() {
        return Err(Error(StatusCode::NOT_FOUND, "INFERENCE_RUNTIME_DISABLED"));
    }
    forward_to(
        &crate::inference_runtime::origin(),
        "health",
        request,
        UNAVAILABLE,
    )
    .await
}

pub(super) async fn forward(
    State(app): State<App>,
    Path(path): Path<String>,
    request: Request,
) -> Api<Response> {
    require(&auth(&app, request.headers()).await?, "admin")?;
    if !crate::inference_runtime::enabled() {
        return Err(Error(StatusCode::NOT_FOUND, "INFERENCE_RUNTIME_DISABLED"));
    }
    let request = if let (axum::http::Method::POST, ["nodes", id, "power"]) =
        (request.method().clone(), path.split('/').collect::<Vec<_>>().as_slice())
    {
        let (parts, body) = request.into_parts();
        let bytes = axum::body::to_bytes(body, 4096).await.map_err(|_| invalid())?;
        let parameters: Value = serde_json::from_slice(&bytes).map_err(|_| invalid())?;
        if parameters["force"] != true {
            let mut nodes = app.config.nodes.iter()
                .filter(|node| node.discovery_source == "modelinfer" && node.discovery_id == *id)
                .map(|node| node.id.clone()).collect::<Vec<_>>();
            if let Some(local) = app.config.local_agent.as_ref().and_then(|entry| entry["node_id"].as_str()) {
                let is_local: bool = sqlx::query_scalar(
                    "SELECT EXISTS(SELECT 1 FROM modelinfer_nodes WHERE id=$1 AND access_mode='local')",
                ).bind(id).fetch_one(&app.db).await.map_err(unavailable)?;
                if is_local {
                    nodes.push(local.to_owned());
                }
            }
            let busy: bool = sqlx::query_scalar(
                "SELECT EXISTS(
                    SELECT 1 FROM vrs_tasks WHERE node=ANY($1) AND status IN ('queued','assigned','running','recovering','cancelling','retry_wait')
                    UNION ALL SELECT 1 FROM vrs_permits WHERE node=ANY($1) AND state<>'released'
                    UNION ALL SELECT 1 FROM vrs_reservations WHERE node=ANY($1) AND state<>'released'
                    UNION ALL SELECT 1 FROM vrs_permit_placements x JOIN vrs_permits p ON p.id=x.permit_id
                    WHERE x.node_id=ANY($1) AND p.state<>'released')",
            ).bind(&nodes).fetch_one(&app.db).await.map_err(unavailable)?;
            if busy {
                return Err(Error(StatusCode::CONFLICT, "NODE_HAS_SCHEDULER_WORK"));
            }
        }
        Request::from_parts(parts, Body::from(bytes))
    } else {
        request
    };
    forward_to(&crate::inference_runtime::origin(), &path, request, UNAVAILABLE).await
}

pub(super) async fn cluster(
    State(app): State<App>,
    Path(path): Path<String>,
    request: Request,
) -> Api<Response> {
    require(&auth(&app, request.headers()).await?, "admin")?;
    if path.starts_with("application-backend/") && path.ends_with("/channel") {
        return Err(Error(StatusCode::FORBIDDEN, "FORBIDDEN"));
    }
    if !crate::cluster_runtime::enabled() {
        return Err(Error(StatusCode::NOT_FOUND, "CLUSTER_MANAGER_DISABLED"));
    }
    forward_to(
        crate::cluster_runtime::ORIGIN,
        &path,
        request,
        Error(StatusCode::SERVICE_UNAVAILABLE, "CLUSTER_MANAGER_UNAVAILABLE"),
    ).await
}

pub(super) async fn image_hub(
    State(app): State<App>,
    Path(path): Path<String>,
    request: Request,
) -> Api<Response> {
    require(&auth(&app, request.headers()).await?, "admin")?;
    if !crate::image_hub_runtime::enabled() {
        return Err(Error(StatusCode::NOT_FOUND, "IMAGE_HUB_DISABLED"));
    }
    forward_to(
        crate::image_hub_runtime::ORIGIN,
        &path,
        request,
        Error(StatusCode::SERVICE_UNAVAILABLE, "IMAGE_HUB_UNAVAILABLE"),
    )
    .await
}

async fn forward_to(origin: &str, path: &str, request: Request, error: Error) -> Api<Response> {
    let mut url = reqwest::Url::parse(origin).expect("fixed local runtime origin");
    {
        let mut target = url.path_segments_mut().expect("hierarchical inference origin");
        target.clear().push("api");
        for part in path.split('/') {
            if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
                return Err(invalid());
            }
            target.push(part);
        }
    }
    url.set_query(request.uri().query());
    let (parts, body) = request.into_parts();
    let mut upstream = client().request(parts.method, url);
    for name in [header::ACCEPT, header::CONTENT_TYPE] {
        if let Some(value) = parts.headers.get(&name) {
            upstream = upstream.header(name, value);
        }
    }
    let result = upstream
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
        .map_err(|_| Error(error.0, error.1))?;
    let mut response = Response::builder().status(result.status());
    for name in [header::CONTENT_TYPE, header::CONTENT_DISPOSITION] {
        if let Some(value) = result.headers().get(&name) {
            response = response.header(name, value);
        }
    }
    response
        .header(header::CACHE_CONTROL, "no-store")
        .header("x-accel-buffering", "no")
        .body(Body::from_stream(result.bytes_stream()))
        .map_err(|_| error)
}
