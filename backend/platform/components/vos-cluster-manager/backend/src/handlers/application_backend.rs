#[path = "../../../../../shared/application_backend.rs"]
pub(crate) mod wire;

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
            let env_vars = HashMap::from([
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
