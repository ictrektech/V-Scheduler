use super::{
    Hub,
    download::{env, number},
    storage::*,
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::time::{Duration, Instant};

pub fn runtime(id: &str) -> Result<Value> {
    let (role, model, slots, context) = match id {
        "qa" => ("generate", "qwen3.5:2b", 8, 24576),
        "embedding" => ("embedding", "bge-m3", 4, 8192),
        "rerank" => ("rerank", "qllama/bge-reranker-v2-m3:q8_0", 2, 8192),
        _ => anyhow::bail!("unknown runtime"),
    };
    let upper = id.to_uppercase();
    let base = env(
        &format!("OLLAMA_{upper}_API_URL"),
        &format!("http://v-scheduler-ollama-{id}:11434"),
    );
    Ok(
        json!({"id":id,"service_id":format!("v-scheduler-ollama-{id}"),"name":format!("Ollama {upper}"),"kind":"ollama","role":role,"base_url":base,"api_url":base,"gateway_url":env(&format!("OLLAMA_{upper}_GATEWAY_URL"),&format!("http://v-scheduler-ollama-{id}:11535")),"preload_model":env(&format!("OLLAMA_{upper}_MODEL"),model),"num_parallel":number(&format!("OLLAMA_{upper}_NUM_PARALLEL"),slots),"context_length":number(if id=="qa" {"OLLAMA_CONTEXT_LENGTH"} else if id=="embedding" {"OLLAMA_EMBEDDING_CONTEXT_LENGTH"} else {"OLLAMA_RERANK_CONTEXT_LENGTH"},context),"keep_alive":env("OLLAMA_KEEP_ALIVE","-1m"),"api_contract":ollama_contract(),"metadata":{"builtin":true}}),
    )
}
pub fn ollama_contract() -> Value {
    json!({"health_path":"/api/version","version_path":"/api/version","models_path":"/api/tags","running_models_path":"/api/ps","runtime_status_path":"/api/ps","start_model_path":"/api/generate","stop_model_path":"/api/generate"})
}
pub fn validate_registration(mut value: Value) -> Result<Value> {
    for key in ["service_id", "name", "base_url"] {
        ensure!(
            value[key].as_str().is_some_and(|x| !x.trim().is_empty()),
            "missing service field {key}"
        );
    }
    for key in ["base_url", "gateway_url"] {
        if let Some(url) = value[key].as_str() {
            let parsed = reqwest::Url::parse(url)?;
            ensure!(
                matches!(parsed.scheme(), "http" | "https")
                    && parsed.username().is_empty()
                    && parsed.password().is_none()
                    && parsed.query().is_none()
                    && parsed.fragment().is_none(),
                "invalid service URL"
            );
        }
    }
    let kind = value["kind"].as_str().unwrap_or("custom").to_owned();
    value["kind"] = json!(kind);
    ensure!(
        ["custom", "ollama", "openai_compatible"].contains(&kind.as_str()),
        "unknown service kind"
    );
    if kind == "ollama" {
        let mut default = ollama_contract();
        if let Some(contract) = value["api_contract"].as_object() {
            for (k, v) in contract {
                if !v.is_null() {
                    default[k] = v.clone();
                }
            }
        }
        value["api_contract"] = default;
    } else {
        ensure!(
            value["api_contract"].is_object(),
            "custom/OpenAI service requires api_contract"
        );
    }
    if let Some(contract) = value["api_contract"].as_object() {
        for (_, path) in contract {
            if let Some(path) = path.as_str() {
                ensure!(
                    path.starts_with('/') && !path.starts_with("//") && !path.contains("://"),
                    "contract paths must be relative to service"
                );
            }
        }
    }
    if value["metadata"].is_null() {
        value["metadata"] = json!({});
    }
    ensure!(value["metadata"].is_object(), "invalid service metadata");
    if let Some(links) = value["metadata"].get("model_bindings") {
        let links = links
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("invalid model_bindings"))?;
        ensure!(
            links.len() <= 256
                && links.iter().all(|(name, id)| !name.is_empty()
                    && name.len() <= 512
                    && id
                        .as_str()
                        .is_some_and(|id| !id.is_empty() && id.len() <= 1024)),
            "invalid model_bindings"
        );
    }
    value["updated_at"] = json!(now());
    Ok(value)
}
pub fn services(hub: &Hub, include_builtin: bool) -> Result<Vec<Value>> {
    let mut items: Vec<Value> = hub
        .store
        .entries("inference_services")?
        .as_object()
        .unwrap()
        .iter()
        .filter(|(id, _)| {
            include_builtin
                || !(id.starts_with("model-hub-ollama-") || id.starts_with("v-scheduler-ollama-"))
        })
        .map(|(id, v)| {
            let mut v = v.clone();
            v["service_id"] = json!(id);
            v
        })
        .collect();
    if include_builtin {
        for id in ["qa", "embedding", "rerank"] {
            let value = runtime(id)?;
            if !items.iter().any(|v| v["service_id"] == value["service_id"]) {
                items.push(value);
            }
        }
    }
    Ok(items)
}
pub fn service(hub: &Hub, id: &str) -> Result<Value> {
    if let Some(role) = id.strip_prefix("v-scheduler-ollama-") {
        return runtime(role);
    }
    let value = hub.store.entries("inference_services")?[id].clone();
    ensure!(!value.is_null(), "service not found");
    Ok(value)
}
async fn get(hub: &Hub, base: &str, path: &str) -> Result<Value> {
    let response = hub
        .client
        .get(format!("{}{}", base.trim_end_matches('/'), path))
        .timeout(Duration::from_secs(20))
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "service returned HTTP {}",
        response.status()
    );
    if response.content_length() == Some(0) {
        return Ok(json!({}));
    }
    Ok(response.json().await?)
}
fn model_items(data: &Value) -> Vec<Value> {
    if let Some(array) = data.as_array() {
        return array.clone();
    }
    for key in ["models", "running_models", "items", "data"] {
        if let Some(array) = data[key].as_array() {
            return array.clone();
        }
    }
    vec![]
}
fn name(item: &Value) -> String {
    item.as_str()
        .or(item["name"].as_str())
        .or(item["model"].as_str())
        .or(item["id"].as_str())
        .unwrap_or("")
        .to_owned()
}
fn item_running(item: &Value, default: bool) -> Option<bool> {
    if let Some(value) = item["running"].as_bool() {
        return Some(value);
    }
    let status = item["state"]
        .as_str()
        .or(item["status"].as_str())
        .or(item["phase"].as_str())
        .unwrap_or("")
        .to_lowercase();
    match status.as_str() {
        "running" | "started" | "ready" | "inferencing" | "prefill" | "decode" | "embedding" => {
            Some(true)
        }
        "stopped" | "stopping" | "unloaded" | "not_running" | "not-running" | "idle" | "failed" => {
            Some(false)
        }
        _ => {
            if default {
                Some(true)
            } else {
                None
            }
        }
    }
}
pub async fn inspect(hub: &Hub, service: &Value) -> Value {
    let base = service["base_url"].as_str().unwrap_or("");
    let contract = service["api_contract"].clone();
    let ollama = service["kind"] == "ollama";
    let mut result = service.clone();
    let mut errors = vec![];
    let mut running_observed = false;
    let health = hub
        .client
        .get(format!(
            "{}{}",
            base.trim_end_matches('/'),
            contract["health_path"].as_str().unwrap_or("/health")
        ))
        .timeout(Duration::from_secs(10))
        .send()
        .await;
    result["healthy"] = json!(health.is_ok_and(|r| r.status().is_success()));
    result["online"] = result["healthy"].clone();
    for (field, key) in [
        ("version", "version_path"),
        ("models", "models_path"),
        ("running_models", "running_models_path"),
        ("runtime_status", "runtime_status_path"),
        ("metrics", "metrics_path"),
    ] {
        if let Some(path) = contract[key].as_str() {
            match get(hub, base, path).await {
                Ok(value) => {
                    if field == "running_models" {
                        running_observed = true;
                    }
                    result[field] = value;
                }
                Err(_) => {
                    errors.push(format!("{field} unavailable"));
                    result[field] = json!({});
                }
            }
        }
    }
    if ollama {
        if let Some(gateway) = service["gateway_url"].as_str() {
            result["metrics"] = get(hub, gateway, "/metrics").await.unwrap_or(json!({}));
        }
    }
    let models = model_items(&result["models"]);
    let mut running = model_items(&result["running_models"]);
    if !ollama {
        for key in ["runtime_status", "metrics"] {
            running.extend(model_items(&result[key]));
        }
    }
    let metrics = result["metrics"].clone();
    let mut normalized = vec![];
    let mut candidates = models.clone();
    for item in &running {
        if !candidates.iter().any(|v| name(v) == name(item)) {
            candidates.push(item.clone());
        }
    }
    for item in candidates {
        let model = name(&item);
        if model.is_empty() {
            continue;
        }
        let mut value = if item.is_object() { item } else { json!({}) };
        let live = running.iter().find(|v| name(v) == model);
        value["name"] = json!(model);
        value["model"] = json!(model);
        value["running"] = if ollama {
            json!(live.is_some())
        } else {
            item_running(live.unwrap_or(&value), live.is_some())
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
        };
        if let Some(live) = live {
            if let Some(fields) = live.as_object() {
                for (k, v) in fields {
                    value[k] = v.clone();
                }
            }
        }
        if !ollama {
            value["running"] = item_running(&value, live.is_some())
                .map(|v| json!(v))
                .unwrap_or(Value::Null);
        }
        value["runtime_metrics"] = metrics["model_metrics"][&model].clone();
        for key in [
            "phase",
            "active_slots",
            "total_slots",
            "slot_usage",
            "tokens_per_second",
            "prefill_tokens_per_second",
            "decode_tokens_per_second",
        ] {
            if !metrics["model_metrics"][&model][key].is_null() {
                value[key] = metrics["model_metrics"][&model][key].clone();
            }
        }
        if value["active_slots"].is_null() {
            value["active_slots"] = json!(metrics["active_by_model"][&model].as_u64().unwrap_or(0));
        }
        if value["total_slots"].is_null() {
            value["total_slots"] = metrics["total_slots"]
                .as_u64()
                .or(service["num_parallel"].as_u64())
                .or(service["metadata"]["num_parallel"].as_u64())
                .map(|x| json!(x))
                .unwrap_or(Value::Null);
        }
        if value["phase"].is_null() {
            value["phase"] = json!(if live.is_some() { "idle" } else { "downloaded" });
        }
        normalized.push(value);
    }
    result["running_state_observed"] = json!(running_observed);
    result["models"] = json!(normalized);
    result["running_models"] = json!(running);
    result["error"] = if errors.is_empty() {
        Value::Null
    } else {
        json!(errors.join("; "))
    };
    result["active_slots"] = metrics["active_slots"].clone();
    result["total_slots"] = metrics["total_slots"]
        .as_u64()
        .or(service["num_parallel"].as_u64())
        .map(|x| json!(x))
        .unwrap_or(Value::Null);
    result
}
pub async fn action(hub: &Hub, service: &Value, request: Value, start: bool) -> Result<Value> {
    let model = request["model"]
        .as_str()
        .filter(|x| !x.is_empty())
        .ok_or_else(|| anyhow::anyhow!("model is required"))?;
    if service["metadata"]["builtin"] == true
        && std::env::var("V_ROUTER_ADDR").is_ok_and(|v| !v.is_empty())
    {
        let role = service["service_id"]
            .as_str()
            .and_then(|id| id.strip_prefix("v-scheduler-ollama-"))
            .ok_or_else(|| anyhow::anyhow!("invalid built-in service"))?;
        return crate::router::builtin_model_action(role, model, start).await;
    }
    let ollama = service["kind"] == "ollama";
    let role = request["mode"]
        .as_str()
        .or(service["role"].as_str())
        .unwrap_or("generate");
    let mut body = request.clone();
    let path;
    if ollama {
        let keep = if start {
            if request["keep_alive"].is_null() {
                json!(env("OLLAMA_KEEP_ALIVE", "-1m"))
            } else {
                request["keep_alive"].clone()
            }
        } else {
            json!(0)
        };
        if start && role == "rerank" {
            path = "/v1/rerank";
            body = json!({"model":model,"query":"warmup","documents":["warmup"],"keep_alive":keep});
        } else if role == "embedding" {
            path = "/api/embed";
            body = json!({"model":model,"input":if start {"warmup"} else {""},"keep_alive":keep});
        } else {
            path = "/api/generate";
            body = json!({"model":model,"prompt":if start {"warmup"} else {""},"stream":false,"keep_alive":keep});
        }
    } else {
        path = service["api_contract"][if start {
            "start_model_path"
        } else {
            "stop_model_path"
        }]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("service does not expose model action"))?;
        if let Some(map) = body.as_object_mut() {
            map.remove("confirm_timeout_seconds");
        }
    }
    let response = hub
        .client
        .post(format!(
            "{}{}",
            service["base_url"].as_str().unwrap().trim_end_matches('/'),
            path
        ))
        .timeout(Duration::from_secs(300))
        .json(&body)
        .send()
        .await?;
    ensure!(
        response.status().is_success(),
        "model action returned HTTP {}",
        response.status()
    );
    let raw = response.json::<Value>().await.unwrap_or(json!({}));
    let timeout = request["confirm_timeout_seconds"].as_f64().unwrap_or(8.);
    ensure!(
        (0. ..=60.).contains(&timeout),
        "confirmation timeout must be 0..60"
    );
    let mut confirmed = Value::Null;
    let mut observed = Value::Null;
    let deadline = Instant::now() + Duration::from_secs_f64(timeout);
    if timeout > 0. {
        confirmed = json!(false);
        loop {
            if let Some(status_path) = service["api_contract"]["model_action_status_path"].as_str()
            {
                let response = hub
                    .client
                    .get(format!(
                        "{}{}",
                        service["base_url"].as_str().unwrap().trim_end_matches('/'),
                        status_path
                    ))
                    .query(&[
                        ("model", model),
                        ("action", if start { "start" } else { "stop" }),
                    ])
                    .timeout(Duration::from_secs(5))
                    .send()
                    .await;
                if let Ok(response) = response {
                    if response.status().is_success() {
                        if let Ok(value) = response.json::<Value>().await {
                            if value["confirmed"] == true {
                                confirmed = json!(true);
                                break;
                            }
                        }
                    }
                }
            }
            let state = inspect(hub, service).await;
            if state["healthy"] == true && state["running_state_observed"] == true {
                let found = state["models"].as_array().into_iter().flatten().any(|m| {
                    (name(m) == model || name(m) == format!("{model}:latest"))
                        && m["running"] == true
                });
                observed = json!(found);
                if found == start {
                    confirmed = json!(true);
                    break;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }
    let mut result = json!({"service_id":service["service_id"],"model":model,"accepted":true,"confirmed":confirmed,"observed_running":observed,"target_state":if start {"running"} else {"stopped"},"result":raw});
    result[if start { "started" } else { "stopped" }] = json!(confirmed == true);
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registration_preserves_custom_contract_and_normalizes_ollama() {
        let value=validate_registration(json!({"service_id":"external","name":"External","kind":"ollama","base_url":"http://ollama:11434","api_contract":{"metrics_path":"/metrics"}})).unwrap();
        assert_eq!(value["api_contract"]["models_path"], "/api/tags");
        assert_eq!(value["api_contract"]["metrics_path"], "/metrics");
        assert!(validate_registration(json!({"service_id":"external","name":"External","kind":"custom","base_url":"http://service:8000"})).is_err());
        assert!(validate_registration(json!({"service_id":"external","name":"External","kind":"ollama","base_url":"http://user:secret@service:8000"})).is_err());
    }
    #[test]
    fn roles_and_preload_defaults_remain_distinct() {
        for (id, role, slots) in [
            ("qa", "generate", 8),
            ("embedding", "embedding", 4),
            ("rerank", "rerank", 2),
        ] {
            let value = runtime(id).unwrap();
            assert_eq!(value["role"], role);
            assert_eq!(value["num_parallel"], slots);
            assert!(
                value["base_url"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("v-scheduler-ollama-{id}:11434"))
            );
        }
    }
}
