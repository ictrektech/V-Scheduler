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
    forward_to(&crate::inference_runtime::origin(), &path, request, UNAVAILABLE).await
}

pub(super) async fn cluster(
    State(app): State<App>,
    Path(path): Path<String>,
    request: Request,
) -> Api<Response> {
    require(&auth(&app, request.headers()).await?, "admin")?;
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
