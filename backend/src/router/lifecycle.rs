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
    let (weights, services, tasks) = tokio::try_join!(
        hub(g, "/api/v1/models?status=all"),
        hub(g, "/api/v1/inference-services?include_builtin=true"),
        hub(g, "/api/v1/tasks")
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
                        .filter(|m| m.visible(u, id))
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
    // Ollama /api/ps reports residency without loading weights. Read each
    // built-in service once per snapshot, including when several models share it.
    let running = futures_util::future::join_all(
        services.as_array().into_iter().flatten()
            .filter(|s| s["metadata"]["builtin"] == true && s["kind"] == "ollama")
            .map(|service| async move {
                let result = tokio::time::timeout(Duration::from_secs(2), async {
                    let response = g.client.get(format!("{}/api/ps", text(service,"base_url").trim_end_matches('/')))
                        .send().await.ok()?.error_for_status().ok()?;
                    let data = read_json(response, 1 << 20).await.ok()?;
                    Some(data["models"].as_array()?.iter().filter_map(|m| {
                        m["name"].as_str().or(m["model"].as_str()).map(ollama_name)
                    }).collect::<std::collections::HashSet<_>>())
                }).await.ok().flatten();
                (text(service, "service_id"), result)
            })
    ).await.into_iter().collect::<std::collections::HashMap<_,_>>();
    let mut items = Vec::new();
    let mut linked_models = std::collections::HashSet::new();
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
                linked_models.insert((&u.id, &m.id));
                let model_running = running.get(text(service, "service_id"))
                    .and_then(Option::as_ref).map(|names| names.contains(&ollama_name(&m.id)));
                deployments.push(json!({"service_id":service["service_id"],"upstream_id":u.id,"gateway_model":m.qualified,
                    "native_model":m.id,"endpoints":m.endpoints,"route_state":status["state"],"scheduler_state":binding,
                    "model_running":model_running,"configured_callable":blockers.is_empty(),"blockers":blockers}));
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
        let download = tasks.as_array().into_iter().flatten()
            .find(|task| task["model_id"] == weight["model_id"])
            .map(|task| json!({"phase":task["phase"],"progress":task["progress"]}));
        items.push(json!({"model_id":weight["model_id"],"name":weight["name"],"weight_state":weight["status"],"download":download,
            "download_scope":weight["download_scope"],"selected_files":weight["files"],
            "deployments":deployments,"blockers":blockers}));
    }
    let upstream_models: Vec<_> = catalog.iter()
        .filter(|(u, m, _)| !linked_models.contains(&(&u.id, &m.id)) && m.kind != "service")
        .map(|(u, m, status)| json!({
            "upstream_id":u.id,"native_model":m.id,"gateway_model":m.qualified,"name":m.display_name(u,id),
            "weight_source":if ["v-scheduler","model-hub"].contains(&u.source.as_str())
                || services.as_array().into_iter().flatten().any(|service|same_origin(service,u))
                {"unlinked"} else {"upstream"},
            "service_state":status["state"]
        }))
        .collect();
    Ok(
        json!({"items":items,"upstream_models":upstream_models,"observed_at":Utc::now(),"admission":"checked-at-request-time",
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
