//! Read-only correlation of repository weights, declared services, discovered
//! gateway models and private admission bindings. Never loads a model to probe it.
use super::*;

async fn hub(g: &Gateway, path: &str) -> Api<Value> {
    let base = env(
        "V_SCHEDULER_MODEL_MANAGEMENT_URL",
        "http://v-scheduler:6005",
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        let response = g
            .client
            .get(format!("{}{path}", base.trim_end_matches('/')))
            .send()
            .await
            .map_err(|_| Failure::new(503, "model repository is unavailable"))?;
        if !response.status().is_success() {
            return Err(Failure::new(503, "model repository is unavailable"));
        }
        read_json(response, 4 << 20).await
    })
    .await
    .map_err(|_| Failure::new(503, "model repository inspection timed out"))?
}
fn ollama_name(value: &str) -> String {
    let name = value.strip_prefix("ollama://").unwrap_or(value);
    if name.rsplit('/').next().unwrap_or("").contains(':') {
        name.into()
    } else {
        format!("{name}:latest")
    }
}
fn linked(weight: &Value, service: &Value, model: &str) -> bool {
    let id = text(weight, "model_id");
    if service["metadata"]["model_bindings"][model] == id {
        return true;
    }
    id.starts_with("ollama://")
        && service["kind"] == "ollama"
        && ollama_name(id) == ollama_name(model)
}
fn same_origin(service: &Value, u: &Upstream) -> bool {
    if let Some(role) = u.id.strip_prefix("v-scheduler/ollama-") {
        return service["service_id"] == format!("v-scheduler-ollama-{role}");
    }
    ["gateway_url", "base_url"]
        .into_iter()
        .filter_map(|k| service[k].as_str())
        .any(|url| {
            url.trim_end_matches('/').trim_end_matches("/v1")
                == u.base_url.trim_end_matches('/').trim_end_matches("/v1")
        })
}
pub(super) async fn snapshot(g: &Gateway, id: &Identity) -> Api<Value> {
    tokio::time::timeout(Duration::from_secs(8), inspect(g, id))
        .await
        .map_err(|_| {
            Failure::new(
                503,
                "model lifecycle inspection timed out; no inference was started",
            )
        })?
}
async fn inspect(g: &Gateway, id: &Identity) -> Api<Value> {
    if !id.admin {
        return Err(Failure::new(
            403,
            "model lifecycle inspection requires an administrator",
        ));
    }
    let (weights, services) = tokio::try_join!(
        hub(g, "/api/v1/models?status=all"),
        hub(g, "/api/v1/inference-services?include_builtin=true")
    )?;
    let (catalog, upstreams) = {
        let r = g.registry.lock().unwrap();
        (
            r.upstreams
                .values()
                .filter(|u| u.visible(id))
                .flat_map(|u| {
                    r.models
                        .get(&u.id)
                        .into_iter()
                        .flatten()
                        .map(|m| (u.clone(), m.clone(), r.status(u)))
                })
                .collect::<Vec<_>>(),
            r.upstreams
                .values()
                .filter(|u| u.visible(id))
                .cloned()
                .collect::<Vec<_>>(),
        )
    };
    let mut items = Vec::new();
    for weight in weights.as_array().into_iter().flatten() {
        let mut deployments = Vec::new();
        for (u, m, status) in &catalog {
            for service in services
                .as_array()
                .into_iter()
                .flatten()
                .filter(|s| same_origin(s, u) && linked(weight, s, &m.id))
            {
                let mut blockers = Vec::new();
                if !u.live() {
                    blockers.push("UPSTREAM_UNAVAILABLE");
                }
                if status["state"] == "paused" {
                    blockers.push("ROUTING_PAUSED");
                }
                if weight["status"] != "ready" && weight["status"] != "READY" {
                    blockers.push("WEIGHTS_NOT_READY");
                }
                let binding = match lease::approval(g, u, &m.id, "POST", "/chat/completions", false)
                {
                    Ok(Some(approval)) => {
                        let result=lease::call(g,text(&approval.config,"token"),"/permits/inspect",json!({"executor_id":approval.policy["executor_id"],"upstream_url":u.base_url,"model_id":m.id}),false).await;
                        let size_current = !text(&approval.policy,"executor_id").starts_with("builtin-ollama-auto-")
                            || approval.policy["native_model_size_bytes"].as_i64().filter(|n|*n>0)
                                .is_some_and(|size| weight["size"].as_i64()==Some(size));
                        match result {
                            Ok(_) if !size_current => {
                                blockers.push("SCHEDULER_BINDING_PENDING");
                                "pending"
                            }
                            Ok(state) if state["node_ready"] == true => "binding_ready",
                            Ok(_) => {
                                blockers.push("EXECUTION_NODE_UNAVAILABLE");
                                "node_unavailable"
                            }
                            Err(_) => {
                                blockers.push("SCHEDULER_BINDING_PENDING");
                                "pending"
                            }
                        }
                    }
                    Ok(None) => "external",
                    Err(_) => {
                        blockers.push("SCHEDULER_BINDING_PENDING");
                        "pending"
                    }
                };
                deployments.push(json!({"service_id":service["service_id"],"upstream_id":u.id,"gateway_model":m.qualified,
                    "native_model":m.id,"endpoints":m.endpoints,"route_state":status["state"],"scheduler_state":binding,
                    "configured_callable":blockers.is_empty(),"blockers":blockers}));
            }
        }
        // A failed discovery must not erase the association or disguise an
        // offline backend as missing weights. Explicit mappings also describe
        // a service before its gateway registration has become visible.
        for service in services.as_array().into_iter().flatten() {
            let native = service["metadata"]["model_bindings"]
                .as_object()
                .and_then(|links| {
                    links
                        .iter()
                        .find(|(_, v)| *v == &weight["model_id"])
                        .map(|(k, _)| k.clone())
                });
            let offline = upstreams
                .iter()
                .find(|u| same_origin(service, u) && !u.live());
            let native = native.or_else(|| {
                (offline.is_some()
                    && service["metadata"]["builtin"] == true
                    && text(weight, "model_id").starts_with("ollama://"))
                .then(|| ollama_name(text(weight, "model_id")))
            });
            if let Some(native) = native {
                if !deployments.iter().any(|d| {
                    d["service_id"] == service["service_id"] && d["native_model"] == native
                }) {
                    deployments.push(json!({"service_id":service["service_id"],"native_model":native,"gateway_model":null,
                        "route_state":if offline.is_some(){"unavailable"}else{"undiscovered"},"scheduler_state":"unverified",
                        "configured_callable":false,"blockers":[if offline.is_some(){"UPSTREAM_UNAVAILABLE"}else{"GATEWAY_MODEL_NOT_DISCOVERED"}]}));
                }
            }
        }
        let blockers = if deployments.is_empty() {
            vec!["NO_LINKED_GATEWAY_SERVICE"]
        } else {
            vec![]
        };
        items.push(json!({"model_id":weight["model_id"],"name":weight["name"],"weight_state":weight["status"],
            "download_scope":weight["download_scope"],"selected_files":weight["files"],
            "deployments":deployments,"blockers":blockers}));
    }
    Ok(
        json!({"items":items,"observed_at":Utc::now(),"admission":"checked-at-request-time",
        "note":"A ready binding describes configuration; current capacity is checked atomically when inference starts."}),
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn weights_require_exact_backend_mapping() {
        let hf = json!({"model_id":"hf://org/weights"});
        let s = json!({"kind":"openai_compatible","metadata":{"model_bindings":{"served-name":"hf://org/weights"}}});
        assert!(linked(&hf, &s, "served-name"));
        assert!(!linked(&hf, &s, "org/weights"));
        assert!(linked(
            &json!({"model_id":"ollama://qwen"}),
            &json!({"kind":"ollama"}),
            "qwen:latest"
        ));
        assert!(!linked(
            &json!({"model_id":"ollama://qwen:2b"}),
            &json!({"kind":"ollama"}),
            "qwen:4b"
        ));
    }
}
