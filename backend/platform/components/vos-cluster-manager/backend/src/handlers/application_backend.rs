#[path = "../../../../../shared/application_backend.rs"]
pub(crate) mod wire;
#[path = "../../../../../shared/application_backend_channel.rs"]
pub(crate) mod channel;

use base64::{Engine as _, engine::general_purpose::STANDARD};

use axum::{
    Json,
    extract::{Path, State},
};
use std::collections::HashMap;

use crate::{
    error::{ResultBody, VcmError, codes},
    projects::{APP_ID, ProjectDeployBo},
    state::AppState,
};
use wire::{Instance, Operation, PROJECT, RuntimeState};

fn validate(instance: &Instance) -> Result<(), VcmError> {
    if APP_ID != "com.ictrek.v-scheduler" || !instance.valid() {
        return Err(VcmError::new(
            codes::PARAM_INVALID,
            "invalid_application_backend",
        ));
    }
    Ok(())
}

pub(crate) async fn dispatch(
    State(state): State<AppState>,
    Path(member_id): Path<String>,
    Json(instance): Json<Instance>,
) -> Result<Json<ResultBody<RuntimeState>>, VcmError> {
    validate(&instance)?;
    if instance.operation == Operation::Ensure {
        let node = state
            .etcd_cluster_client
            .get_node_record(&member_id)
            .await?
            .ok_or_else(|| VcmError::new(codes::NOT_FOUND, "backend_cluster_member_not_found"))?;
        if node.metadata.node_ip != instance.host.to_string() {
            return Err(VcmError::new(
                codes::PARAM_INVALID,
                "backend_cluster_member_changed",
            ));
        }
    }
    let runtime = state.remote_client.application_backend(&instance).await?;
    Ok(Json(ResultBody::ok(runtime, state.version)))
}

pub(crate) async fn local(
    State(state): State<AppState>,
    Json(instance): Json<Instance>,
) -> Result<Json<ResultBody<RuntimeState>>, VcmError> {
    validate(&instance)?;
    let name = instance.name();
    let projects = &state.projects_client;
    let records = projects.instances().await?;
    let record = records
        .iter()
        .find(|record| record.compose_project_name == name && record.state != "removed");
    if record.is_some_and(|record| record.app_id != APP_ID || record.project_name != PROJECT) {
        return Err(VcmError::new(codes::EXISTS, "backend_project_name_in_use"));
    }
    let current = match record.map(|record| record.state.as_str()) {
        None => RuntimeState::Absent,
        Some("deployed") => RuntimeState::Running,
        Some("deploying" | "removing") => RuntimeState::Pending,
        Some(_) => RuntimeState::Failed,
    };
    let runtime = match (instance.operation, current) {
        (Operation::Ensure, RuntimeState::Absent) => {
            let mut env_vars = HashMap::from([
                (
                    "SCHEDULER_BACKEND_CPU".into(),
                    format!(
                        "{:.3}",
                        f64::from(instance.cpu_millicores.unwrap_or(0)) / 1000.0
                    ),
                ),
                (
                    "SCHEDULER_BACKEND_MEMORY".into(),
                    instance
                        .memory_mib
                        .map(|mib| format!("{mib}m"))
                        .unwrap_or_else(|| "0".into()),
                ),
                (
                    "SCHEDULER_BACKEND_CONFIG_B64".into(),
                    instance.config_b64.unwrap_or_else(|| "e30=".into()),
                ),
                ("SCHEDULER_BACKEND_IMAGE".into(), instance.image),
                (
                    "SCHEDULER_BACKEND_PORT".into(),
                    instance.container_port.to_string(),
                ),
                (
                    "SCHEDULER_BACKEND_HOST_PORT".into(),
                    instance.host_port.to_string(),
                ),
                (
                    "SCHEDULER_BACKEND_RUNTIME".into(),
                    if instance.nvidia { "nvidia" } else { "runc" }.into(),
                ),
                (
                    "SCHEDULER_BACKEND_VISIBLE_DEVICES".into(),
                    if instance.nvidia { "all" } else { "void" }.into(),
                ),
            ]);
            if let Some(token) = instance.channel_token {
                env_vars.insert("V_SCHEDULER_BACKEND_CHANNEL_TOKEN".into(), token);
            }
            projects
                .deploy_async(&ProjectDeployBo {
                    app_id: APP_ID.into(),
                    project_name: PROJECT.into(),
                    compose_project_name: name,
                    env_vars,
                    health_timeout_secs: Some(120),
                })
                .await?;
            RuntimeState::Pending
        }
        (Operation::Remove, RuntimeState::Running | RuntimeState::Failed) => {
            projects.remove_async(&name).await?;
            RuntimeState::Pending
        }
        (Operation::Status, RuntimeState::Running) => {
            match projects
                .status(&name)
                .await?
                .and_then(|result| result.runtime)
            {
                Some(runtime)
                    if runtime.docker_reachable == Some(true)
                        && runtime
                            .containers
                            .iter()
                            .any(|item| item.service == "backend" && item.state == "running") =>
                {
                    RuntimeState::Running
                }
                Some(runtime) if runtime.docker_reachable == Some(true) => RuntimeState::Stopped,
                _ => {
                    return Err(VcmError::new(
                        codes::PROJECTS_CALL_FAILED,
                        "backend_project_status_unavailable",
                    ));
                }
            }
        }
        (_, runtime) => runtime,
    };
    Ok(Json(ResultBody::ok(runtime, state.version)))
}

fn channel_error() -> VcmError {
    VcmError::new(codes::PARAM_INVALID, "application_backend_channel_unavailable")
}

pub(crate) async fn dispatch_channel(
    State(state): State<AppState>,
    Path(member_id): Path<String>,
    Json(packet): Json<channel::Packet>,
) -> Result<Json<ResultBody<channel::Reply>>, VcmError> {
    if APP_ID != "com.ictrek.v-scheduler" || !packet.valid_target() {
        return Err(channel_error());
    }
    let member = state.etcd_cluster_client.get_node_record(&member_id).await?
        .ok_or_else(channel_error)?;
    if member.metadata.node_ip != packet.host.to_string() {
        return Err(channel_error());
    }
    let reply = state.remote_client.application_backend_channel(&packet).await?;
    Ok(Json(ResultBody::ok(reply, state.version)))
}

pub(crate) async fn local_channel(
    State(state): State<AppState>,
    Json(packet): Json<channel::Packet>,
) -> Result<Json<ResultBody<channel::Reply>>, VcmError> {
    if APP_ID != "com.ictrek.v-scheduler" || !packet.valid_target() {
        return Err(channel_error());
    }
    let name = format!("vs-backend-{}", packet.deployment_id);
    let instances = state.projects_client.instances().await?;
    if !instances.iter().any(|instance| instance.compose_project_name == name
        && instance.app_id == APP_ID && instance.project_name == PROJECT && instance.state == "deployed") {
        return Err(channel_error());
    }
    let bytes = STANDARD.decode(&packet.body_b64).map_err(|_| channel_error())?;
    let (method, path, content_type) = packet.operation.request(bytes.len()).ok_or_else(channel_error)?;
    let gateway = std::env::var("VOS_HOST_GW_IP").ok()
        .and_then(|value| value.parse::<std::net::Ipv4Addr>().ok())
        .ok_or_else(channel_error)?;
    let method = reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| channel_error())?;
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(std::time::Duration::from_secs(packet.operation.timeout_seconds()))
        .build()
        .map_err(|_| channel_error())?;
    let mut request = client
        .request(method, format!("http://{gateway}:{}{path}", packet.host_port))
        .bearer_auth(packet.token)
        .body(bytes);
    if let Some(content_type) = content_type {
        request = request.header(reqwest::header::CONTENT_TYPE, content_type);
    }
    let mut response = request.send().await.map_err(|_| channel_error())?;
    let status = response.status();
    let data = if status.is_success() || status.as_u16() == 409 {
        let mut result = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| channel_error())? {
            if result.len() + chunk.len() > packet.operation.max_reply_bytes() {
                return Err(channel_error());
            }
            result.extend_from_slice(&chunk);
        }
        if status.is_success() {
            serde_json::from_slice(&result).map_err(|_| channel_error())?
        } else {
            channel::shared_unavailable(status.as_u16(), &result)
        }
    } else {
        serde_json::Value::Null
    };
    Ok(Json(ResultBody::ok(channel::Reply { status: status.as_u16(), data }, state.version)))
}
