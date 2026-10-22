//! Administrator-calibrated per-model budgets for built-in Ollama.
//! Calibration replaces the automatic estimate only for the selected model;
//! the automatic executor remains available as the restore target.
use super::ollama::{
    OWNER, automatic_id, binding_path, default_cost, directory, node_budget, origin, save,
};
use super::*;
use std::path::PathBuf;

const ROLES: [&str; 3] = ["qa", "embedding", "rerank"];
const LIMIT_KEYS: [&str; 6] = [
    "max_tokens",
    "options.num_ctx",
    "options.num_thread",
    "options.num_gpu",
    "options.num_batch",
    "options.num_predict",
];

fn native(model: &str) -> Option<(&'static str, String)> {
    let rest = model.strip_prefix("v-scheduler/ollama-")?;
    let (role, name) = rest.split_once('/')?;
    let role = ROLES.iter().find(|candidate| **candidate == role)?;
    (!name.is_empty() && name.len() <= 512 && !name.chars().any(char::is_control))
        .then(|| (*role, name.to_string()))
}

fn storage_error() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "MODEL_BUDGET_STORAGE_UNAVAILABLE",
    )
}

fn sample_error() -> Error {
    Error(
        StatusCode::SERVICE_UNAVAILABLE,
        "RESOURCE_SAMPLE_NOT_READY",
    )
}

fn invalid_budget() -> Error {
    Error(StatusCode::UNPROCESSABLE_ENTITY, "INVALID_MODEL_BUDGET")
}

fn limits_default() -> Value {
    json!({
        "max_tokens": 4096,
        "options.num_ctx": 8192,
        "options.num_thread": 4,
        "options.num_gpu": 999,
        "options.num_batch": 512,
        "options.num_predict": 4096,
    })
}

fn calibration_limits(limits: Option<Value>) -> Api<Value> {
    let Some(limits) = limits else {
        return Ok(limits_default());
    };
    let Some(values) = limits.as_object() else {
        return Err(invalid_budget());
    };
    if values.len() != LIMIT_KEYS.len()
        || !values
            .keys()
            .all(|key| LIMIT_KEYS.contains(&key.as_str()))
    {
        return Err(invalid_budget());
    }
    let mut approved = serde_json::Map::new();
    for key in LIMIT_KEYS {
        let number = values
            .get(key)
            .and_then(Value::as_u64)
            .filter(|number| match key {
                "max_tokens" | "options.num_predict" => (1..=262144).contains(number),
                "options.num_ctx" => (1..=1048576).contains(number),
                "options.num_thread" => (1..=1024).contains(number),
                "options.num_gpu" => *number <= 9999,
                _ => (1..=65536).contains(number),
            })
            .ok_or_else(invalid_budget)?;
        approved.insert(key.into(), json!(number));
    }
    Ok(Value::Object(approved))
}

fn bindings(root: &std::path::Path) -> Vec<(String, PathBuf, Value)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(root.join("ollama-bindings")) else {
        return found;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if name.len() != 69
            || !name.ends_with(".json")
            || !name[..64].bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            continue;
        }
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        if raw.len() > 262144 {
            continue;
        }
        let Ok(config) = serde_json::from_slice::<Value>(&raw) else {
            continue;
        };
        let Some(models) = config["models"].as_object() else {
            continue;
        };
        if models.len() != 1 {
            continue;
        }
        let model = models.keys().next().cloned().unwrap_or_default();
        // Never trust a file name that does not match the model key it carries.
        if binding_path(root, &model) != path {
            continue;
        }
        found.push((model, path, config));
    }
    found.sort_by(|left, right| left.0.cmp(&right.0));
    found
}

pub(super) async fn list(State(a): State<App>, h: HeaderMap) -> Api<Json<Value>> {
    require(&auth(&a, &h).await?, "admin")?;
    let root = directory();
    let local_node = a
        .voice_config
        .snapshot()
        .local_agent
        .as_ref()
        .and_then(|local| local["node_id"].as_str().map(str::to_owned));
    let mut items = Vec::new();
    for (model, _path, config) in bindings(&root) {
        let Some((role, name)) = native(&model) else {
            continue;
        };
        let policy = &config["models"][&model];
        let direct = policy["mode"] == "direct";
        let executor = policy["executor_id"].as_str().unwrap_or("");
        if !direct && !executor.starts_with("builtin-ollama-") {
            continue;
        }
        let (node, enabled, version, updated_at, resources) = if direct {
            let Some(node) = local_node.clone() else {
                continue;
            };
            (node, true, 0, Value::Null, None)
        } else {
            let row = sqlx::query(
                "SELECT node_id,definition,state,version,updated_at FROM vrs_executor_settings WHERE id=$1",
            )
            .bind(executor)
            .fetch_optional(&a.db)
            .await
            .map_err(unavailable)?;
            let Some(row) = row else {
                continue;
            };
            let definition: Value = row.get("definition");
            let Ok(resources) = serde_json::from_value::<Resources>(definition["resources"].clone())
            else {
                continue;
            };
            (
                row.get("node_id"),
                row.get::<String, _>("state") == "enabled",
                row.get::<i64, _>("version"),
                json!(row.get::<DateTime<Utc>, _>("updated_at")),
                Some(resources),
            )
        };
        let calibrated = !direct && executor.starts_with("builtin-ollama-calibrated-");
        let bytes = policy["native_model_size_bytes"].as_i64();
        let (default_resources, capacity) = match node_budget(&a, &node).await {
            Ok(budget) => {
                let default = bytes.filter(|value| *value > 0).and_then(|bytes| {
                    default_cost(
                        &budget.capacity,
                        &budget.baseline,
                        &budget.reserve,
                        &budget.report,
                        bytes,
                    )
                    .ok()
                });
                (default, Some(budget.capacity))
            }
            Err(_) => (None, None),
        };
        items.push(json!({
            "model_id": model,
            "repository_model_id": format!("ollama://{name}"),
            "role": role,
            "native_model": name,
            "node_id": node,
            "executor_id": if direct { Value::Null } else { json!(executor) },
            "mode": if direct { "direct" } else if calibrated { "calibrated" } else { "automatic" },
            "calibrated": calibrated,
            "enabled": enabled,
            "version": version,
            "updated_at": updated_at,
            "resources": resources,
            "default_resources": default_resources,
            "limits": policy["limits"].clone(),
            "default_limits": limits_default(),
            "capacity": capacity,
        }));
    }
    Ok(ok(json!({"items": items})))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BudgetEdit {
    model_id: String,
    mode: Option<String>,
    resources: Option<Resources>,
    limits: Option<Value>,
}

fn valid(resources: &Resources) -> bool {
    resources.valid()
        && resources.cpu_millicores > 0
        && resources.memory_mib > 0
        && (resources.gpu_memory_mib == 0 || resources.gpu_slots > 0)
}

// `direct` intentionally skips scheduler admission; it is never a resource value.
fn direct_mode(b: &BudgetEdit) -> Api<bool> {
    match b.mode.as_deref() {
        None | Some("calibrated") => {
            if b.resources.as_ref().is_none_or(|resources| !valid(resources)) {
                return Err(invalid_budget());
            }
            Ok(false)
        }
        Some("direct") => {
            if b.resources.is_some() || b.limits.is_some() {
                return Err(invalid_budget());
            }
            Ok(true)
        }
        Some(_) => Err(invalid_budget()),
    }
}

pub(super) async fn calibrate(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<BudgetEdit>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let direct = direct_mode(&b)?;
    let Some((role, _)) = native(&b.model_id) else {
        return Err(invalid_budget());
    };
    let root = directory();
    let path = binding_path(&root, &b.model_id);
    let _guard = a.voice_config.management_lock.lock().await;
    let config: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|_| {
        Error(StatusCode::NOT_FOUND, "MODEL_BINDING_NOT_FOUND")
    })?)
    .map_err(|_| storage_error())?;
    let policy = config["models"].get(&b.model_id).cloned().ok_or_else(|| {
        Error(StatusCode::NOT_FOUND, "MODEL_BINDING_NOT_FOUND")
    })?;
    let old = policy["executor_id"].as_str().unwrap_or("");
    let direct_binding = policy["mode"] == "direct";
    if !direct_binding && !old.starts_with("builtin-ollama-") {
        return Err(Error(StatusCode::CONFLICT, "MODEL_BINDING_UNMANAGED"));
    }
    let upstream = origin(role);
    let (node, mut definition) = if direct_binding {
        if policy["upstream_url"] != upstream {
            return Err(Error(StatusCode::CONFLICT, "MODEL_BINDING_UNMANAGED"));
        }
        let node = a
            .voice_config
            .snapshot()
            .local_agent
            .as_ref()
            .and_then(|local| local["node_id"].as_str().map(str::to_owned))
            .ok_or_else(sample_error)?;
        let definition = json!({"id":"","contract_version":"1","task_type":"model.infer","node_id":node,"app_id":OWNER,
            "resources":Resources::default(),"upstream_urls":[upstream],"placements":[],"retry_mode":"none","max_attempts":1});
        (node, definition)
    } else {
        let row = sqlx::query("SELECT node_id,definition FROM vrs_executor_settings WHERE id=$1")
            .bind(old)
            .fetch_optional(&a.db)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Error(StatusCode::NOT_FOUND, "EXECUTOR_NOT_FOUND"))?;
        let node: String = row.get("node_id");
        let definition: Value = row.get("definition");
        if definition["app_id"] != OWNER || definition["task_type"] != "model.infer" {
            return Err(Error(StatusCode::CONFLICT, "MODEL_BINDING_UNMANAGED"));
        }
        let upstream = definition["upstream_urls"]
            .as_array()
            .and_then(|urls| urls.first())
            .and_then(Value::as_str)
            .ok_or_else(|| Error(StatusCode::CONFLICT, "MODEL_BINDING_UNMANAGED"))?;
        if upstream != origin(role) {
            return Err(Error(StatusCode::CONFLICT, "MODEL_BINDING_UNMANAGED"));
        }
        (node, definition)
    };
    if direct {
        if direct_binding {
            return Ok(ok(json!({
                "model_id": b.model_id,
                "mode": "direct",
                "calibrated": false,
                "resources": Value::Null,
            })));
        }
        if policy["native_model_size_bytes"]
            .as_i64()
            .filter(|bytes| *bytes > 0)
            .is_none()
        {
            return Err(sample_error());
        }
        let mut updated = config;
        let entry = updated["models"][&b.model_id]
            .as_object_mut()
            .ok_or_else(|| Error(StatusCode::NOT_FOUND, "MODEL_BINDING_NOT_FOUND"))?;
        entry.insert("mode".into(), json!("direct"));
        entry.remove("executor_id");
        entry.remove("limits");
        let mut tx = a.db.begin().await.map_err(unavailable)?;
        if old.starts_with("builtin-ollama-calibrated-") {
            sqlx::query("UPDATE vrs_executor_settings SET state='disabled',version=version+1,updated_at=now() WHERE id=$1")
                .bind(old)
                .execute(&mut *tx)
                .await
                .map_err(unavailable)?;
        }
        management::audit(&mut tx, &p, "model.budget.direct", &b.model_id).await?;
        tx.commit().await.map_err(unavailable)?;
        management::restore(&a.db, &a.voice_config)
            .await
            .map_err(|_| storage_error())?;
        save(&path, &updated).map_err(|_| storage_error())?;
        return Ok(ok(json!({
            "model_id": b.model_id,
            "mode": "direct",
            "calibrated": false,
            "resources": Value::Null,
        })));
    }
    let resources = b.resources.ok_or_else(invalid_budget)?;
    let limits = calibration_limits(b.limits)?;
    let node_row = sqlx::query("SELECT capacities,resident_baseline,management_enabled FROM vrs_nodes WHERE id=$1")
        .bind(&node)
        .fetch_one(&a.db)
        .await
        .map_err(unavailable)?;
    let capacity: Resources = serde_json::from_value(node_row.get("capacities")).map_err(|_| invalid_budget())?;
    let baseline: Resources = serde_json::from_value(node_row.get("resident_baseline")).map_err(|_| invalid_budget())?;
    if !node_row.get::<bool, _>("management_enabled") || !resources.fits(&baseline, &capacity) {
        return Err(invalid_budget());
    }
    let fingerprint = json!([
        b.model_id,
        node,
        resources,
        upstream,
        "calibrated-v1",
        policy["native_model_size_bytes"]
    ]);
    let digest = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&fingerprint).map_err(|_| invalid_budget())?)
    );
    let executor_id = format!("builtin-ollama-calibrated-{}", &digest[..24]);
    definition["id"] = json!(executor_id);
    definition["resources"] = json!(resources);
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    sqlx::query("INSERT INTO vrs_executor_settings(id,node_id,definition,state) VALUES($1,$2,$3,'enabled') ON CONFLICT(id) DO UPDATE SET definition=excluded.definition,state='enabled',version=vrs_executor_settings.version+1,updated_at=now()")
        .bind(&executor_id)
        .bind(&node)
        .bind(&definition)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    if old.starts_with("builtin-ollama-calibrated-") && old != executor_id {
        sqlx::query("UPDATE vrs_executor_settings SET state='disabled',version=version+1,updated_at=now() WHERE id=$1")
            .bind(old)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
    }
    management::audit(&mut tx, &p, "model.budget.calibrated", &b.model_id).await?;
    tx.commit().await.map_err(unavailable)?;
    management::restore(&a.db, &a.voice_config)
        .await
        .map_err(|_| storage_error())?;
    let mut updated = config;
    updated["models"][&b.model_id]["executor_id"] = json!(executor_id);
    updated["models"][&b.model_id]["mode"] = json!("permit");
    updated["models"][&b.model_id]["limits"] = limits;
    save(&path, &updated).map_err(|_| storage_error())?;
    Ok(ok(json!({
        "model_id": b.model_id,
        "executor_id": executor_id,
        "mode": "calibrated",
        "calibrated": true,
        "resources": resources,
    })))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct BudgetRestore {
    model_id: String,
}

pub(super) async fn restore(
    State(a): State<App>,
    h: HeaderMap,
    Json(b): Json<BudgetRestore>,
) -> Api<Json<Value>> {
    let p = auth(&a, &h).await?;
    require(&p, "admin")?;
    let role = native(&b.model_id)
        .map(|(role, _)| role)
        .ok_or_else(invalid_budget)?;
    let root = directory();
    let path = binding_path(&root, &b.model_id);
    let _guard = a.voice_config.management_lock.lock().await;
    let config: Value = serde_json::from_slice(&std::fs::read(&path).map_err(|_| {
        Error(StatusCode::NOT_FOUND, "MODEL_BINDING_NOT_FOUND")
    })?)
    .map_err(|_| storage_error())?;
    let policy = config["models"].get(&b.model_id).cloned().ok_or_else(|| {
        Error(StatusCode::NOT_FOUND, "MODEL_BINDING_NOT_FOUND")
    })?;
    let old = policy["executor_id"].as_str().unwrap_or("");
    let calibrated = old.starts_with("builtin-ollama-calibrated-");
    let direct = policy["mode"] == "direct";
    if !calibrated && !direct {
        return Ok(ok(json!({"model_id": b.model_id, "calibrated": false})));
    }
    if policy["upstream_url"] != origin(role) {
        return Err(Error(StatusCode::CONFLICT, "MODEL_BINDING_UNMANAGED"));
    }
    let bytes = policy["native_model_size_bytes"]
        .as_i64()
        .filter(|value| *value > 0)
        .ok_or_else(sample_error)?;
    let node: String = if calibrated {
        let row = sqlx::query("SELECT node_id,definition FROM vrs_executor_settings WHERE id=$1")
            .bind(old)
            .fetch_optional(&a.db)
            .await
            .map_err(unavailable)?
            .ok_or_else(|| Error(StatusCode::NOT_FOUND, "EXECUTOR_NOT_FOUND"))?;
        let definition: Value = row.get("definition");
        if definition["app_id"] != OWNER || definition["task_type"] != "model.infer" {
            return Err(Error(StatusCode::CONFLICT, "MODEL_BINDING_UNMANAGED"));
        }
        row.get("node_id")
    } else {
        a.voice_config
            .snapshot()
            .local_agent
            .as_ref()
            .and_then(|local| local["node_id"].as_str().map(str::to_owned))
            .ok_or_else(sample_error)?
    };
    let budget = node_budget(&a, &node).await.map_err(|_| sample_error())?;
    let cost = default_cost(
        &budget.capacity,
        &budget.baseline,
        &budget.reserve,
        &budget.report,
        bytes,
    )
    .map_err(|_| sample_error())?;
    let upstream = origin(role);
    let executor_id = automatic_id(&b.model_id, &node, &cost, &upstream, bytes)
        .map_err(|_| storage_error())?;
    let definition = json!({
        "id": executor_id,
        "contract_version": "1",
        "task_type": "model.infer",
        "node_id": node,
        "app_id": OWNER,
        "resources": cost,
        "upstream_urls": [upstream],
        "placements": [],
        "retry_mode": "none",
        "max_attempts": 1,
    });
    let mut tx = a.db.begin().await.map_err(unavailable)?;
    sqlx::query("INSERT INTO vrs_executor_settings(id,node_id,definition,state) VALUES($1,$2,$3,'enabled') ON CONFLICT(id) DO UPDATE SET definition=excluded.definition,state='enabled',version=vrs_executor_settings.version+1,updated_at=now()")
        .bind(&executor_id)
        .bind(&node)
        .bind(&definition)
        .execute(&mut *tx)
        .await
        .map_err(unavailable)?;
    if calibrated {
        sqlx::query("UPDATE vrs_executor_settings SET state='disabled',version=version+1,updated_at=now() WHERE id=$1")
            .bind(old)
            .execute(&mut *tx)
            .await
            .map_err(unavailable)?;
    }
    management::audit(&mut tx, &p, "model.budget.restored", &b.model_id).await?;
    tx.commit().await.map_err(unavailable)?;
    management::restore(&a.db, &a.voice_config)
        .await
        .map_err(|_| storage_error())?;
    let mut updated = config;
    let entry = updated["models"][&b.model_id]
        .as_object_mut()
        .ok_or_else(|| Error(StatusCode::NOT_FOUND, "MODEL_BINDING_NOT_FOUND"))?;
    entry.insert("mode".into(), json!("permit"));
    entry.insert("executor_id".into(), json!(executor_id));
    entry.remove("limits");
    save(&path, &updated).map_err(|_| storage_error())?;
    Ok(ok(json!({
        "model_id": b.model_id,
        "executor_id": executor_id,
        "calibrated": false,
        "resources": cost,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_keys_map_to_owned_ollama_roles_only() {
        assert_eq!(
            native("v-scheduler/ollama-qa/qwen3.5:2b"),
            Some(("qa", "qwen3.5:2b".into()))
        );
        assert_eq!(
            native("v-scheduler/ollama-rerank/qllama/bge-reranker-v2-m3:q8_0"),
            Some(("rerank", "qllama/bge-reranker-v2-m3:q8_0".into()))
        );
        assert_eq!(native("v-scheduler/ollama-other/model"), None);
        assert_eq!(native("v-scheduler/ollama-qa/"), None);
        assert_eq!(native("other/ollama-qa/model"), None);
    }
    #[test]
    fn calibration_accepts_only_positive_compute_budgets() {
        assert!(valid(&Resources {
            cpu_millicores: 1000,
            memory_mib: 512,
            gpu_slots: 1,
            gpu_memory_mib: 0,
        }));
        assert!(!valid(&Resources::default()));
        assert!(!valid(&Resources {
            cpu_millicores: 1000,
            memory_mib: 512,
            gpu_slots: 0,
            gpu_memory_mib: 512,
        }));
    }
    #[test]
    fn direct_mode_never_carries_a_resource_budget() {
        let direct = BudgetEdit {
            model_id: "v-scheduler/ollama-qa/model".into(),
            mode: Some("direct".into()),
            resources: None,
            limits: None,
        };
        assert!(direct_mode(&direct).is_ok_and(|direct| direct));
        assert!(direct_mode(&BudgetEdit {
            resources: Some(Resources {
                cpu_millicores: 1000,
                memory_mib: 512,
                ..Resources::default()
            }),
            ..direct
        })
        .is_err());
        assert!(direct_mode(&BudgetEdit {
            model_id: "v-scheduler/ollama-qa/model".into(),
            mode: Some("direct".into()),
            resources: None,
            limits: Some(limits_default()),
        })
        .is_err());
        assert!(direct_mode(&BudgetEdit {
            model_id: "v-scheduler/ollama-qa/model".into(),
            mode: Some("calibrated".into()),
            resources: None,
            limits: None,
        })
        .is_err());
        assert!(direct_mode(&BudgetEdit {
            model_id: "v-scheduler/ollama-qa/model".into(),
            mode: Some("unbounded".into()),
            resources: None,
            limits: None,
        })
        .is_err());
        assert!(!direct_mode(&BudgetEdit {
            model_id: "v-scheduler/ollama-qa/model".into(),
            mode: None,
            resources: Some(Resources {
                cpu_millicores: 1000,
                memory_mib: 512,
                ..Resources::default()
            }),
            limits: None,
        })
        .is_ok_and(|direct| direct));
    }
    #[test]
    fn calibrated_limits_are_complete_and_bounded() {
        let defaults = calibration_limits(None).ok().unwrap();
        assert_eq!(
            defaults
                .as_object()
                .map(|limits| limits.len())
                .unwrap_or_default(),
            LIMIT_KEYS.len()
        );
        assert_eq!(defaults["options.num_ctx"], json!(8192));
        let mut edited = defaults.clone();
        edited["options.num_ctx"] = json!(16384);
        assert_eq!(
            calibration_limits(Some(edited))
                .ok()
                .unwrap()["options.num_ctx"],
            json!(16384)
        );
        let mut incomplete = defaults.clone();
        incomplete
            .as_object_mut()
            .unwrap()
            .remove("options.num_ctx");
        assert!(calibration_limits(Some(incomplete)).is_err());
        let mut zero = defaults.clone();
        zero["max_tokens"] = json!(0);
        assert!(calibration_limits(Some(zero)).is_err());
        let mut unknown = defaults.clone();
        unknown["options.num_keep_alive"] = json!(1);
        assert!(calibration_limits(Some(unknown)).is_err());
    }
}
