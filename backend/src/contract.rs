//! Additive management error fields; successful legacy payloads stay unchanged.
use axum::{
    Json,
    http::{HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use uuid::Uuid;
pub async fn confirm_application_binding(mut response: reqwest::Response, revision: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(response.status().is_success(), "application binding pending acceptance");
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        anyhow::ensure!(bytes.len().saturating_add(chunk.len()) <= 262144, "application binding receipt exceeds bound");
        bytes.extend_from_slice(&chunk);
    }
    let receipt: Value = serde_json::from_slice(&bytes)?;
    let receipt = receipt.get("data").unwrap_or(&receipt);
    let expected = revision.as_u64().filter(|revision| *revision > 0);
    anyhow::ensure!(expected.is_some() && receipt["configured"] == true && receipt["binding_revision"].as_u64() == expected, "application binding pending acceptance");
    Ok(())
}
pub fn code(status: StatusCode) -> &'static str {
    match status.as_u16() {
        400 | 422 => "INVALID_REQUEST",
        401 => "UNAUTHENTICATED",
        403 => "FORBIDDEN",
        404 => "NOT_FOUND",
        405 => "METHOD_NOT_ALLOWED",
        409 => "STATE_CONFLICT",
        413 => "REQUEST_TOO_LARGE",
        429 => "WAIT_RESOURCE",
        502 => "UPSTREAM_UNAVAILABLE",
        503 | 504 => "TEMPORARILY_UNAVAILABLE",
        _ => "INTERNAL_ERROR",
    }
}
pub fn envelope(data: Value, code: &str, message: &str) -> Value {
    json!({"code":code,"category":"v-scheduler","data":data,"msg":message,"version":"1","request_id":Uuid::new_v4().to_string()})
}
pub fn failure(status: StatusCode, code: &str, message: &str, legacy_detail: bool) -> Response {
    let mut value = envelope(json!({}), code, message);
    value["error"] = json!({"code":code,"message":message});
    if legacy_detail {
        value["detail"] = json!(message)
    }
    let request_id = value["request_id"].as_str().unwrap().to_owned();
    let mut response = (status, Json(value)).into_response();
    if status == StatusCode::UNAUTHORIZED {
        response
            .headers_mut()
            .insert("www-authenticate", HeaderValue::from_static("Bearer"));
    }
    response
        .headers_mut()
        .insert("x-request-id", HeaderValue::from_str(&request_id).unwrap());
    response
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn binding_receipt_requires_the_current_revision_and_configuration() {
        fn response(status: u16, body: Value) -> reqwest::Response {
            axum::http::Response::builder().status(status).body(body.to_string()).unwrap().into()
        }
        for body in [json!({}), json!({"configured":false,"binding_revision":4}), json!({"configured":true,"binding_revision":3}), json!({"configured":true,"binding_revision":"4"}), json!({"data":{"configured":false,"binding_revision":4}})] {
            assert!(confirm_application_binding(response(200, body), &json!(4)).await.is_err(), "HTTP success alone must not mark an application ready");
        }
        for body in [json!({"configured":true,"binding_revision":4}), json!({"data":{"configured":true,"binding_revision":4}})] {
            assert!(confirm_application_binding(response(200, body), &json!(4)).await.is_ok());
        }
        assert!(confirm_application_binding(response(422, json!({"configured":true,"binding_revision":4})), &json!(4)).await.is_err());
        assert!(confirm_application_binding(response(200, json!({"configured":true})), &Value::Null).await.is_err());
    }
    #[test]
    fn matches_shared_status_contract() {
        let spec: Value =
            serde_json::from_str(include_str!("../../api/error-contract.json")).unwrap();
        for (status, expected) in spec["http_codes"].as_object().unwrap() {
            assert_eq!(
                code(StatusCode::from_u16(status.parse().unwrap()).unwrap()),
                expected.as_str().unwrap()
            );
        }
    }
    #[test]
    fn stable_status_codes() {
        for (status, expected) in [
            (401, "UNAUTHENTICATED"),
            (403, "FORBIDDEN"),
            (429, "WAIT_RESOURCE"),
            (503, "TEMPORARILY_UNAVAILABLE"),
        ] {
            assert_eq!(code(StatusCode::from_u16(status).unwrap()), expected)
        }
    }
    #[test]
    fn warehouse_errors_keep_legacy_detail_and_trace() {
        let response = failure(
            StatusCode::BAD_REQUEST,
            "INVALID_REQUEST",
            "invalid files",
            true,
        );
        assert_eq!(response.status(), 400);
        assert!(!response.headers()["x-request-id"].is_empty())
    }
}

// Normalize framework-generated API rejections too (malformed JSON, 404/405).
pub async fn normalize_errors(
    request: axum::extract::Request,
    next: axum::middleware::Next,
) -> Response {
    let api = request.uri().path().starts_with("/api/")
        || request.uri().path().starts_with("/scheduler/api/");
    let response = next.run(request).await;
    if api
        && response.status().is_client_error()
        && response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .is_none_or(|v| !v.starts_with("application/json"))
    {
        let status = response.status();
        return failure(
            status,
            code(status),
            status.canonical_reason().unwrap_or("Request rejected"),
            true,
        );
    }
    response
}

#[cfg(test)]
mod rejection_tests {
    use super::*;
    #[tokio::test]
    async fn malformed_json_and_unknown_routes_share_error_contract() {
        let app = axum::Router::new()
            .route(
                "/api/test",
                axum::routing::post(|Json(value): Json<Value>| async move { Json(value) }),
            )
            .fallback_service(tower_http::services::ServeDir::new(
                "/nonexistent-contract-fixture-ui",
            ))
            .layer(axum::middleware::from_fn(normalize_errors));
        let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let origin = format!("http://{}", socket.local_addr().unwrap());
        let task = tokio::spawn(async move { axum::serve(socket, app).await.unwrap() });
        let client = reqwest::Client::new();
        for (method, path, body, want) in [
            ("POST", "/api/test", "{", 400),
            ("GET", "/api/test", "", 405),
            ("GET", "/api/missing", "", 404),
        ] {
            let response = client
                .request(method.parse().unwrap(), format!("{origin}{path}"))
                .header("content-type", "application/json")
                .body(body.to_owned())
                .send()
                .await
                .unwrap();
            assert_eq!(response.status().as_u16(), want);
            let trace = response.headers()["x-request-id"]
                .to_str()
                .unwrap()
                .to_owned();
            let value: Value = response.json().await.unwrap();
            assert_eq!(value["request_id"], trace);
            assert_eq!(value["code"], code(StatusCode::from_u16(want).unwrap()));
            assert_eq!(value["detail"], value["msg"]);
        }
        task.abort();
    }
}
