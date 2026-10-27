//! Fixed VOS application bindings. Discovery describes readiness; only a matching
//! measured calibration authorizes compute. No client-supplied URL grants trust.
use super::*;
use std::{os::unix::fs::PermissionsExt, path::Path, time::Duration};

// Existing execution apps validate this binding URL strictly. Public callers
// use the canonical v-scheduler endpoints documented in the integration guide.
const CONTROLLER: &str = "http://v-router:8080/scheduler/api/v1";

#[derive(Clone, Copy)]
pub(super) struct Application {
    slug: &'static str,
    id: &'static str,
    origin: &'static str,
    upstream: &'static str,
    discovery: &'static str,
    asynchronous: bool,
    native_permit_only: bool,
}

const APPLICATIONS: [Application; 4] = [
    Application {
        slug: "v-music",
        id: "com.ictrek.v-music",
        origin: "http://v-music-backend:7080",
        upstream: "app-v-music",
        discovery: "/api/v1/discovery",
        asynchronous: false,
        native_permit_only: false,
    },
    Application {
        slug: "desensitize",
        id: "com.ictrek.desensitize",
        origin: "http://desensitize-backend:5000",
        upstream: "app-desensitize",
        discovery: "/api/v1/discovery",
        asynchronous: false,
        native_permit_only: false,
    },
    Application {
        slug: "image-studio",
        id: "com.ictrek.image-studio",
        origin: "http://image-studio-backend:5000",
        upstream: "app-image-studio",
        discovery: "/api/v1/v-router/registration",
        asynchronous: true,
        native_permit_only: false,
    },
    Application {
        slug: "v-yue2",
        id: "com.ictrek.v-yue2",
        origin: "http://v-yue2-backend:8791",
        upstream: "app-v-yue2",
        discovery: "/api/v1/scheduler/discovery",
        asynchronous: false,
        native_permit_only: true,
    },
];

pub(super) const VSUB: Application = Application {
    slug: "v-sub",
    id: "com.ictrek.v-sub",
    origin: "http://v-sub-backend:7090",
    upstream: "v-sub",
    discovery: "/api/v1/scheduler/discovery",
    asynchronous: false,
    native_permit_only: false,
};

// CPU 总核数属于主机观测；已固定 --threads 4 的图像运行时仍按原批准预算准入。
fn execution_settings_match(application: Application, expected: &Value, actual: &Value) -> bool {
    if application.slug != "image-studio" || expected["sd_cpp_extra_args_sha256"] != "d020f4ca7c181c781d4ef5f4343d0bb4410a8806ce547538fac1e6b92a123579" {
        return expected == actual;
    }
    let (Some(mut expected), Some(mut actual)) = (expected.as_object().cloned(), actual.as_object().cloned()) else { return false; };
    expected.remove("logical_cpu_count");
    actual.remove("logical_cpu_count");
    expected == actual
}

fn supported_profile(application: Application, profile: &str) -> bool {
    let profiles: &[&str] = if application.slug == "desensitize" || application.native_permit_only {
        &[
            "amd",
            "amd-without-cuda",
            "arm",
            "arm-without-cuda",
            "l4t",
            "thor-spark",
        ]
    } else {
        &["amd", "amd-cuda", "arm", "arm-cuda", "l4t", "thor"]
    };
    profiles.contains(&profile)
}

fn upstream(application: Application) -> String {
    if application.asynchronous || application.native_permit_only {
        application.origin.to_owned()
    } else {
        format!("{}/v1", application.origin)
    }
}

fn declaration<'a>(application: Application, discovery: &'a Value) -> anyhow::Result<&'a Value> {
    let contract = &discovery["scheduler"];
    ensure!(
        contract["protocol_version"] == 1
            && contract["app_id"] == application.id
            && contract["executor_base_url"] == application.origin
            && contract["upstream_url"] == upstream(application)
            && contract["binding_endpoint"] == "/api/v1/scheduler/binding",
        "application binding origin mismatch"
    );
    ensure!(
        contract["execution_settings"]
            .as_object()
            .is_some_and(|settings| !settings.is_empty()),
        "actual execution settings required"
    );
    Ok(contract)
}

fn credential(old: Option<&Value>, field: &str) -> String {
    old.and_then(|value| value["binding"][field].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple()))
}

pub(super) fn hardware(report: &Value) -> Value {
    let mut devices: Vec<Value> = report["gpus"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|gpu| json!({"name":gpu["name"],"memory_type":gpu["memory_type"]}))
        .collect();
    devices.sort_by_key(Value::to_string);
    json!(devices)
}

// The caller supplies the actual VOS profile and telemetry, never discovery's
// claim of its own hardware. A missing calibration produces no executors.
fn build(
    application: Application,
    node: &str,
    capacity: &Resources,
    baseline: &Resources,
    profile: &str,
    report: &Value,
    contract: &Value,
    approvals: &Value,
    old: Option<&Value>,
) -> anyhow::Result<Value> {
    ensure!(
        approvals["protocol_version"] == 1,
        "unsupported application calibration format"
    );
    let epoch = approvals["credential_epoch"]
        .as_u64()
        .context("credential epoch required")?;
    ensure!(
        old.is_none_or(|value| epoch >= value["credential_epoch"].as_u64().unwrap_or(0)),
        "credential epoch cannot roll back"
    );
    let previous = old.filter(|value| value["credential_epoch"] == epoch);
    let token = credential(previous, "token");
    let executor_token = if application.native_permit_only { String::new() } else { credential(previous, "executor_token") };
    let gateway_token = if application.native_permit_only { String::new() } else { previous
        .and_then(|value| value["gateway"]["token"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())) };
    let owner = format!("v-scheduler.app-gateway.{}", application.slug);
    let mut models = serde_json::Map::new();
    let mut gateway_executors = serde_json::Map::new();
    let mut model_limits = serde_json::Map::new();
    let mut model_parameters = serde_json::Map::new();
    let mut policies = serde_json::Map::new();
    let mut executors = Vec::new();
    let mut adapters = Vec::new();
    let mut gateway_ids = Vec::new();
    let mut delegated_ids = Vec::new();
    let mut policy_predecessors = Vec::new();
    let actual_hardware = hardware(report);
    for approval in approvals["approvals"]
        .as_array()
        .context("calibration list required")?
    {
        if approval["app_id"] != application.id
            || approval["architecture"].as_str() != Some(std::env::consts::ARCH)
            || approval["profile"] != profile
            || approval["hardware"] != actual_hardware
            || !execution_settings_match(application, &approval["execution_settings"], &contract["execution_settings"])
        {
            continue;
        }
        let model = approval["model_id"]
            .as_str()
            .context("calibrated model required")?;
        let task_type = approval["task_type"]
            .as_str()
            .context("calibrated task type required")?;
        ensure!(
            !model.is_empty() && model.len() <= 512 && !model.chars().any(char::is_control),
            "invalid calibrated model"
        );
        ensure!(
            if application.asynchronous {
                ["image.generate", "video.generate"].contains(&task_type)
            } else {
                task_type == "model.infer"
            },
            "unexpected application task type"
        );
        let list = if application.asynchronous {
            "async_models"
        } else {
            "sync_models"
        };
        let restart = application.id == "com.ictrek.image-studio" && contract["preemption_modes"].as_array().is_some_and(|v| v.iter().any(|x| x == "restart"));
        // A changed execution policy gets a new calibrated identity. Never
        // overwrite an existing executor or an administrator's disable/edit.
        let legacy_digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&json!([application.id,node,approval]))?));
        let fingerprint = if restart { json!([application.id, node, approval, "restart-v1"]) } else { json!([application.id, node, approval]) };
        let digest = format!("{:x}", Sha256::digest(serde_json::to_vec(&fingerprint)?));
        let executor = format!("app-auto-{}-{}", application.slug, &digest[..24]);
        let declared_ready = contract[list].as_array().is_some_and(|rows| {
            rows.iter()
                .any(|row| row["id"] == model && row["task_type"] == task_type)
        });
        // A download/update may briefly omit a model from ready discovery. Keep
        // only its exact existing calibrated identity, so a transient snapshot
        // does not retire it permanently; this never creates an unready grant.
        let existing = previous.is_some_and(|state| state["binding"]["models"][model] == executor);
        if !declared_ready && !existing {
            continue;
        }
        let resources: Resources = serde_json::from_value(approval["resources"].clone())?;
        ensure!(
            resources.valid()
                && resources.cpu_millicores > 0
                && resources.memory_mib > 0
                && (resources.gpu_memory_mib == 0 || resources.gpu_slots > 0),
            "invalid measured resources"
        );
        ensure!(
            approval["evidence"]["id"]
                .as_str()
                .is_some_and(|id| !id.is_empty())
                && approval["evidence"]["peak_memory_mib"]
                    .as_i64()
                    .is_some_and(|peak| peak > 0 && peak <= resources.memory_mib)
                && approval["evidence"]["confirmed_stopped"] == true,
            "measurement and stop evidence required"
        );
        if !resources.fits(baseline, capacity) {
            continue;
        }
        let maximum = approval["max_input_bytes"]
            .as_u64()
            .context("measured input bound required")?;
        ensure!(
            (1..=33554432).contains(&maximum)
                && approval["model_limits"]
                    .as_object()
                    .is_some_and(|limits| !limits.is_empty()),
            "model input limits required"
        );
        if application.native_permit_only {
            for key in ["duration_seconds", "steps", "prompt_characters", "lyrics_characters"] {
                ensure!(approval["model_limits"][key].as_u64().is_some_and(|v|v>0), "native input bounds required");
            }
            if resources.gpu_memory_mib > 0 {
                ensure!(approval["evidence"]["peak_gpu_memory_mib"].as_i64().is_some_and(|v|v>0 && v<=resources.gpu_memory_mib), "native GPU measurement required");
            }
        }
        let gateway_executor = format!("app-gateway-{}-{}", application.slug, &digest[..24]);
        ensure!(!models.contains_key(model), "duplicate model calibration");
        models.insert(model.into(), json!(executor));
        gateway_executors.insert(model.into(), json!(gateway_executor));
        model_limits.insert(model.into(), approval["model_limits"].clone());
        if application.asynchronous {
            ensure!(
                approval["model_parameters"]
                    .as_object()
                    .is_some_and(|parameters| !parameters.is_empty()),
                "measured image runtime parameters required"
            );
            model_parameters.insert(model.into(), approval["model_parameters"].clone());
        }
        executors.push(json!({"id":executor,"app_id":application.id,"node_id":node,"contract_version":"1","task_type":task_type,"resources":resources,"upstream_urls":[upstream(application)],"placements":[],"retry_mode":if restart {"idempotent"} else {"none"},"max_attempts":if restart {3} else {1}}));
        if !application.native_permit_only { executors.push(json!({"id":gateway_executor,"app_id":owner,"node_id":node,"contract_version":"1","task_type":"model.infer","resources":resources,"upstream_urls":[upstream(application)],"placements":[],"retry_mode":"none","max_attempts":1})); }
        if restart {
            let mut task = executors[executors.len()-2].clone();
            task["id"] = json!(format!("app-auto-{}-{}",application.slug,&legacy_digest[..24]));
            task["retry_mode"] = json!("none"); task["max_attempts"] = json!(1);
            let mut gateway = executors.last().unwrap().clone();
            gateway["id"] = json!(format!("app-gateway-{}-{}",application.slug,&legacy_digest[..24]));
            policy_predecessors.extend([task,gateway]);
        }
        if !application.native_permit_only { gateway_ids.push(gateway_executor.clone()); }
        if application.asynchronous {
            delegated_ids.push(executor.clone());
            adapters.push(json!({"id":executor,"url":format!("{}/api/v1",application.origin),"token":executor_token,"health_url":format!("{}/health",application.origin)}));
        }
        if !application.native_permit_only { policies.insert(format!("{}/{model}",application.upstream),json!({"mode":if application.asynchronous {"delegated"} else {"permit"},"executor_id":gateway_executor,"completion":"synchronous","upstream_url":upstream(application),"max_input_bytes":maximum,"limits":approval["limits"].as_object().cloned().unwrap_or_default()})); }
    }
    let mut state = json!({"credential_epoch":epoch,"binding":{"protocol_version":1,"binding_revision":1,"app_id":application.id,"url":CONTROLLER,"token":token,"executor_token":executor_token,"upstream_url":upstream(application),"execution_settings":contract["execution_settings"],"models":models,"gateway_executors":gateway_executors,"model_limits":model_limits,"model_parameters":model_parameters},
        "principals":[{"token":token,"role":"submitter","app_id":application.id,"permit_executors":gateway_ids},{"token":gateway_token,"role":"submitter","app_id":owner,"delegated_executors":delegated_ids}],
        "executors":executors,"adapters":adapters,"gateway":{"url":CONTROLLER,"token":gateway_token,"require_model_policy":true,"models":policies}});
    if application.native_permit_only {
        state["binding"]["url"] = json!("http://v-scheduler:8080/scheduler/api/v1");
        state["principals"].as_array_mut().unwrap().truncate(1);
        let b=state["binding"].as_object_mut().unwrap();
        for key in ["executor_token","gateway_executors","model_parameters"] { b.remove(key); }
        state["gateway"] = Value::Null;
    }
    if !policy_predecessors.is_empty() { state["policy_predecessors"] = json!(policy_predecessors); }
    if contract["lifecycle_protocol_version"] == 1 {
        state["binding"]["lifecycle_protocol_version"] = json!(1);
    }
    if let Some(old) = old {
        let revision = old["binding"]["binding_revision"]
            .as_u64()
            .context("persisted binding revision missing")?;
        state["binding"]["binding_revision"] = json!(revision);
        if state != *old {
            state["binding"]["binding_revision"] = json!(
                revision
                    .checked_add(1)
                    .context("binding revision exhausted")?
            );
        }
    }
    Ok(state)
}

pub(super) fn read_private(path: &Path) -> anyhow::Result<Value> {
    let metadata = std::fs::symlink_metadata(path)?;
    ensure!(
        metadata.is_file()
            && metadata.permissions().mode() & 0o077 == 0
            && metadata.len() <= 262144,
        "private bounded binding required"
    );
    Ok(serde_json::from_slice(&std::fs::read(path)?)?)
}

pub(super) fn save_private(path: &Path, value: &Value) -> anyhow::Result<()> {
    use std::{io::Write, os::unix::fs::OpenOptionsExt};
    let directory = path.parent().context("binding parent required")?;
    std::fs::create_dir_all(directory)?;
    let data = serde_json::to_vec(value)?;
    ensure!(data.len() <= 262144, "binding exceeds bound");
    let pending = directory.join(format!(".{}.pending", Uuid::new_v4().simple()));
    let result = (|| -> anyhow::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&pending)?;
        file.write_all(&data)?;
        file.sync_all()?;
        std::fs::rename(&pending, path)?;
        std::fs::File::open(directory)?.sync_all()?;
        Ok(())
    })();
    let _ = std::fs::remove_file(pending);
    result
}

pub(super) async fn bounded_json(response: reqwest::Response) -> anyhow::Result<Value> {
    bounded_json_limit(response, 262144).await
}

// VOS app details include the installation icon and manifest. Their bound is
// separate from private discovery/binding payloads and credential snapshots.
pub(super) async fn bounded_vos_installation(response: reqwest::Response) -> anyhow::Result<Value> {
    bounded_json_limit(response, 1048576).await
}

async fn bounded_json_limit(
    mut response: reqwest::Response,
    maximum: usize,
) -> anyhow::Result<Value> {
    ensure!(
        response.status().is_success(),
        "application discovery unavailable"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            bytes.len().saturating_add(chunk.len()) <= maximum,
            "application response exceeds bound"
        );
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub(super) async fn persist(
    a: &App,
    application: Application,
    state: &Value,
    old: Option<&Value>,
) -> anyhow::Result<()> {
    let node = a
        .config
        .local_agent
        .as_ref()
        .and_then(|local| local["node_id"].as_str())
        .context("local node required")?;
    let owner = format!("v-scheduler.app-gateway.{}", application.slug);
    let mut tx = a.db.begin().await?;
    sqlx::query("SELECT id FROM vrs_nodes WHERE id=$1 FOR UPDATE")
        .bind(node)
        .fetch_one(&mut *tx)
        .await?;
    let busy: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_permits WHERE app IN ($1,$2) AND state<>'released') OR EXISTS(SELECT 1 FROM vrs_reservations r JOIN vrs_tasks t ON t.id=r.task WHERE t.app=$1 AND r.state<>'released') OR EXISTS(SELECT 1 FROM vrs_tasks WHERE app=$1 AND status NOT IN ('succeeded','failed','cancelled'))")
        .bind(application.id).bind(&owner).fetch_one(&mut *tx).await?;
    ensure!(
        !busy,
        "application binding waits for active or unknown execution"
    );
    let definitions = state["executors"]
        .as_array()
        .context("executor definitions required")?;
    let ids: Vec<&str> = definitions
        .iter()
        .map(|row| row["id"].as_str().context("executor ID missing"))
        .collect::<anyhow::Result<_>>()?;
    // Retire only this module's own executors. Administrator-created identities
    // and bindings belonging to another application remain untouched.
    let prefix = format!("app-auto-{}-", application.slug);
    let gateway_prefix = format!("app-gateway-{}-", application.slug);
    let actor = format!("v-scheduler.auto.{}", application.slug);
    // Check the exact legacy identities derived from the same measured approval,
    // even if a transient empty discovery has removed the previous file mapping.
    for previous in state["policy_predecessors"].as_array().into_iter().flatten() {
        let old_id = previous["id"].as_str().context("previous executor ID required")?;
        if let Some(row) = sqlx::query("SELECT definition,state,version FROM vrs_executor_settings WHERE id=$1 FOR UPDATE")
            .bind(old_id).fetch_optional(&mut *tx).await? {
            let auto_retired: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_management_events WHERE actor=$1 AND action='executor.auto-retired' AND resource=$2)")
                .bind(&actor).bind(format!("{}:{}",old_id,row.get::<i64,_>("version"))).fetch_one(&mut *tx).await?;
            ensure!(row.get::<Value,_>("definition") == *previous && (row.get::<String,_>("state") == "enabled" || auto_retired), "application predecessor was disabled or changed");
        }
    }
    let retired = sqlx::query("UPDATE vrs_executor_settings SET state='disabled',version=version+1,updated_at=now() WHERE (starts_with(id,$1) OR starts_with(id,$2)) AND NOT(id=ANY($3)) AND state='enabled' RETURNING id,version")
        .bind(&prefix).bind(&gateway_prefix).bind(&ids).fetch_all(&mut *tx).await?;
    for row in retired {
        let resource = format!(
            "{}:{}",
            row.get::<String, _>("id"),
            row.get::<i64, _>("version")
        );
        sqlx::query("INSERT INTO vrs_management_events(actor,action,resource) VALUES($1,'executor.auto-retired',$2)")
            .bind(&actor).bind(resource).execute(&mut *tx).await?;
    }
    for definition in definitions {
        let id = definition["id"].as_str().context("executor ID missing")?;
        // Only our own exact retirement at the current row version can recover.
        // All existing admin writers increment version, including old releases;
        // a manual disable therefore cannot be mistaken for our prior action.
        sqlx::query("INSERT INTO vrs_executor_settings(id,node_id,definition,state) VALUES($1,$2,$3,'enabled') ON CONFLICT(id) DO UPDATE SET state='enabled',version=vrs_executor_settings.version+1,updated_at=now() WHERE vrs_executor_settings.state='disabled' AND vrs_executor_settings.node_id=excluded.node_id AND vrs_executor_settings.definition=excluded.definition AND EXISTS(SELECT 1 FROM vrs_management_events e WHERE e.actor=$4 AND e.action='executor.auto-retired' AND e.resource=vrs_executor_settings.id||':'||vrs_executor_settings.version::text)")
            .bind(id).bind(node).bind(definition).bind(&actor).execute(&mut *tx).await?;
        let enabled: bool = sqlx::query_scalar(
            "SELECT state='enabled' AND definition=$2 FROM vrs_executor_settings WHERE id=$1",
        )
        .bind(id)
        .bind(definition)
        .fetch_one(&mut *tx)
        .await?;
        ensure!(enabled, "application executor was disabled or changed");
    }
    for principal in state["principals"]
        .as_array()
        .context("service identities required")?
    {
        let secret = principal["token"]
            .as_str()
            .context("service credential required")?;
        let digest = format!("{:x}", Sha256::digest(secret.as_bytes()));
        let id = format!("app-auto-identity-{}", &digest[..24]);
        let app = principal["app_id"]
            .as_str()
            .context("identity owner required")?;
        let permit = principal["permit_executors"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let delegated = principal["delegated_executors"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        sqlx::query("INSERT INTO vrs_service_identities(id,app_id,token_hash,permit_executors,delegated_executors) VALUES($1,$2,$3,$4,$5) ON CONFLICT(id) DO UPDATE SET permit_executors=excluded.permit_executors,delegated_executors=excluded.delegated_executors WHERE vrs_service_identities.revoked_at IS NULL AND vrs_service_identities.app_id=excluded.app_id AND vrs_service_identities.token_hash=excluded.token_hash")
            .bind(&id).bind(app).bind(&digest).bind(json!(permit)).bind(json!(delegated)).execute(&mut *tx).await?;
        let enabled: bool = sqlx::query_scalar("SELECT revoked_at IS NULL AND app_id=$2 AND token_hash=$3 FROM vrs_service_identities WHERE id=$1")
            .bind(&id).bind(app).bind(&digest).fetch_one(&mut *tx).await?;
        ensure!(enabled, "application service identity was revoked");
    }
    if let Some(old) = old {
        for principal in old["principals"]
            .as_array()
            .context("persisted service identities required")?
        {
            if state["principals"]
                .as_array()
                .is_some_and(|rows| rows.iter().any(|row| row["token"] == principal["token"]))
            {
                continue;
            }
            let token = principal["token"]
                .as_str()
                .context("persisted credential required")?;
            let digest = format!("{:x}", Sha256::digest(token.as_bytes()));
            sqlx::query("UPDATE vrs_service_identities SET revoked_at=COALESCE(revoked_at,now()) WHERE token_hash=$1 AND app_id IN ($2,$3)")
                .bind(digest).bind(application.id).bind(&owner).execute(&mut *tx).await?;
        }
    }
    sqlx::query("INSERT INTO vrs_management_events(actor,action,resource) VALUES('v-scheduler','application.binding.update',$1)")
        .bind(application.id).execute(&mut *tx).await?;
    tx.commit().await?;
    management::restore(&a.db, &a.voice_config).await?;
    Ok(())
}

async fn reconcile(
    shared: &SharedApp,
    configuration: &Path,
    application: Application,
    client: &reqwest::Client,
    approvals: &Value,
) -> anyhow::Result<()> {
    let directory = configuration
        .parent()
        .context("configuration directory required")?;
    let path = directory
        .join("app-bindings")
        .join(format!("{}.json", application.slug));
    if !path.exists()
        && !approvals["approvals"]
            .as_array()
            .is_some_and(|rows| rows.iter().any(|row| row["app_id"] == application.id))
    {
        return Ok(());
    }
    let context_token = std::fs::read_to_string("/var/lib/vos-context/access-token")?;
    let vos = crate::identity::base().context("VOS identity endpoint required")?;
    crate::identity::application(client, &vos, &context_token, "com.ictrek.v-scheduler").await?;
    let installed = bounded_vos_installation(
        client
            .get(format!(
                "{}/v1000/app/detail?app_id={}",
                vos.trim_end_matches('/'),
                application.id
            ))
            .bearer_auth(context_token.trim())
            .send()
            .await?,
    )
    .await?;
    ensure!(
        installed["code"] == 0 && installed["data"]["state"] == "installed",
        "application must be installed on this VOS"
    );
    let discovery = bounded_json(
        client
            .get(format!("{}{}", application.origin, application.discovery))
            .send()
            .await?,
    )
    .await?;
    let contract = declaration(application, &discovery)?;
    let _guard = shared.config.management_lock.lock().await;
    let a = App::from_ref(shared);
    let node_id = a
        .config
        .local_agent
        .as_ref()
        .and_then(|local| local["node_id"].as_str())
        .context("local node required")?;
    let row = sqlx::query(
        "SELECT capacities,resident_baseline,report,management_enabled FROM vrs_nodes WHERE id=$1",
    )
    .bind(node_id)
    .fetch_one(&a.db)
    .await?;
    ensure!(
        row.get::<bool, _>("management_enabled"),
        "local node disabled"
    );
    let report: Value = row.get("report");
    let profile = installed["data"]["configs_current"]["VOS_APP_PROFILES"]
        .as_str()
        .context("actual application installation profile required")?;
    ensure!(
        supported_profile(application, profile),
        "unsupported application profile"
    );
    let old = if path.exists() {
        Some(read_private(&path)?)
    } else {
        None
    };
    let state = build(
        application,
        node_id,
        &serde_json::from_value(row.get("capacities"))?,
        &serde_json::from_value(row.get("resident_baseline"))?,
        &profile,
        &report,
        contract,
        approvals,
        old.as_ref(),
    )?;
    if old.as_ref() != Some(&state) {
        ensure!(
            serde_json::to_vec(&state)?.len() <= 262144,
            "binding exceeds bound"
        );
        persist(&a, application, &state, old.as_ref()).await?;
        save_private(&path, &state)?;
    }
    if !application.native_permit_only {
    // Sharing this lock with V-Voice's read/merge/write prevents one app's
    // adapter refresh from losing another app's configuration.
    let agent_path = directory.join("local-agent.json");
    let mut agent = read_private(&agent_path)?;
    let adapters = agent["executors"]
        .as_array_mut()
        .context("local adapters required")?;
    let prefix = format!("app-auto-{}-", application.slug);
    adapters.retain(|entry| {
        !entry["id"]
            .as_str()
            .is_some_and(|id| id.starts_with(&prefix))
    });
    adapters.extend(
        state["adapters"]
            .as_array()
            .context("application adapters required")?
            .clone(),
    );
    save_private(&agent_path, &agent)?;
    let gateway = PathBuf::from(
        std::env::var("SCHEDULER_GATEWAY_DATA").unwrap_or_else(|_| "/gateway-data".into()),
    );
    save_private(
        &gateway
            .join("app-bindings")
            .join(format!("{}.json", application.slug)),
        &state["gateway"],
    )?;
    }
    if state["binding"]["models"]
        .as_object()
        .is_some_and(|models| !models.is_empty())
    {
        let response = client
            .post(format!("{}/api/v1/scheduler/binding", application.origin))
            .bearer_auth(context_token.trim())
            .json(&state["binding"])
            .send()
            .await?;
        crate::contract::confirm_application_binding(response, &state["binding"]["binding_revision"]).await?;
    }
    Ok(())
}

fn safe_failure(error: &anyhow::Error) -> &'static str {
    const REASONS: &[&str] = &[
        "unsupported application calibration format",
        "credential epoch cannot roll back",
        "invalid measured resources",
        "measurement and stop evidence required",
        "native input bounds required",
        "native GPU measurement required",
        "duplicate model calibration",
        "application binding waits for active or unknown execution",
        "application executor was disabled or changed",
        "application predecessor was disabled or changed",
        "application service identity was revoked",
        "application binding pending acceptance",
        "application identity required",
        "actual application profile required",
        "unsupported application profile",
    ];
    for cause in error.chain() {
        if let Some(reason) = REASONS.iter().find(|reason| cause.to_string() == **reason) {
            return reason;
        }
    }
    "identity, transport or storage unavailable"
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
            .expect("application binding client");
        loop {
            let result = (|| -> anyhow::Result<Value> {
                let path = configuration
                    .parent()
                    .context("configuration directory missing")?
                    .join("app-approvals.json");
                if path.exists() {
                    read_private(&path)
                } else {
                    Ok(serde_json::from_slice(&std::fs::read(
                        "/opt/v-scheduler/app-calibrations.json",
                    )?)?)
                }
            })();
            if let Ok(approvals) = result {
                for application in APPLICATIONS {
                    if let Err(error) =
                        reconcile(&shared, &configuration, application, &client, &approvals).await
                    {
                        eprintln!(
                            "application calibrated binding pending ({}): {}",
                            application.slug,
                            safe_failure(&error)
                        );
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn application_version_changes_preserve_the_measured_contract() {
        let capacity = Resources { cpu_millicores: 8000, memory_mib: 16000, ..Default::default() };
        for app in APPLICATIONS.iter().copied().filter(|app| app.slug != "v-music") {
            let profile = if app.slug == "desensitize" { "thor-spark" } else { "thor" };
            let task_type = if app.asynchronous { "image.generate" } else { "model.infer" };
            let list = if app.asynchronous { "async_models" } else { "sync_models" };
            let mut contract = json!({"protocol_version":1,"app_id":app.id,"executor_base_url":app.origin,"upstream_url":upstream(app),"binding_endpoint":"/api/v1/scheduler/binding","execution_settings":{"runtime":"fixture"}});
            contract[list] = json!([{"id":"fixture-model","task_type":task_type}]);
            let approvals = json!({"protocol_version":1,"credential_epoch":1,"approvals":[{"app_id":app.id,"architecture":std::env::consts::ARCH,"profile":profile,"hardware":[],"execution_settings":contract["execution_settings"],"model_id":"fixture-model","task_type":task_type,"resources":{"cpu_millicores":1000,"memory_mib":1024},"evidence":{"id":"unit-only","peak_memory_mib":512,"confirmed_stopped":true},"max_input_bytes":8192,"model_limits":{"input_bytes":8192,"duration_seconds":1,"steps":1,"prompt_characters":100,"lyrics_characters":100},"model_parameters":{"backend":"fixture"}}]});
            let first = build(app,"node",&capacity,&Resources::default(),profile,&json!({}),&contract,&approvals,None).unwrap();
            assert!(!first["binding"]["models"].as_object().unwrap().is_empty());
            for version in ["0.0.99", "0.1.0", "1.0.0"] {
                contract["application_version"] = json!(version);
                contract["_application_version"] = json!(version);
                assert!(declaration(app, &json!({"version":version,"scheduler":contract})).is_ok());
                let next = build(app,"node",&capacity,&Resources::default(),profile,&json!({}),&contract,&approvals,Some(&first)).unwrap();
                assert_eq!(next, first, "{} version {} changed its identity, address or budget", app.slug, version);
            }
            contract["execution_settings"]["runtime"] = json!("uncalibrated");
            let changed = build(app,"node",&capacity,&Resources::default(),profile,&json!({}),&contract,&approvals,Some(&first)).unwrap();
            assert!(changed["binding"]["models"].as_object().unwrap().is_empty(), "a version upgrade must not authorize changed execution settings");
        }
    }

    #[test]
    fn fixed_image_threads_do_not_pin_approval_to_host_core_count() {
        let image = APPLICATIONS.iter().find(|app| app.slug == "image-studio").copied().unwrap();
        let expected = json!({"profile":"thor","runtime":"stable-diffusion.cpp","logical_cpu_count":14,"max_concurrency":1,"sd_cpp_extra_args_sha256":"d020f4ca7c181c781d4ef5f4343d0bb4410a8806ce547538fac1e6b92a123579"});
        let mut actual = expected.clone(); actual["logical_cpu_count"] = json!(12);
        assert!(execution_settings_match(image, &expected, &actual));
        actual["max_concurrency"] = json!(2);
        assert!(!execution_settings_match(image, &expected, &actual));
        actual["max_concurrency"] = json!(1); actual["sd_cpp_extra_args_sha256"] = json!("different-threads");
        assert!(!execution_settings_match(image, &expected, &actual));
        assert!(!execution_settings_match(APPLICATIONS[1], &expected, &actual));
    }

    #[test]
    fn target_application_profiles_are_not_scheduler_profile_names() {
        let music = APPLICATIONS[0];
        let desensitize = APPLICATIONS[1];
        let image = APPLICATIONS[2];
        assert_eq!(upstream(image), "http://image-studio-backend:5000");
        assert_eq!(upstream(music), "http://v-music-backend:7080/v1");
        for app in [music, image] {
            assert!(supported_profile(app, "thor"));
            assert!(supported_profile(app, "amd-cuda"));
            assert!(supported_profile(app, "arm-cuda"));
            assert!(!supported_profile(app, "thor-spark"));
            assert!(!supported_profile(app, "amd-without-cuda"));
        }
        assert!(supported_profile(desensitize, "thor-spark"));
        assert!(supported_profile(desensitize, "amd-without-cuda"));
        assert!(!supported_profile(desensitize, "thor"));
        assert!(!supported_profile(desensitize, "amd-cuda"));
    }

    #[tokio::test]
    async fn vos_installation_icons_have_a_separate_bounded_response() {
        fn response(body: Vec<u8>) -> reqwest::Response {
            axum::http::Response::builder()
                .status(200)
                .body(body)
                .unwrap()
                .into()
        }
        let mut value = json!({"code":0,"data":{"state":"installed","icon_base64":"x".repeat(288032),"manifest_yaml":""}});
        let padding = 304914 - serde_json::to_vec(&value).unwrap().len();
        value["data"]["manifest_yaml"] = json!("x".repeat(padding));
        let bytes = serde_json::to_vec(&value).unwrap();
        assert_eq!(bytes.len(), 304914);
        assert!(bounded_json(response(bytes.clone())).await.is_err());
        assert_eq!(
            bounded_vos_installation(response(bytes)).await.unwrap()["data"]["state"],
            "installed"
        );
        assert!(
            bounded_vos_installation(response(vec![b' '; 1048577]))
                .await
                .is_err()
        );
    }

    #[test]
    fn image_native_origin_and_execution_adapter_have_distinct_bases() {
        let app = APPLICATIONS[2];
        let capacity = Resources {
            cpu_millicores: 4000,
            memory_mib: 8192,
            ..Default::default()
        };
        let contract = json!({"execution_settings":{"runtime":"fixture"},"async_models":[{"id":"fixture-image","task_type":"image.generate"}]});
        let approvals = json!({"protocol_version":1,"credential_epoch":1,"approvals":[{"app_id":app.id,"architecture":std::env::consts::ARCH,"profile":"thor","hardware":[],"execution_settings":contract["execution_settings"],"model_id":"fixture-image","task_type":"image.generate","resources":{"cpu_millicores":1000,"memory_mib":1024},"evidence":{"id":"unit-only","peak_memory_mib":512,"confirmed_stopped":true},"max_input_bytes":8192,"model_limits":{"width":512},"model_parameters":{"backend":"fixture"}}]});
        let state = build(
            app,
            "node",
            &capacity,
            &Resources::default(),
            "thor",
            &json!({}),
            &contract,
            &approvals,
            None,
        )
        .unwrap();
        let mut restart_contract = contract.clone();
        restart_contract["preemption_modes"] = json!(["restart"]);
        let upgraded = build(app,"node",&capacity,&Resources::default(),"thor",&json!({}),&restart_contract,&approvals,Some(&state)).unwrap();
        assert_ne!(upgraded["executors"][0]["id"], state["executors"][0]["id"], "policy upgrade must not collide with an immutable old executor");
        assert_eq!(upgraded["executors"][0]["retry_mode"], "idempotent");
        assert_eq!(upgraded["executors"][0]["max_attempts"], 3);
        assert_eq!(upgraded["executors"][0]["resources"], state["executors"][0]["resources"]);
        assert_eq!(upgraded["binding"]["token"], state["binding"]["token"]);
        assert_eq!(upgraded["binding"]["binding_revision"], 2);
        let repeated = build(app,"node",&capacity,&Resources::default(),"thor",&json!({}),&restart_contract,&approvals,Some(&upgraded)).unwrap();
        assert_eq!(repeated, upgraded, "steady discovery must not churn bindings");
        assert_eq!(
            state["executors"][0]["upstream_urls"][0],
            "http://image-studio-backend:5000"
        );
        assert_eq!(
            state["adapters"][0]["url"],
            "http://image-studio-backend:5000/api/v1"
        );
        assert_eq!(
            state["adapters"][0]["health_url"],
            "http://image-studio-backend:5000/health"
        );
    }

    #[test]
    fn application_urls_are_fixed_and_settings_are_required() {
        let app = APPLICATIONS[0];
        let mut discovery = json!({"scheduler":{"protocol_version":1,"app_id":app.id,"executor_base_url":app.origin,"upstream_url":upstream(app),"binding_endpoint":"/api/v1/scheduler/binding","execution_settings":{"provider":"cpu"}}});
        assert!(declaration(app, &discovery).is_ok());
        discovery["scheduler"]["executor_base_url"] = json!("http://untrusted");
        assert!(declaration(app, &discovery).is_err());
    }

    #[test]
    fn discovery_without_measured_calibration_grants_nothing() {
        let app = APPLICATIONS[0];
        let capacity = Resources {
            cpu_millicores: 4000,
            memory_mib: 8192,
            ..Resources::default()
        };
        let state = build(app,"node",&capacity,&Resources::default(),"amd",&json!({}),&json!({"execution_settings":{"provider":"cpu"},"sync_models":[{"id":"musicgen-small","task_type":"model.infer"}]}),&json!({"protocol_version":1,"credential_epoch":1,"approvals":[]}),None).unwrap();
        assert_eq!(state["executors"], json!([]));
        assert_eq!(state["binding"]["models"], json!({}));
        let repeated = build(
            app,
            "node",
            &capacity,
            &Resources::default(),
            "amd",
            &json!({}),
            &json!({"execution_settings":{"provider":"cpu"},"sync_models":[{"id":"musicgen-small","task_type":"model.infer"}]}),
            &json!({"protocol_version":1,"credential_epoch":1,"approvals":[]}),
            Some(&state),
        )
        .unwrap();
        assert_eq!(state, repeated);
    }

    #[test]
    fn measured_settings_and_hardware_must_match_and_limits_persist() {
        let app = APPLICATIONS[0];
        let capacity = Resources {
            cpu_millicores: 4000,
            memory_mib: 8192,
            ..Resources::default()
        };
        let contract = json!({"execution_settings":{"provider":"cpu","torch_threads":4},"sync_models":[{"id":"musicgen-small","task_type":"model.infer"}]});
        let mut approvals = json!({"protocol_version":1,"credential_epoch":1,"approvals":[{"app_id":app.id,"architecture":std::env::consts::ARCH,"profile":"amd","hardware":[],"execution_settings":contract["execution_settings"],"model_id":"musicgen-small","task_type":"model.infer","resources":{"cpu_millicores":4000,"memory_mib":4096},"evidence":{"id":"unit-fixture-only","peak_memory_mib":2048,"confirmed_stopped":true},"max_input_bytes":8192,"model_limits":{"duration_seconds":10,"prompt_characters":100,"negative_prompt_characters":100}}]});
        let state = build(
            app,
            "node",
            &capacity,
            &Resources::default(),
            "amd",
            &json!({}),
            &contract,
            &approvals,
            None,
        )
        .unwrap();
        assert_eq!(state["executors"].as_array().unwrap().len(), 2);
        let mut temporarily_missing = contract.clone();
        temporarily_missing["sync_models"] = json!([]);
        let retained = build(
            app,
            "node",
            &capacity,
            &Resources::default(),
            "amd",
            &json!({}),
            &temporarily_missing,
            &approvals,
            Some(&state),
        )
        .unwrap();
        assert_eq!(
            state, retained,
            "transient readiness cannot retire an unchanged approval"
        );
        let initial_missing = build(
            app,
            "node",
            &capacity,
            &Resources::default(),
            "amd",
            &json!({}),
            &temporarily_missing,
            &approvals,
            None,
        )
        .unwrap();
        assert!(
            initial_missing["executors"].as_array().unwrap().is_empty(),
            "an unready model cannot receive its first grant"
        );
        let mut different = approvals.clone();
        different["approvals"][0]["resources"]["memory_mib"] = json!(8192);
        let changed_missing = build(
            app,
            "node",
            &capacity,
            &Resources::default(),
            "amd",
            &json!({}),
            &temporarily_missing,
            &different,
            Some(&state),
        )
        .unwrap();
        assert!(
            changed_missing["executors"].as_array().unwrap().is_empty(),
            "readiness retention cannot authorize a different budget"
        );
        for definition in state["executors"].as_array().unwrap() {
            let parsed: Executor = serde_json::from_value(definition.clone()).unwrap();
            assert_eq!(
                *definition,
                serde_json::to_value(parsed).unwrap(),
                "persisted approval must match the runtime executor exactly"
            );
        }
        assert_eq!(
            state["binding"]["model_limits"]["musicgen-small"]["duration_seconds"],
            10
        );
        assert_eq!(
            build(
                app,
                "node",
                &capacity,
                &Resources::default(),
                "amd",
                &json!({}),
                &contract,
                &approvals,
                Some(&state)
            )
            .unwrap(),
            state
        );
        approvals["approvals"][0]["execution_settings"]["torch_threads"] = json!(8);
        assert_eq!(
            build(
                app,
                "node",
                &capacity,
                &Resources::default(),
                "amd",
                &json!({}),
                &contract,
                &approvals,
                None
            )
            .unwrap()["executors"],
            json!([])
        );
        approvals["approvals"][0]["execution_settings"] = contract["execution_settings"].clone();
        approvals["approvals"][0]["evidence"]["confirmed_stopped"] = json!(false);
        assert!(
            build(
                app,
                "node",
                &capacity,
                &Resources::default(),
                "amd",
                &json!({}),
                &contract,
                &approvals,
                None
            )
            .is_err()
        );
    }
    #[test]
    #[cfg(target_arch = "x86_64")]
    fn published_yue_measurement_matches_only_its_hardware_and_runtime() {
        let app = APPLICATIONS.iter().find(|a| a.slug == "v-yue2").copied().unwrap();
        let approvals: Value = serde_json::from_str(include_str!("../../../config/app-calibrations.json")).unwrap();
        let measured = approvals["approvals"].as_array().unwrap().iter().find(|v| v["app_id"] == app.id).unwrap();
        let report = json!({"gpus": measured["hardware"]});
        let contract = json!({"protocol_version":1,"app_id":app.id,"executor_base_url":app.origin,"upstream_url":app.origin,"binding_endpoint":"/api/v1/scheduler/binding","execution_settings":measured["execution_settings"],"sync_models":[{"id":"yue2-q8","task_type":"model.infer"}]});
        let capacity = Resources { cpu_millicores: 16000, memory_mib: 65536, gpu_slots: 1, gpu_memory_mib: 49152 };
        let baseline = Resources::default();
        let state = build(app, "local", &capacity, &baseline, "amd", &report, &contract, &approvals, None).unwrap();
        assert_eq!(state["executors"].as_array().unwrap().len(), 1);
        assert_eq!(state["executors"][0]["resources"], measured["resources"]);
        let mut changed = contract.clone();
        changed["execution_settings"]["native_parent_guard"] = json!("missing");
        assert!(build(app, "local", &capacity, &baseline, "amd", &report, &changed, &approvals, None).unwrap()["executors"].as_array().unwrap().is_empty());
        assert!(build(app, "local", &capacity, &baseline, "amd", &json!({"gpus":[]}), &contract, &approvals, None).unwrap()["executors"].as_array().unwrap().is_empty());
    }
    #[test]
    fn yue_native_binding_uses_one_identity_and_only_measured_complete_bounds() {
        let app=APPLICATIONS.iter().find(|a|a.slug=="v-yue2").copied().unwrap();
        assert!(supported_profile(app,"amd"));assert!(supported_profile(app,"thor-spark"));
        assert_eq!(upstream(app),"http://v-yue2-backend:8791");
        let contract=json!({"protocol_version":1,"app_id":app.id,"executor_base_url":app.origin,"upstream_url":app.origin,"binding_endpoint":"/api/v1/scheduler/binding","execution_settings":{"cpu_threads":8,"keep_loaded":false},"sync_models":[{"id":"yue2-q8","task_type":"model.infer"}]});
        declaration(app,&json!({"scheduler":contract})).unwrap();
        let mut bad=contract.clone();bad["upstream_url"]=json!(format!("{}/v1",app.origin));assert!(declaration(app,&json!({"scheduler":bad})).is_err());
        let capacity=Resources{cpu_millicores:16000,memory_mib:65536,gpu_slots:1,gpu_memory_mib:65536};
        let baseline=Resources::default();let report=json!({"gpus":[]});
        let mut approvals=json!({"protocol_version":1,"credential_epoch":1,"approvals":[{"app_id":app.id,"architecture":std::env::consts::ARCH,"profile":"amd","hardware":[],"execution_settings":contract["execution_settings"],"model_id":"yue2-q8","task_type":"model.infer","resources":{"cpu_millicores":8000,"memory_mib":32768,"gpu_slots":1,"gpu_memory_mib":32768},"evidence":{"id":"unit-fixture-only","peak_memory_mib":16000,"peak_gpu_memory_mib":12000,"confirmed_stopped":true},"max_input_bytes":65536,"model_limits":{"duration_seconds":120,"steps":32,"prompt_characters":1024,"lyrics_characters":4096}}]});
        let state=build(app,"local",&capacity,&baseline,"amd",&report,&contract,&approvals,None).unwrap();
        assert_eq!(state["executors"].as_array().unwrap().len(),1);assert_eq!(state["principals"].as_array().unwrap().len(),1);assert!(state["adapters"].as_array().unwrap().is_empty());assert!(state["gateway"].is_null());
        assert_eq!(state["binding"]["url"],"http://v-scheduler:8080/scheduler/api/v1");assert!(state["binding"].get("executor_token").is_none());
        assert_eq!(state["executors"][0]["upstream_urls"],json!([app.origin]));
        let repeated=build(app,"local",&capacity,&baseline,"amd",&report,&contract,&approvals,Some(&state)).unwrap();assert_eq!(repeated["binding"],state["binding"]);
        approvals["approvals"][0]["model_limits"].as_object_mut().unwrap().remove("lyrics_characters");assert!(build(app,"local",&capacity,&baseline,"amd",&report,&contract,&approvals,None).is_err());
        approvals["approvals"][0]["model_limits"]["lyrics_characters"]=json!(4096);approvals["approvals"][0]["evidence"].as_object_mut().unwrap().remove("peak_gpu_memory_mib");assert!(build(app,"local",&capacity,&baseline,"amd",&report,&contract,&approvals,None).is_err());
    }

}
