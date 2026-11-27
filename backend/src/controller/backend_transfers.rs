#[path = "../../platform/shared/application_backend_channel.rs"]
pub(super) mod wire;
use super::*;
use axum::body::Bytes;

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Manifest {
    model_id: String,
    files: Vec<File>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
}

#[derive(Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct File {
    file_id: String,
    path: String,
    size_bytes: u64,
    sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    shared_path: Option<String>,
}

impl Manifest {
    fn valid(&self) -> bool {
        let mut ids = std::collections::HashSet::new();
        let shared = self.source.as_deref() == Some("shared");
        !self.model_id.is_empty()
            && self.model_id.len() <= 512
            && !self.model_id.chars().any(char::is_control)
            && (self.source.is_none() || shared)
            && (1..=32).contains(&self.files.len())
            && self.files.iter().all(|file| {
                wire::identifier(&file.file_id)
                    && ids.insert(&file.file_id)
                    && !file.path.starts_with('.')
                    && (1..=200).contains(&file.path.len())
                    && !file.path.contains(['/', '\\'])
                    && !file.path.chars().any(char::is_control)
                    && match file.shared_path.as_deref() {
                        Some(path) if shared => path.len() <= 1024
                            && !path.contains('\\') && !path.chars().any(char::is_control)
                            && path.split('/').all(|part| !matches!(part, "" | "." | ".."))
                            && path.rsplit('/').next() == Some(file.path.as_str()),
                        None => !shared,
                        _ => false,
                    }
                    && (1..=(1_u64 << 40)).contains(&file.size_bytes)
                    && file.sha256.len() == 64
                    && file.sha256.bytes().all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Offset {
    offset: u64,
}

pub(super) async fn create(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Json(manifest): Json<Manifest>,
) -> Api<Json<Value>> {
    if !manifest.valid() { return Err(invalid()); }
    if manifest.source.is_some() {
        if !model_hub_ready(&app, &headers, &id).await? {
            return Err(Error(StatusCode::CONFLICT, "BACKEND_SHARED_MODEL_HUB_UNAVAILABLE"));
        }
    } else {
        backend_deployments::channel_target(&app, &headers, &id).await?;
    }
    let bytes = serde_json::to_vec(&manifest).map_err(|_| invalid())?;
    let mut tx = app.db.begin().await.map_err(unavailable)?;
    sqlx::query("SELECT id FROM vrs_backend_deployments WHERE id=$1 FOR SHARE")
        .bind(&id).fetch_optional(&mut *tx).await.map_err(unavailable)?;
    backend_models::ensure_model_not_deleting(&mut tx, &id, &manifest.model_id).await?;
    let reply = backend_deployments::exchange(&app, &headers, &id, wire::Operation::CreateTransfer, &bytes).await?;
    tx.commit().await.map_err(unavailable)?;
    Ok(reply)
}

async fn model_hub_ready(app: &App, headers: &HeaderMap, id: &str) -> Api<bool> {
    backend_deployments::channel_target(app, headers, id).await?;
    match backend_deployments::exchange(app, headers, id, wire::Operation::ModelHub, &[]).await {
        Ok(reply) => Ok(reply.0["data"]["state"] == "ready"),
        Err(Error(_, "BACKEND_CHANNEL_UNAVAILABLE" | "BACKEND_TRANSFER_NOT_FOUND")) => Ok(false),
        Err(error) => Err(error),
    }
}

pub(super) async fn model_hub(
    State(app): State<App>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Api<Json<Value>> {
    let ready = model_hub_ready(&app, &headers, &id).await?;
    Ok(ok(json!({"state": if ready { "ready" } else { "unavailable" }})))
}

pub(super) async fn read(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, transfer_id)): Path<(String, String)>,
) -> Api<Json<Value>> {
    backend_deployments::exchange(&app, &headers, &id, wire::Operation::ReadTransfer { transfer_id }, &[]).await
}

pub(super) async fn write(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, transfer_id, file_id)): Path<(String, String, String)>,
    Query(Offset { offset }): Query<Offset>,
    bytes: Bytes,
) -> Api<Json<Value>> {
    if headers.get(axum::http::header::CONTENT_TYPE).and_then(|value| value.to_str().ok()) != Some("application/octet-stream") {
        return Err(Error(StatusCode::UNSUPPORTED_MEDIA_TYPE, "BACKEND_TRANSFER_CONTENT_TYPE"));
    }
    backend_deployments::exchange(
        &app, &headers, &id, wire::Operation::WriteChunk { transfer_id, file_id, offset }, &bytes,
    ).await
}

pub(super) async fn complete(
    State(app): State<App>,
    headers: HeaderMap,
    Path((id, transfer_id)): Path<(String, String)>,
) -> Api<Json<Value>> {
    backend_deployments::exchange(&app, &headers, &id, wire::Operation::CompleteTransfer { transfer_id }, &[]).await
}
