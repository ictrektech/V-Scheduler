//! Read-only VOS adapter. Inventory and monitoring cannot grant execution rights.
use crate::model::{Config, VosEndpoint};
use anyhow::{Context, ensure};
use chrono::{DateTime, Utc};
use reqwest::{Client, Url};
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::{sync::Arc, time::Duration};

fn base(endpoint: &VosEndpoint) -> anyhow::Result<Url> {
    let url = Url::parse(&endpoint.url)?;
    ensure!(
        matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
        "invalid VOS URL"
    );
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "VOS URL must not contain credentials, query or fragment"
    );
    Ok(url)
}
async fn get(client: &Client, endpoint: &VosEndpoint, path: &str) -> anyhow::Result<Value> {
    let url = format!("{}{}", endpoint.url.trim_end_matches('/'), path);
    let mut req = client.get(url);
    if !endpoint.token.is_empty() {
        req = req.bearer_auth(&endpoint.token);
    }
    let mut response = req.send().await?;
    ensure!(response.status().is_success(), "VOS request rejected");
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            body.len() + chunk.len() <= 2_097_152,
            "VOS response too large"
        );
        body.extend_from_slice(&chunk);
    }
    let value: Value = serde_json::from_slice(&body)?;
    ensure!(value["code"] == 0, "VOS response failed");
    Ok(value["data"].clone())
}
fn inventory(data: &Value) -> anyhow::Result<Value> {
    let items = data["nodes"].as_array().context("missing nodes")?;
    ensure!(items.len() <= 1000, "too many nodes");
    let mut nodes = Vec::new();
    let mut ids = std::collections::HashSet::new();
    for node in items {
        let id = node["member_id"].as_str().context("missing member ID")?;
        ensure!(
            !id.is_empty() && id.len() <= 128 && ids.insert(id),
            "invalid or duplicate member ID"
        );
        // Never persist raw upstream payloads: allowlist excludes credentials.
        nodes.push(json!({"member_id":id,"display_name":node["display_name"].as_str(),"role":node["role"].as_str(),"node_ip":node["node_ip"].as_str()}));
    }
    Ok(json!(nodes))
}
fn telemetry(info: &Value, usage: &Value) -> anyhow::Result<Value> {
    ensure!(
        info["cpu_logical_count"].as_u64().is_some(),
        "missing logical CPU count"
    );
    let total = info["memory"]["size"]
        .as_f64()
        .context("missing memory capacity")?;
    ensure!(
        info["memory"]["unit"] == "B" && total > 0.0,
        "invalid memory units"
    );
    let used = usage["ram"]["used"].as_f64().context("missing RAM usage")?;
    let ram_total = usage["ram"]["size"].as_f64().context("missing RAM total")?;
    let cpu = usage["cpu"]["used"].as_f64().context("missing CPU usage")?;
    ensure!(
        usage["ram"]["unit"] == "B"
            && ram_total > 0.0
            && used >= 0.0
            && used <= ram_total
            && (0.0..=100.0).contains(&cpu),
        "invalid usage"
    );
    Ok(
        json!({"cpu_logical_count":info["cpu_logical_count"],"cpu_used_percent":cpu,"memory_total_mib":total/1048576.0,"memory_used_mib":used/1048576.0,"disk": {"size_bytes":usage["disk"]["size"].as_f64(),"used_bytes":usage["disk"]["used"].as_f64()},"gpu_usage":null,"gpu_status":"UNSUPPORTED_BY_VOS"}),
    )
}
fn applications(data: &Value) -> anyhow::Result<Value> {
    let items = data["data"].as_array().context("missing apps")?;
    ensure!(
        items.len() <= 1000 && data["total"].as_u64() == Some(items.len() as u64),
        "partial app inventory"
    );
    let mut views = Vec::new();
    for app in items {
        let id = app["app_id"].as_str().context("missing app ID")?;
        let instance = app["installed_app_id"]
            .as_str()
            .context("missing installed ID")?;
        // An installation state is not proof that an executor process is healthy.
        views.push(json!({"app_id":id,"installed_app_id":instance,"version":app["version"].as_str(),"state":app["state"].as_str()}));
    }
    Ok(json!(views))
}
async fn store(db: &PgPool, id: &str, result: anyhow::Result<Value>) {
    // Deliberately exclude error text which may embed URLs or credentials.
    let query = match result {
        Ok(value) => sqlx::query("INSERT INTO vrs_observations(id,data,observed_at) VALUES($1,$2,now()) ON CONFLICT(id) DO UPDATE SET data=excluded.data,observed_at=excluded.observed_at,last_error=NULL").bind(id).bind(value),
        Err(_) => sqlx::query("INSERT INTO vrs_observations(id,last_error) VALUES($1,'SOURCE_UNAVAILABLE') ON CONFLICT(id) DO UPDATE SET last_error=excluded.last_error").bind(id),
    };
    let _ = query.execute(db).await;
}
pub fn spawn(db: PgPool, config: Arc<Config>) -> anyhow::Result<()> {
    for source in config
        .vos_cluster
        .iter()
        .chain(config.nodes.iter().filter_map(|n| n.vos.as_ref()))
    {
        base(source)?;
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()?;
    tokio::spawn(async move {
        loop {
            if let Some(source) = &config.vos_cluster {
                let result = async { inventory(&get(&client, source, "/api/nodes").await?) }.await;
                store(&db, "vos:inventory", result).await;
            }
            // Endpoints are operator configured; never follow node_ip/api_endpoint from discovery.
            for node in &config.nodes {
                if let Some(source) = &node.vos {
                    let result = async {
                        let info = get(&client, source, "/v1000/nas/device/info").await?;
                        let usage = get(&client, source, "/v1000/nas/device_usage").await?;
                        telemetry(&info, &usage)
                    }
                    .await;
                    store(&db, &format!("vos:metrics:{}", node.id), result).await;
                    let apps =
                        async { applications(&get(&client, source, "/v1000/app/list").await?) }
                            .await;
                    store(&db, &format!("vos:apps:{}", node.id), apps).await;
                }
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
    Ok(())
}
fn observation(row: &sqlx::postgres::PgRow) -> Value {
    let at: Option<DateTime<Utc>> = row.get("observed_at");
    let error: Option<String> = row.get("last_error");
    json!({"data":row.get::<Value,_>("data"),"observed_at":at,"fresh":error.is_none() && at.is_some_and(|at|(Utc::now()-at).num_seconds()<90),"error_code":error})
}
pub async fn decorate(
    db: &PgPool,
    config: &Config,
    views: &mut Vec<Value>,
) -> Result<(), sqlx::Error> {
    let rows = sqlx::query("SELECT * FROM vrs_observations")
        .fetch_all(db)
        .await?;
    let source = rows
        .iter()
        .find(|r| r.get::<String, _>("id") == "vos:inventory")
        .map(observation);
    for view in views.iter_mut() {
        let id = view["id"].as_str().unwrap_or_default().to_owned();
        view["managed"] = json!(true);
        let bound = config.nodes.iter().find(|n| n.id == id);
        let member = bound.map(|n| n.vos_member_id.as_str()).unwrap_or_default();
        if let Some(source) = &source {
            view["vos_inventory"] = json!({"observed_at":source["observed_at"],"fresh":source["fresh"],"error_code":source["error_code"]});
            if !member.is_empty() {
                let entry = source["data"]
                    .as_array()
                    .and_then(|items| items.iter().find(|n| n["member_id"] == member));
                view["vos_node"] = entry.cloned().unwrap_or(Value::Null);
                view["vos_present"] = json!(entry.is_some());
            }
        }
        if let Some(row) = rows
            .iter()
            .find(|r| r.get::<String, _>("id") == format!("vos:metrics:{id}"))
        {
            view["vos_metrics"] = observation(row);
        }
        if let Some(row) = rows
            .iter()
            .find(|r| r.get::<String, _>("id") == format!("vos:apps:{id}"))
        {
            view["vos_applications"] = observation(row);
        }
        view["executors"] = json!(
            config
                .executors
                .iter()
                .filter(|e| e.node_id == id)
                .collect::<Vec<_>>()
        );
    }
    if let Some(source) = source {
        if let Some(items) = source["data"].as_array() {
            for item in items {
                if config
                    .nodes
                    .iter()
                    .any(|n| !n.vos_member_id.is_empty() && item["member_id"] == n.vos_member_id)
                {
                    continue;
                }
                views.push(json!({"id":format!("vos:{}",item["member_id"].as_str().unwrap_or_default()),"managed":false,"capacities":{"cpu_millicores":0,"memory_mib":0,"gpu_slots":0,"gpu_memory_mib":0},"heartbeat":null,"report":{},"draining":true,"vos_node":item,"vos_inventory":{"observed_at":source["observed_at"],"fresh":source["fresh"],"error_code":source["error_code"]}}));
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imports_only_public_metadata_and_never_credentials() {
        let value=inventory(&json!({"nodes":[{"member_id":"member-1","role":"worker","display_name":"GPU","node_ip":"127.0.0.1","admin_password":"do-not-copy","api_endpoint":"http://untrusted"}]})).unwrap();
        assert!(value[0].get("admin_password").is_none());
        assert!(value[0].get("api_endpoint").is_none());
        assert!(inventory(&json!({"nodes":[{"member_id":"x"},{"member_id":"x"}]})).is_err());
        assert!(inventory(&json!({})).is_err());
        assert_eq!(inventory(&json!({"nodes":[]})).unwrap(), json!([]));
    }
    #[test]
    fn correct_units_and_unknown_gpu_instead_of_zero() {
        let info = json!({"cpu_logical_count":8,"memory":{"size":8388608,"unit":"B"}});
        let mut usage = json!({"cpu":{"used":25},"ram":{"size":8388608,"used":2097152,"unit":"B"},"gpu":{"used":0}});
        let value = telemetry(&info, &usage).unwrap();
        assert_eq!(value["memory_used_mib"], 2.0);
        assert_eq!(value["cpu_used_percent"], 25.0);
        assert!(value["gpu_usage"].is_null());
        usage["ram"]["unit"] = json!("GB");
        assert!(telemetry(&info, &usage).is_err());
    }
    #[test]
    fn rejects_url_credentials_and_queries() {
        let endpoint = |url: &str| VosEndpoint {
            url: url.to_owned(),
            token: String::new(),
        };
        assert!(base(&endpoint("http://localhost:19906")).is_ok());
        assert!(base(&endpoint("http://user:secret@localhost")).is_err());
        assert!(base(&endpoint("http://localhost?token=secret")).is_err());
        assert!(base(&endpoint("file:///tmp/source")).is_err());
    }
    #[test]
    fn app_inventory_is_complete_and_strips_private_fields() {
        let value=applications(&json!({"total":1,"data":[{"app_id":"com.ictrek.image-studio","installed_app_id":"instance-1","state":"installed","owner_user_id":"private","configs_json":{"secret":"private"}}]})).unwrap();
        assert!(value[0].get("owner_user_id").is_none());
        assert!(value[0].get("configs_json").is_none());
        assert!(applications(&json!({"total":2,"data":[]})).is_err());
    }
}
