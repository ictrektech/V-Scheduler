//! Weight mutations use the same stop proof as residency before touching manifests.
use super::*;
use sha2::{Digest, Sha256};
pub(super) fn native_name(model: &str) -> String {
    if model.rsplit('/').next().unwrap_or("").contains(':') {
        model.into()
    } else {
        format!("{model}:latest")
    }
}
pub(crate) async fn prepare_model_change(model: &str) -> anyhow::Result<()> {
    let Some(g) = MANAGED_GATEWAY.get().and_then(std::sync::Weak::upgrade) else {
        // The standalone repository CLI has no managed gateway or resident leases.
        return Ok(());
    };
    let model = native_name(model);
    anyhow::ensure!(
        residency::unload_for_change(&g, &model).await,
        "model unload is not confirmed; wait for native stop recovery before changing weights"
    );
    for role in ["qa", "embedding", "rerank"] {
        let upstream = Upstream {
            id: format!("v-scheduler/ollama-{role}"),
            source: "v-scheduler".into(),
            base_url: format!("http://v-scheduler-ollama-{role}:11535/v1"),
            ..Default::default()
        };
        let key = format!("{}/{model}", upstream.id);
        let path = g
            .state_path
            .parent()
            .unwrap()
            .join("ollama-bindings")
            .join(format!("{:x}.json", Sha256::digest(key.as_bytes())));
        if path.exists() {
            let approval =
                lease::approval(&g, &upstream, &model, "POST", "/chat/completions", false)
                    .map_err(|_| {
                        anyhow::anyhow!(
                            "model resource binding cannot be verified before changing weights"
                        )
                    })?
                    .ok_or_else(|| anyhow::anyhow!("model resource binding is missing"))?;
            let state = lease::call(&g, text(&approval.config, "token"), "/permits/inspect",
                json!({"executor_id":approval.policy["executor_id"],"upstream_url":upstream.base_url,"model_id":model}), false)
                .await.map_err(|_| anyhow::anyhow!("model reservations cannot be verified before changing weights"))?;
            anyhow::ensure!(
                state["outstanding_permits"].as_i64() == Some(0),
                "model still has active or unconfirmed reservations; retry after native stop recovery"
            );
        }
        let nonce = uuid::Uuid::new_v4().simple().to_string();
        let response = g
            .client
            .post(format!(
                "http://v-scheduler-ollama-{role}:11434/api/scheduler/confirm-stopped"
            ))
            .timeout(Duration::from_secs(5))
            .json(&json!({"model":model,"nonce":nonce}))
            .send()
            .await?;
        // No installed manifest is valid for a new download, after all managed
        // reservations above have been checked. Other failures remain closed.
        if response.status() == StatusCode::NOT_FOUND {
            continue;
        }
        anyhow::ensure!(
            response.status().is_success(),
            "model is active or native unloading is unavailable"
        );
        let receipt = read_json(response, 65536)
            .await
            .map_err(|_| anyhow::anyhow!("native stop receipt is unavailable"))?;
        anyhow::ensure!(
            receipt["protocol_version"] == 1
                && receipt["model"] == model
                && receipt["nonce"] == nonce
                && receipt["stopped"] == true
                && receipt["native_exit_barrier"] == true,
            "native model stop is not confirmed; weights were not changed"
        );
    }
    Ok(())
}
pub(crate) fn forget_builtin_model(model: &str) {
    let Some(g) = MANAGED_GATEWAY.get().and_then(std::sync::Weak::upgrade) else {
        return;
    };
    let model = native_name(model);
    let mut registry = g.registry.lock().unwrap();
    for role in ["qa", "embedding", "rerank"] {
        if let Some(models) = registry
            .models
            .get_mut(&format!("v-scheduler/ollama-{role}"))
        {
            models.retain(|m| native_name(&m.id) != model);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_names_preserve_explicit_tags() {
        assert_eq!(native_name("org/model"), "org/model:latest");
        assert_eq!(native_name("org/model:2b"), "org/model:2b");
        assert_eq!(
            native_name("registry:5000/org/model"),
            "registry:5000/org/model:latest"
        );
    }
}
