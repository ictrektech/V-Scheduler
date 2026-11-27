use std::{process::Stdio, time::Duration};
use serde_json::Value;

pub fn enabled() -> bool {
    std::env::var("SCHEDULER_EMBEDDED_CLUSTER_MANAGER").as_deref() == Ok("true")
}

pub const ORIGIN: &str = "http://127.0.0.1:38123";

pub async fn members(client: &reqwest::Client) -> Option<Vec<Value>> {
    let response = client
        .get(format!("{ORIGIN}/api/nodes"))
        .timeout(Duration::from_secs(2))
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .json::<Value>()
        .await
        .ok()?;
    if response["code"] != 0 {
        return None;
    }
    response["data"]["nodes"].as_array().cloned()
}

pub fn start(database_url: &str) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled() {
        return None;
    }
    let database_url = database_url.to_owned();
    Some(tokio::spawn(async move {
        loop {
            let process = tokio::process::Command::new("vos-cluster-manager")
                .args([
                    "--bind-address", "127.0.0.1", "--port", "38123",
                    "--frontend-dir", "/opt/v-scheduler/no-cluster-ui",
                    "--data-dir", "/var/lib/vos-cluster-manager", "--url-base", "/",
                ])
                .env("VOS_CLUSTER_MANAGER_DATABASE_URL", &database_url)
                .env_remove("SCHEDULER_PGV_BOOTSTRAP_PASSWORD")
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .spawn();
            match process {
                Ok(mut child) => match child.wait().await {
                    Ok(status) => eprintln!("Cluster Manager exited ({status})"),
                    Err(error) => eprintln!("Failed to wait for Cluster Manager: {error}"),
                },
                Err(error) => eprintln!("Failed to start Cluster Manager: {error}"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }))
}
