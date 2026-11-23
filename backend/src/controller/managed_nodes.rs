use std::{sync::OnceLock, time::Duration};

use axum::{body::to_bytes, extract::Request, http::header};

use super::*;

#[derive(Deserialize)]
pub(super) struct Filters {
    profile: Option<String>,
}

pub(super) async fn forward(
    State(app): State<App>,
    Query(filters): Query<Filters>,
    request: Request,
) -> Api<Response> {
    let reading = request.method() == axum::http::Method::GET;
    let can_manage = match auth(&app, request.headers()).await {
        Ok(principal) if reading && principal.role == "submitter" => false,
        Ok(principal) => {
            require(&principal, "admin")?;
            true
        }
        Err(Error(StatusCode::FORBIDDEN, _)) if reading => {
            crate::identity::verified_account(request.headers())
                .await
                .map_err(|status| Error(status, "FORBIDDEN"))?
        }
        Err(error) => return Err(error),
    };
    if !crate::managed_nodes::enabled() {
        return Err(Error(StatusCode::NOT_FOUND, "NODE_MANAGER_DISABLED"));
    }
    let (parts, body) = request.into_parts();
    let suffix = parts
        .uri
        .path()
        .strip_prefix("/managed-nodes")
        .ok_or_else(invalid)?;
    if !suffix
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || b"/_-".contains(&byte))
    {
        return Err(invalid());
    }
    let body = to_bytes(body, 64 * 1024).await.map_err(|_| invalid())?;
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    let client = CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(3))
            .timeout(Duration::from_secs(205))
            .build()
            .expect("local node manager client")
    });
    let response = client
        .request(
            parts.method,
            format!("{}/nodes{suffix}", crate::managed_nodes::origin()),
        )
        .header(header::CONTENT_TYPE, "application/json")
        .body(body)
        .send()
        .await
        .map_err(|_| Error(StatusCode::SERVICE_UNAVAILABLE, "NODE_MANAGER_UNAVAILABLE"))?;
    let status = response.status();
    let value = response.json::<Value>().await.unwrap_or(Value::Null);
    let modelinfer_error = value["code"].as_i64().filter(|code| *code != 0);
    if !status.is_success() || modelinfer_error.is_some() {
        let (status, code) = match (modelinfer_error, value["msg"].as_str()) {
            (_, Some("image_pull_failed")) => (StatusCode::BAD_GATEWAY, "NODE_IMAGE_PULL_FAILED"),
            (_, Some("container_start_failed")) => {
                (StatusCode::BAD_GATEWAY, "NODE_CONTAINER_START_FAILED")
            }
            (Some(20701), _) => (StatusCode::UNPROCESSABLE_ENTITY, "INVALID_REQUEST"),
            (Some(20702), _) => (StatusCode::NOT_FOUND, "NODE_NOT_FOUND"),
            (Some(20703), _) => (StatusCode::CONFLICT, "NODE_ALREADY_EXISTS"),
            (Some(20704 | 20705), _) => (StatusCode::BAD_GATEWAY, "NODE_SSH_FAILED"),
            (Some(_), _) => (StatusCode::BAD_GATEWAY, "NODE_OPERATION_FAILED"),
            (None, _) => (status, "NODE_OPERATION_FAILED"),
        };
        return Err(Error(status, code));
    }
    if value.is_null() {
        return Err(Error(
            StatusCode::BAD_GATEWAY,
            "NODE_MANAGER_INVALID_RESPONSE",
        ));
    }
    let mut data = if reading {
        let rows = value.as_array().ok_or(Error(
            StatusCode::BAD_GATEWAY,
            "NODE_MANAGER_INVALID_RESPONSE",
        ))?;
        let profile = match filters.profile.as_deref() {
            Some("thor" | "thor-spark") => Some("jetson-thor"),
            Some("l4t") => Some("jetson-orin"),
            other => other,
        };
        let rows = rows.iter().filter(|row| {
            profile.is_none_or(|profile| {
                row["compatible_profiles"]
                    .as_array()
                    .is_some_and(|profiles| profiles.iter().any(|value| value == profile))
            })
        });
        if can_manage {
            json!({"items": rows.collect::<Vec<_>>(), "can_manage": true})
        } else {
            json!({
                "items": rows.map(|row| json!({
                    "id": row["id"], "name": row["name"], "status": row["status"],
                    "arch": row["arch"], "gpu_model": row["gpu_model"], "profile": row["profile"],
                    "detected_profile": row["detected_profile"],
                    "compatible_profiles": row["compatible_profiles"],
                    "last_checked_at": row["last_checked_at"],
                })).collect::<Vec<_>>(),
                "can_manage": false
            })
        }
    } else {
        value.clone()
    };
    if reading && filters.profile.is_none() && crate::cluster_runtime::enabled() {
        if let Some(members) = crate::cluster_runtime::members(client).await {
            let mut inventory = Vec::with_capacity(members.len());
            for member in members {
                let Some(id) = member["member_id"].as_str() else { continue };
                let node_ip = member["node_ip"].as_str().and_then(|ip| ip.parse::<std::net::IpAddr>().ok());
                let ssh_node_id = node_ip.and_then(|ip| value.as_array()?.iter().find_map(|row| {
                    let host = row["host"].as_str()?.parse::<std::net::IpAddr>().ok()?;
                    (ip == host).then(|| row["id"].as_str()).flatten()
                }));
                inventory.push(json!({
                    "id":id,
                    "name":member["display_name"].as_str().filter(|name| !name.is_empty()).unwrap_or(id),
                    "role":member["role"].as_str().unwrap_or("unknown"),
                    "ssh_node_id":ssh_node_id
                }));
            }
            data["cluster_nodes"] = json!(inventory);
        }
    }
    let mut response = ok(data).into_response();
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        header::HeaderValue::from_static("no-store"),
    );
    Ok(response)
}
