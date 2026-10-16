//! V-Sub root-broker bindings. A namespace comes only from the installed app's
//! authenticated live-worker discovery, never a public registration request.
use super::app_bindings::{
    VSUB, bounded_json, bounded_vos_installation, hardware, persist, read_private, save_private,
};
use super::*;
use std::{path::Path, time::Duration};
const APP: &str = "com.ictrek.v-sub";
const GATEWAY_APP: &str = "v-scheduler.app-gateway.v-sub";
const ORIGIN: &str = "http://v-sub-backend:7090";
const CONTROLLER: &str = "http://v-router:8080/scheduler/api/v1";
fn hex(value: &str, length: usize) -> bool {
    value.len() == length
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
fn model(row: &Value) -> anyhow::Result<(String, String, String, String)> {
    let namespace = row["namespace"]
        .as_str()
        .context("full V-Sub namespace required")?;
    let generation = row["generation"]
        .as_str()
        .context("live worker generation required")?;
    ensure!(
        hex(namespace, 64) && hex(generation, 32),
        "invalid namespace or worker generation"
    );
    let id = row["id"]
        .as_str()
        .context("private model identity required")?;
    let native = id
        .strip_prefix(&format!("v-sub-{}/", &namespace[..12]))
        .context("model namespace prefix mismatch")?;
    ensure!(
        !native.is_empty()
            && id.len() <= 512
            && !id.chars().any(char::is_control)
            && row["native_id"] == native
            && row["task_type"] == "model.infer",
        "private model declaration mismatch"
    );
    let upstream = format!("{ORIGIN}/inference/{namespace}/v1");
    ensure!(
        row["upstream_url"] == upstream,
        "private upstream origin mismatch"
    );
    Ok((
        id.into(),
        native.into(),
        namespace.into(),
        generation.into(),
    ))
}
fn credential(old: Option<&Value>, section: &str) -> String {
    old.and_then(|s| s[section]["token"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()))
}
fn build(
    node: &str,
    capacity: &Resources,
    baseline: &Resources,
    report: &Value,
    profile: &str,
    discovery: &Value,
    approvals: &Value,
    old: Option<&Value>,
) -> anyhow::Result<Value> {
    ensure!(
        approvals["protocol_version"] == 1,
        "unsupported V-Sub calibration format"
    );
    let epoch = approvals["credential_epoch"]
        .as_u64()
        .context("V-Sub credential epoch required")?;
    ensure!(
        old.is_none_or(|s| epoch >= s["credential_epoch"].as_u64().unwrap_or(0)),
        "credential epoch cannot roll back"
    );
    let previous = old.filter(|s| s["credential_epoch"] == epoch);
    let token = credential(previous, "binding");
    let gateway_token = credential(previous, "gateway");
    let mut models = serde_json::Map::new();
    let mut gateway_executors = serde_json::Map::new();
    let mut namespaces = serde_json::Map::new();
    let mut generations = serde_json::Map::new();
    let mut upstreams = serde_json::Map::new();
    let mut limits = serde_json::Map::new();
    let mut settings = serde_json::Map::new();
    let mut gateway_namespaces = serde_json::Map::new();
    let mut executors = Vec::new();
    let mut gateway_ids = Vec::new();
    let mut rows = discovery["models"]
        .as_array()
        .context("live worker model snapshot required")?
        .clone();
    ensure!(rows.len() <= 512, "V-Sub model snapshot too large");
    rows.sort_by_key(Value::to_string);
    let mut seen = std::collections::HashSet::new();
    for row in rows {
        let (id, native, namespace, generation) = model(&row)?;
        ensure!(seen.insert(id.clone()), "duplicate private model identity");
        let matches = approvals["approvals"]
            .as_array()
            .context("V-Sub calibrations required")?
            .iter()
            .filter(|t| {
                t["architecture"].as_str() == Some(std::env::consts::ARCH)
                    && t["profile"] == profile
                    && t["hardware"] == hardware(report)
                    && t["native_model"] == native
            })
            .collect::<Vec<_>>();
        ensure!(matches.len() <= 1, "ambiguous private model calibration");
        let Some(approval) = matches.first() else {
            continue;
        };
        let resources: Resources = serde_json::from_value(approval["resources"].clone())?;
        ensure!(
            resources.valid()
                && resources.cpu_millicores > 0
                && resources.memory_mib > 0
                && (resources.gpu_memory_mib == 0 || resources.gpu_slots > 0),
            "invalid private model resources"
        );
        let evidence = &approval["evidence"];
        ensure!(
            evidence["id"].as_str().is_some_and(|s| !s.is_empty())
                && evidence["confirmed_stopped"] == true
                && evidence["peak_memory_mib"]
                    .as_i64()
                    .is_some_and(|n| n > 0 && n <= resources.memory_mib)
                && evidence["observed_peak_cpu_millicores"]
                    .as_i64()
                    .is_some_and(|n| n > 0 && n <= resources.cpu_millicores),
            "measured private model peak and stop evidence required"
        );
        if !resources.fits(baseline, capacity) {
            continue;
        }
        let max_input = approval["max_input_bytes"]
            .as_u64()
            .context("private input byte bound required")?;
        ensure!(
            (1..=33554432).contains(&max_input)
                && approval["model_limits"]
                    .as_object()
                    .is_some_and(|v| !v.is_empty()
                        && v.values()
                            .all(|x| x.as_f64().is_some_and(|n| n.is_finite() && n >= 0.0)))
                && approval["execution_settings"]
                    .as_object()
                    .is_some_and(|s| !s.is_empty())
                && approval["execution_settings"]["profile"] == profile,
            "private runtime/input calibration required"
        );
        let fingerprint = json!([node, namespace, generation, approval]);
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&fingerprint)?));
        let own = format!("app-auto-v-sub-{}", &digest[..24]);
        let gateway = format!("app-gateway-v-sub-{}", &digest[..24]);
        let upstream = format!("{ORIGIN}/inference/{namespace}/v1");
        models.insert(id.clone(), json!(own));
        gateway_executors.insert(id.clone(), json!(gateway));
        namespaces.insert(id.clone(), json!(namespace));
        generations.insert(id.clone(), json!(generation));
        upstreams.insert(id.clone(), json!(upstream));
        limits.insert(id.clone(), approval["model_limits"].clone());
        settings.insert(id.clone(), approval["execution_settings"].clone());
        for (executor, app) in [(&own, APP), (&gateway, GATEWAY_APP)] {
            executors.push(json!({"id":executor,"app_id":app,"node_id":node,"contract_version":"1","task_type":"model.infer","resources":resources,"upstream_urls":[upstream],"placements":[],"retry_mode":"none","max_attempts":1}));
        }
        gateway_ids.push(gateway.clone());
        let cfg=gateway_namespaces.entry(namespace.clone()).or_insert_with(||json!({"url":CONTROLLER,"token":gateway_token,"require_model_policy":true,"models":{}}));
        cfg["models"][&id] = json!({"mode":"permit","executor_id":gateway,"completion":"synchronous","upstream_url":upstream,"max_input_bytes":max_input,"limits":approval["limits"].as_object().cloned().unwrap_or_default()});
    }
    let mut state = json!({"credential_epoch":epoch,"binding":{"protocol_version":1,"binding_revision":1,"app_id":APP,"url":CONTROLLER,"token":token,"models":models,"gateway_executors":gateway_executors,"namespaces":namespaces,"model_generations":generations,"upstreams":upstreams,"model_limits":limits,"model_execution_settings":settings},"principals":[{"token":token,"role":"submitter","app_id":APP,"permit_executors":gateway_ids},{"token":gateway_token,"role":"submitter","app_id":GATEWAY_APP}],"executors":executors,"adapters":[],"gateway":{"token":gateway_token},"gateway_snapshot":{"protocol_version":1,"app_id":APP,"namespaces":gateway_namespaces}});
    if let Some(old) = old {
        let revision = old["binding"]["binding_revision"]
            .as_u64()
            .context("private binding revision required")?;
        state["binding"]["binding_revision"] = json!(revision);
        if state != *old {
            state["binding"]["binding_revision"] = json!(
                revision
                    .checked_add(1)
                    .context("private binding revision exhausted")?
            );
        }
    }
    ensure!(
        serde_json::to_vec(&state)?.len() <= 262144,
        "private binding exceeds size bound"
    );
    Ok(state)
}
async fn reconcile(
    shared: &SharedApp,
    configuration: &Path,
    client: &reqwest::Client,
    approvals: &Value,
) -> anyhow::Result<()> {
    let directory = configuration
        .parent()
        .context("configuration directory required")?;
    let path = directory.join("app-bindings/v-sub.json");
    let old = if path.exists() {
        Some(read_private(&path)?)
    } else {
        None
    };
    let context = std::fs::read_to_string("/var/lib/vos-context/access-token")
        .context("V-Sub phase: context read")?;
    let vos = crate::identity::base().context("VOS identity endpoint required")?;
    let identity = bounded_json(
        client
            .get(format!(
                "{}/v1000/oauth2/userinfo",
                vos.trim_end_matches('/')
            ))
            .bearer_auth(context.trim())
            .send()
            .await
            .context("V-Sub phase: identity HTTP")?,
    )
    .await
    .context("V-Sub phase: identity response")?;
    ensure!(
        identity["sub"] == "com.ictrek.v-scheduler" && identity["enable"] == 1,
        "Scheduler application identity required"
    );
    let installed = bounded_vos_installation(
        client
            .get(format!(
                "{}/v1000/app/detail?app_id={APP}",
                vos.trim_end_matches('/')
            ))
            .bearer_auth(context.trim())
            .send()
            .await
            .context("V-Sub phase: installation HTTP")?,
    )
    .await
    .context("V-Sub phase: installation response")?;
    let missing = installed["code"] == 10067
        || (installed["code"] == 0 && installed["data"]["state"] != "installed");
    if missing && old.is_none() {
        return Ok(());
    }
    ensure!(
        missing || installed["code"] == 0,
        "V-Sub installation check unavailable"
    );
    let discovery = if missing {
        json!({"models":[]})
    } else {
        let value = bounded_json(
            client
                .get(format!("{ORIGIN}/api/v1/scheduler/discovery"))
                .bearer_auth(context.trim())
                .send()
                .await
                .context("V-Sub phase: discovery HTTP")?,
        )
        .await
        .context("V-Sub phase: discovery response")?;
        ensure!(
            value["protocol_version"] == 1
                && value["app_id"] == APP
                && value["binding_endpoint"] == "/api/v1/scheduler/binding"
                && value["capabilities"]
                    .as_array()
                    .is_some_and(|v| v.iter().any(|c| c == "namespace-inference-v1")),
            "trusted root broker discovery required"
        );
        value
    };
    let profile = if missing {
        "disabled"
    } else {
        installed["data"]["configs_current"]["VOS_APP_PROFILES"]
            .as_str()
            .context("actual V-Sub profile required")?
    };
    ensure!(
        missing || ["amd", "amd-cuda", "arm", "arm-cuda", "l4t", "thor"].contains(&profile),
        "unsupported V-Sub profile"
    );
    let guard = shared.config.management_lock.lock().await;
    let a = App::from_ref(shared);
    let node = a
        .config
        .local_agent
        .as_ref()
        .and_then(|v| v["node_id"].as_str())
        .context("local node required")?;
    let row = sqlx::query(
        "SELECT capacities,resident_baseline,report,management_enabled FROM vrs_nodes WHERE id=$1",
    )
    .bind(node)
    .fetch_one(&a.db)
    .await
    .context("V-Sub phase: node query")?;
    ensure!(
        row.get::<bool, _>("management_enabled"),
        "local node disabled"
    );
    let state = build(
        node,
        &serde_json::from_value(row.get("capacities"))?,
        &serde_json::from_value(row.get("resident_baseline"))?,
        &row.get::<Value, _>("report"),
        profile,
        &discovery,
        approvals,
        old.as_ref(),
    )
    .context("V-Sub phase: calibration")?;
    if old.as_ref() != Some(&state) {
        persist(&a, VSUB, &state, old.as_ref())
            .await
            .context("V-Sub phase: ledger")?;
        save_private(&path, &state).context("V-Sub phase: private state")?;
    }
    let gateway = PathBuf::from(
        std::env::var("SCHEDULER_GATEWAY_DATA").unwrap_or_else(|_| "/gateway-data".into()),
    )
    .join("vsub-scheduler.local.json");
    save_private(&gateway, &state["gateway_snapshot"]).context("V-Sub phase: gateway snapshot")?;
    drop(guard);
    if state["binding"]["models"]
        .as_object()
        .is_some_and(|m| !m.is_empty())
    {
        let acknowledgement = bounded_json(
            client
                .post(format!("{ORIGIN}/api/v1/scheduler/binding"))
                .bearer_auth(context.trim())
                .json(&state["binding"])
                .send()
                .await?,
        )
        .await?;
        ensure!(
            acknowledgement["configured"] == true
                && acknowledgement["binding_revision"] == state["binding"]["binding_revision"],
            "private broker binding pending acceptance"
        );
    }
    Ok(())
}
// Only known static validation messages may enter logs. Never format a
// transport/database error chain containing URLs, credentials or payloads.
fn safe_failure(error: &anyhow::Error) -> &'static str {
    const REASONS: &[&str] = &[
        "V-Sub phase: context read",
        "V-Sub phase: identity HTTP",
        "V-Sub phase: identity response",
        "V-Sub phase: installation HTTP",
        "V-Sub phase: installation response",
        "V-Sub phase: discovery HTTP",
        "V-Sub phase: discovery response",
        "V-Sub phase: node query",
        "V-Sub phase: calibration",
        "V-Sub phase: ledger",
        "V-Sub phase: private state",
        "V-Sub phase: gateway snapshot",
        "Scheduler application identity required",
        "V-Sub installation check unavailable",
        "trusted root broker discovery required",
        "actual V-Sub profile required",
        "unsupported V-Sub profile",
        "local node required",
        "local node disabled",
        "invalid namespace or worker generation",
        "model namespace prefix mismatch",
        "private model declaration mismatch",
        "private upstream origin mismatch",
        "duplicate private model identity",
        "ambiguous private model calibration",
        "invalid private model resources",
        "measured private model peak and stop evidence required",
        "private runtime/input calibration required",
        "private broker binding pending acceptance",
        "application binding waits for active or unknown execution",
        "application executor was disabled or changed",
        "application service identity was revoked",
    ];
    // Prefer an exact validation reason over its surrounding phase marker.
    let messages = error
        .chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>();
    for phase in [false, true] {
        for reason in REASONS {
            if reason.starts_with("V-Sub phase:") == phase && messages.iter().any(|m| m == reason) {
                return reason;
            }
        }
    }
    "transport, storage or response unavailable"
}
pub(super) fn start(shared: SharedApp, configuration: PathBuf) {
    if std::env::var_os("VOS_APP_VERSION").is_none() {
        return;
    }
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("private broker client");
        loop {
            let result = (|| -> anyhow::Result<Value> {
                let path = configuration
                    .parent()
                    .context("configuration directory required")?
                    .join("vsub-approvals.json");
                if path.exists() {
                    read_private(&path)
                } else {
                    Ok(serde_json::from_slice(&std::fs::read(
                        "/opt/v-scheduler/vsub-calibrations.json",
                    )?)?)
                }
            })();
            if let Ok(approvals) = result {
                if let Err(error) = reconcile(&shared, &configuration, &client, &approvals).await {
                    eprintln!(
                        "V-Sub private calibrated binding pending: {}",
                        safe_failure(&error)
                    );
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, Value) {
        let namespace = "a".repeat(64);
        let id = format!("v-sub-{}/builtin/tiny-q5_1", &namespace[..12]);
        let discovery = json!({"models":[{"id":id,"native_id":"builtin/tiny-q5_1","namespace":namespace,"generation":"b".repeat(32),"task_type":"model.infer","upstream_url":format!("{ORIGIN}/inference/{namespace}/v1")}]});
        let approvals = json!({"protocol_version":1,"credential_epoch":1,"approvals":[{"architecture":std::env::consts::ARCH,"profile":"l4t","hardware":[],"native_model":"builtin/tiny-q5_1","execution_settings":{"profile":"l4t","runtime":"fixture"},"resources":{"cpu_millicores":4500,"memory_mib":2048},"evidence":{"id":"isolated-unit-only","confirmed_stopped":true,"peak_memory_mib":800,"observed_peak_cpu_millicores":4021},"max_input_bytes":1048576,"model_limits":{"input_bytes":1048576,"duration_seconds":20,"prompt_characters":0},"limits":{}}]});
        (discovery, approvals)
    }
    #[test]
    fn diagnostics_never_log_unknown_error_content() {
        let secret = anyhow::anyhow!("private token and signed URL must remain private");
        assert_eq!(
            safe_failure(&secret),
            "transport, storage or response unavailable"
        );
        assert_eq!(
            safe_failure(&secret.context("V-Sub phase: ledger")),
            "V-Sub phase: ledger"
        );
        let validation = anyhow::anyhow!("private runtime/input calibration required")
            .context("V-Sub phase: calibration");
        assert_eq!(
            safe_failure(&validation),
            "private runtime/input calibration required"
        );
    }
    #[test]
    fn full_namespace_origin_and_live_generation_are_required() {
        let (mut d, _) = fixture();
        assert!(model(&d["models"][0]).is_ok());
        d["models"][0]["upstream_url"] = json!("http://other/v1");
        assert!(model(&d["models"][0]).is_err());
        let (mut d, _) = fixture();
        d["models"][0]["namespace"] = json!("a".repeat(12));
        assert!(model(&d["models"][0]).is_err());
        let (mut d, _) = fixture();
        d["models"][0]["generation"] = json!("");
        assert!(model(&d["models"][0]).is_err());
    }
    #[test]
    fn private_models_match_real_calibration_and_restart_uses_new_ids() {
        let (mut d, a) = fixture();
        let cap = Resources {
            cpu_millicores: 4500,
            memory_mib: 8192,
            ..Default::default()
        };
        let first = build(
            "local",
            &cap,
            &Resources::default(),
            &json!({"gpus":[]}),
            "l4t",
            &d,
            &a,
            None,
        )
        .unwrap();
        let same = build(
            "local",
            &cap,
            &Resources::default(),
            &json!({"gpus":[]}),
            "l4t",
            &d,
            &a,
            Some(&first),
        )
        .unwrap();
        assert_eq!(first, same);
        d["models"][0]["generation"] = json!("c".repeat(32));
        let next = build(
            "local",
            &cap,
            &Resources::default(),
            &json!({"gpus":[]}),
            "l4t",
            &d,
            &a,
            Some(&first),
        )
        .unwrap();
        assert_ne!(first["executors"][0]["id"], next["executors"][0]["id"]);
        assert_eq!(next["binding"]["binding_revision"], 2);
        let mismatch = build(
            "local",
            &cap,
            &Resources::default(),
            &json!({"gpus":[]}),
            "arm",
            &d,
            &a,
            None,
        )
        .unwrap();
        assert!(
            mismatch["binding"]["models"]
                .as_object()
                .unwrap()
                .is_empty()
        );
    }
}
