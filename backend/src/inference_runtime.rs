use std::{
    process::Stdio,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

const PORT: u16 = 38122;
static ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, PartialEq, Eq)]
enum LegacyState {
    Absent,
    Stopped,
    Installed,
}

fn legacy_state(status: &serde_json::Value) -> Option<LegacyState> {
    match status["code"].as_u64()? {
        10067 => Some(LegacyState::Absent),
        0 => status["data"]["state"].as_str().map(|state| match state {
            "not_installed" | "uninstalled" => LegacyState::Absent,
            "stopped" => LegacyState::Stopped,
            _ => LegacyState::Installed,
        }),
        _ => None,
    }
}

fn may_start(legacy: Option<LegacyState>, take_over: bool) -> bool {
    legacy == Some(LegacyState::Absent) || (legacy == Some(LegacyState::Stopped) && take_over)
}

pub fn enabled() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

pub fn explicitly_enabled() -> bool {
    std::env::var("SCHEDULER_EMBEDDED_INFERENCE").as_deref() == Ok("true")
}

pub fn origin() -> String {
    format!("http://127.0.0.1:{PORT}")
}

pub fn start(database_url: &str) -> Option<tokio::task::JoinHandle<()>> {
    if !explicitly_enabled() && !crate::managed_nodes::enabled() {
        return None;
    }

    let database_url = database_url.to_owned();
    Some(tokio::spawn(async move {
        // The node manager already initializes the shared ModelInfer tables.
        // Wait for that initialization before the embedded runtime syncs them.
        if crate::managed_nodes::enabled() {
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(Duration::from_secs(3))
                .build()
                .expect("local node manager client");
            let url = format!("{}/nodes", crate::managed_nodes::origin());
            loop {
                if client.get(&url).send().await.is_ok_and(|response| response.status().is_success()) {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        loop {
            let legacy = lookup_legacy_state().await;
            let take_over = legacy == Some(LegacyState::Stopped)
                && (explicitly_enabled() || has_imported_deployments(&database_url).await);
            if !may_start(legacy, take_over) {
                tokio::time::sleep(Duration::from_secs(10)).await;
                continue;
            }
            let child = tokio::process::Command::new("modelinfer")
                .args([
                    "--bind-address",
                    "127.0.0.1",
                    "--port",
                    &PORT.to_string(),
                    "--frontend-dir",
                    "/opt/v-scheduler/inference-ui",
                    "--data-dir",
                    "/var/lib/modelinfer",
                    "--url-base",
                    "/",
                    "--require-temporal-worker",
                ])
                .env("VOS_MODELINFER_DATABASE_URL", &database_url)
                .env("VOS_MODELINFER_TASK_QUEUE", "v-scheduler-inference")
                .env("MODELINFER_PLATFORM_APP_ID", "com.ictrek.v-scheduler")
                .env("MODELINFER_MODEL_HUB_DIR", "/data")
                .env_remove("SCHEDULER_PGV_BOOTSTRAP_PASSWORD")
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .spawn();
            match child {
                Ok(mut child) => {
                    ACTIVE.store(true, Ordering::Release);
                    tokio::select! {
                        result = child.wait() => match result {
                            Ok(status) => eprintln!("Inference runtime exited ({status})"),
                            Err(error) => eprintln!("Failed to wait for inference runtime: {error}"),
                        },
                        _ = watch_legacy_owner() => {
                            ACTIVE.store(false, Ordering::Release);
                            let _ = child.kill().await;
                        }
                    }
                }
                Err(error) => eprintln!("Failed to start inference runtime: {error}"),
            }
            ACTIVE.store(false, Ordering::Release);
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }))
}

async fn has_imported_deployments(database_url: &str) -> bool {
    use sqlx::{Connection, PgConnection};

    let Ok(Ok(mut db)) = tokio::time::timeout(Duration::from_secs(3), PgConnection::connect(database_url)).await else {
        return false;
    };
    let exists: bool = sqlx::query_scalar("SELECT to_regclass('public.vrs_modelinfer_migration') IS NOT NULL")
        .fetch_one(&mut db).await.unwrap_or(false);
    if !exists {
        return false;
    }
    sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM public.vrs_modelinfer_migration WHERE id)")
        .fetch_one(&mut db).await.unwrap_or(false)
}

async fn lookup_legacy_state() -> Option<LegacyState> {
    let endpoint = crate::model::VosEndpoint {
        url: crate::identity::base()?,
        token: crate::modelinfer::context_token()?,
    };
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .build().ok()?;
    let legacy = crate::vos::get_envelope(&client, &endpoint, "/v1000/app/detail?app_id=com.ictrek.modelinfer")
        .await.ok()?;
    legacy_state(&legacy)
}

async fn watch_legacy_owner() {
    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        if lookup_legacy_state().await == Some(LegacyState::Installed) {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn starts_after_legacy_uninstall_without_requiring_an_import_or_override() {
        assert!(may_start(legacy_state(&json!({"code":10067})), false));
        assert!(may_start(legacy_state(&json!({"code":0,"data":{"state":"not_installed"}})), false));
        let stopped = legacy_state(&json!({"code":0,"data":{"state":"stopped"}}));
        assert!(!may_start(stopped, false));
        assert!(may_start(stopped, true));
        assert!(!may_start(legacy_state(&json!({"code":0,"data":{"state":"installed"}})), true));
        assert!(!may_start(legacy_state(&json!({"code":401})), true));
    }
}
