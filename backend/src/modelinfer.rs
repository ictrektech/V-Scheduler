//! Read-only discovery of the existing ModelInfer inventory. SSH credentials,
//! deployment lifecycle and cluster orchestration remain owned by ModelInfer.
use crate::model::{Config, VosEndpoint};
use anyhow::{Context, ensure};
use axum::http::HeaderMap;
use reqwest::Client;
use serde_json::{Value, json};
use sqlx::{PgPool, Row};
use std::time::Duration;

fn identifier(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 96
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn text(value: &Value, limit: usize) -> Option<&str> {
    value
        .as_str()
        .filter(|s| s.len() <= limit && !s.chars().any(char::is_control))
}
fn inventory(rows: &[Value]) -> anyhow::Result<Value> {
    ensure!(rows.len() <= 1000, "too many nodes");
    let mut ids = std::collections::HashSet::new();
    let mut result = Vec::new();
    for row in rows {
        let id = row["id"].as_str().context("missing node ID")?;
        ensure!(identifier(id) && ids.insert(id), "invalid node identity");
        let devices = row["gpu_devices"]
            .as_array()
            .context("missing GPU inventory")?;
        ensure!(devices.len() <= 32, "too many GPUs");
        let gpus=devices.iter().map(|g|json!({"index":g["index"].as_u64(),"name":text(&g["name"],128),"memory_total_mib":g["memory_total_mib"].as_u64()})).collect::<Vec<_>>();
        result.push(json!({"id":id,"display_name":text(&row["name"],128).unwrap_or(id),"arch":text(&row["arch"],32),"gpu_model":text(&row["gpu_model"],128),"gpus":gpus,"status":text(&row["status"],32),"busy":row["busy"].as_bool(),"monitor_state":text(&row["monitor_state"],32)}));
    }
    Ok(json!(result))
}

// Keep numeric measurements only. Prometheus labels contain host/network IDs.
fn metrics(value: &Value) -> Value {
    let mut result = serde_json::Map::new();
    let now = chrono::Utc::now().timestamp() as f64;
    if let Some(groups) = value["groups"].as_array() {
        for group in groups {
            let Some(key) = group["key"].as_str().filter(|key| {
                [
                    "cpu_usage",
                    "mem_usage",
                    "gpu_util",
                    "gpu_mem_used",
                    "gpu_mem_total",
                    "gpu_proc_mem",
                ]
                .contains(key)
            }) else {
                continue;
            };
            let samples = group["series"]
                .as_array()
                .into_iter()
                .flatten()
                .take(32)
                .filter_map(|series| {
                    let point = series["points"].as_array()?.last()?.as_array()?;
                    let time = point.first()?.as_f64()?;
                    let value = point.get(1)?.as_f64()?;
                    if !time.is_finite()
                        || !value.is_finite()
                        || value < 0.0
                        || now - time > 90.0
                        || time - now > 5.0
                    {
                        return None;
                    }
                    if ["cpu_usage", "mem_usage", "gpu_util"].contains(&key) && value > 100.0 {
                        return None;
                    }
                    Some(json!({"observed_at":time,"value":value}))
                })
                .collect::<Vec<_>>();
            result.insert(key.to_string(), json!(samples));
        }
    }
    json!({"measurements":result,"execution_enabled":false,"control_state":"pending_adapter_verification"})
}

/// 应用上下文 access-token：定期发现没有管理员请求可借用凭据。
fn context_token() -> Option<String> {
    std::fs::read_to_string("/var/lib/vos-context/access-token")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}
fn client() -> anyhow::Result<Client> {
    Ok(Client::builder()
        .timeout(Duration::from_secs(3))
        .redirect(reqwest::redirect::Policy::none())
        .build()?)
}
/// 解析 ModelInfer 组件地址（应用已安装配置）；未安装返回 `None`。
async fn component(
    client: &Client,
    platform: &VosEndpoint,
) -> anyhow::Result<Option<VosEndpoint>> {
    let apps = crate::vos::get(client, platform, "/v1000/app/list?limit=1000").await?;
    let list = apps["data"]
        .as_array()
        .context("missing application list")?;
    ensure!(
        apps["total"].as_u64() == Some(list.len() as u64),
        "partial application list"
    );
    if !list
        .iter()
        .any(|a| a["app_id"] == "com.ictrek.modelinfer" && a["state"] == "installed")
    {
        return Ok(None);
    }
    let detail = crate::vos::get(
        client,
        platform,
        "/v1000/app/detail?app_id=com.ictrek.modelinfer",
    )
    .await?;
    let configs = if let Some(raw) = detail["configs_json"].as_str() {
        serde_json::from_str::<Value>(raw)?
    } else {
        detail["configs_json"].clone()
    };
    // ModelInfer's VOS component listens on the host (default 19908). Resolve
    // its approved installation config; never follow inventory host/SSH URLs.
    let port = configs["VOS_MODELINFER_PORT"]
        .as_u64()
        .or_else(|| configs["VOS_MODELINFER_PORT"].as_str()?.parse::<u64>().ok())
        .unwrap_or(19908);
    ensure!((1..=65535).contains(&port), "invalid component port");
    let mut origin = reqwest::Url::parse(&platform.url)?;
    ensure!(
        ["http", "https"].contains(&origin.scheme())
            && origin.host_str().is_some()
            && origin.username().is_empty()
            && origin.password().is_none(),
        "invalid platform origin"
    );
    origin
        .set_scheme("http")
        .map_err(|_| anyhow::anyhow!("invalid local origin"))?;
    origin
        .set_port(Some(port as u16))
        .map_err(|_| anyhow::anyhow!("invalid local port"))?;
    origin.set_path("");
    origin.set_query(None);
    origin.set_fragment(None);
    let prefix = configs["VOS_MODELINFER_URL_BASE"]
        .as_str()
        .unwrap_or("/app/com.ictrek.modelinfer")
        .trim_end_matches('/');
    ensure!(
        prefix.is_empty()
            || (prefix.starts_with('/')
                && !prefix.starts_with("//")
                && !prefix.contains('?')
                && !prefix.contains('#')
                && !prefix.contains("..")),
        "invalid component prefix"
    );
    Ok(Some(VosEndpoint {
        url: format!("{}{}", origin.as_str().trim_end_matches('/'), prefix),
        token: String::new(),
    }))
}
async fn collect(db: &PgPool, headers: &HeaderMap) -> anyhow::Result<Value> {
    let url = crate::identity::base().context("missing platform source")?;
    let token = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .context("missing administrator")?;
    let platform = VosEndpoint {
        url,
        token: token.into(),
    };
    let client = client()?;
    let Some(source) = component(&client, &platform).await? else {
        crate::vos::store(db, "modelinfer:inventory", Ok(json!([]))).await;
        return Ok(json!({"state":"not_installed","node_count":0}));
    };
    let mut rows = Vec::new();
    let mut expected = None;
    let mut pages = 1;
    for page in 1..=10 {
        let data = crate::vos::get(
            &client,
            &source,
            &format!("/api/nodes?page_num={page}&page_size=100"),
        )
        .await?;
        let total = data["total"].as_u64().context("missing node count")?;
        ensure!(
            total <= 1000 && expected.is_none_or(|old| old == total),
            "inventory changed while paging"
        );
        expected = Some(total);
        let entries = data["data"].as_array().context("missing nodes")?;
        rows.extend_from_slice(entries);
        if page == 1 {
            pages = data["pages"].as_u64().unwrap_or(1).max(1);
            ensure!(pages <= 10, "too many pages");
        }
        if page >= pages {
            break;
        }
    }
    ensure!(
        rows.len() as u64 == expected.unwrap_or(0),
        "partial node inventory"
    );
    let mut snapshot = inventory(&rows)?;
    // Existing ModelInfer monitors are owned and deployed by the VOS App.
    // Never install a second remote Agent or contact inventory-provided URLs.
    let mut tasks = tokio::task::JoinSet::new();
    for row in rows.iter().take(32) {
        let id = row["id"].as_str().unwrap_or_default().to_string();
        let c = client.clone();
        let src = source.clone();
        tasks.spawn(async move {
            let data = crate::vos::get(&c, &src, &format!("/api/nodes/{id}/metrics")).await;
            (id, data)
        });
    }
    while let Some(result) = tasks.join_next().await {
        let Ok((id, Ok(data))) = result else {
            continue;
        };
        if let Some(node) = snapshot
            .as_array_mut()
            .and_then(|nodes| nodes.iter_mut().find(|n| n["id"] == id))
        {
            node["telemetry"] = metrics(&data);
        }
    }
    crate::vos::store(db, "modelinfer:inventory", Ok(snapshot)).await;
    Ok(json!({"state":"ready","node_count":rows.len()}))
}
/// 接入地址必须是可路由的纯 origin + path（拒绝凭据 / 查询串 / 穿越路径）。
fn service_url(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() || raw.len() > 512 || raw.contains("..") {
        return None;
    }
    let url = reqwest::Url::parse(raw).ok()?;
    (["http", "https"].contains(&url.scheme())
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none())
        .then(|| url.as_str().trim_end_matches('/').to_string())
}
/// 运行中的部署 = 已登记模型卡片的可路由实例（目录项，不创建执行器/预算）。
async fn services(
    client: &Client,
    source: &VosEndpoint,
) -> anyhow::Result<Vec<crate::router::ModelinferService>> {
    let mut rows = Vec::new();
    for page in 1..=10 {
        let data = crate::vos::get(
            client,
            source,
            &format!(
                "/api/deployments?page_num={page}&page_size=100&state=all&sort=name"
            ),
        )
        .await?;
        let entries = data["data"]
            .as_array()
            .context("missing deployments")?;
        rows.extend_from_slice(entries);
        let total = data["total"]
            .as_u64()
            .context("missing deployment count")?;
        ensure!(total <= 1000, "too many deployments");
        let pages = data["pages"].as_u64().unwrap_or(1).max(1);
        ensure!(pages <= 10, "too many pages");
        if page >= pages {
            break;
        }
    }
    let mut result = Vec::new();
    for row in rows
        .iter()
        .filter(|row| matches!(row["state"].as_str(), Some("running" | "degraded")))
        .take(100)
    {
        let id = text(&row["id"], 96).context("missing deployment ID")?;
        ensure!(identifier(id), "invalid deployment identity");
        let access = crate::vos::get(
            client,
            source,
            &format!("/api/deployments/{id}/access"),
        )
        .await?;
        if access["available"] != true {
            continue;
        }
        let Some(base_url) = service_url(access["base_url"].as_str().unwrap_or_default()) else {
            continue;
        };
        let model = text(&access["model"], 128)
            .filter(|model| !model.trim().is_empty())
            .context("missing served model")?;
        result.push(crate::router::ModelinferService {
            deployment: id.to_string(),
            name: text(&row["name"], 128).unwrap_or(id).to_string(),
            base_url,
            api_key: text(&access["api_key"], 512).unwrap_or_default().to_string(),
            model: model.to_string(),
        });
    }
    Ok(result)
}
/// 定期发现 ModelInfer 运行中的部署并发布为网关目录项（无租约、无执行预算）。
pub async fn sync() {
    let Some(token) = context_token() else {
        return;
    };
    let Some(url) = crate::identity::base() else {
        return;
    };
    let Ok(client) = client() else {
        return;
    };
    let platform = VosEndpoint { url, token };
    let component = match component(&client, &platform).await {
        Ok(source) => source,
        // 平台 / 组件暂时不可用：保留上一次目录，等待下一轮。
        Err(_) => return,
    };
    let Some(source) = component else {
        crate::router::sync_modelinfer(Vec::new());
        return;
    };
    if let Ok(services) = services(&client, &source).await {
        crate::router::sync_modelinfer(services);
    }
}
pub async fn refresh(db: &PgPool, headers: &HeaderMap) {
    let fresh:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM vrs_observations WHERE id='modelinfer:status' AND observed_at>now()-interval '30 seconds')").fetch_one(db).await.unwrap_or(true);
    if fresh {
        return;
    }
    let status = match collect(db, headers).await {
        Ok(status) => status,
        Err(_) => {
            crate::vos::store(
                db,
                "modelinfer:inventory",
                Err(anyhow::anyhow!("source unavailable")),
            )
            .await;
            json!({"state":"unavailable"})
        }
    };
    crate::vos::store(db, "modelinfer:status", Ok(status)).await;
}
pub async fn decorate(
    db: &PgPool,
    config: &Config,
    views: &mut Vec<Value>,
) -> Result<(), sqlx::Error> {
    let row = sqlx::query(
        "SELECT data,observed_at,last_error FROM vrs_observations WHERE id='modelinfer:inventory'",
    )
    .fetch_optional(db)
    .await?;
    let Some(row) = row else {
        return Ok(());
    };
    let time: Option<chrono::DateTime<chrono::Utc>> = row.get("observed_at");
    let fresh = row.get::<Option<String>, _>("last_error").is_none()
        && time.is_some_and(|t| (0..90).contains(&(chrono::Utc::now() - t).num_seconds()));
    let data: Value = row.get("data");
    let plans=sqlx::query("SELECT source_id,display_name,definition,version FROM vrs_source_node_settings WHERE source='modelinfer'").fetch_all(db).await?;
    if let Some(nodes) = data.as_array() {
        for source in nodes {
            let id = source["id"].as_str().unwrap_or_default();
            let plan = plans.iter().find(|p| p.get::<String, _>("source_id") == id);
            let settings=plan.map(|p|json!({"display_name":p.get::<String,_>("display_name"),"definition":p.get::<Value,_>("definition"),"version":p.get::<i64,_>("version")}));
            if let Some(bound) = config
                .nodes
                .iter()
                .find(|n| n.discovery_source == "modelinfer" && n.discovery_id == id)
            {
                if let Some(view) = views.iter_mut().find(|v| v["id"] == bound.id) {
                    view["modelinfer_source"] =
                        json!({"fresh":fresh,"observed_at":time,"node":source});
                    view["source_settings"] = json!(settings);
                    view["discovery_id"] = json!(id);
                    view["source_hardware"] = json!({"gpu_model":source["gpu_model"]});
                }
            } else {
                views.push(json!({"source_settings":settings,"source_control":{"transport":"vos-app","metrics":true,"execution_enabled":false,"state":"pending_adapter_verification"},"id":format!("modelinfer:{id}"),"display_name":source["display_name"],"discovery_source":"modelinfer","discovery_id":id,"managed":false,"capacities":{"cpu_millicores":0,"memory_mib":0,"gpu_slots":0,"gpu_memory_mib":0},"heartbeat":null,"report":{},"draining":true,"health_state":"observing","source_hardware":{"arch":source["arch"],"gpu_model":source["gpu_model"],"gpus":source["gpus"]},"modelinfer_source":{"fresh":fresh,"observed_at":time,"node":source}}));
            }
        }
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_urls_reject_credentials_queries_and_traversal() {
        assert_eq!(
            service_url("http://10.0.0.8:8000/v1/").as_deref(),
            Some("http://10.0.0.8:8000/v1")
        );
        for raw in [
            "",
            "10.0.0.8:8000/v1",
            "http://user:pass@10.0.0.8:8000/v1",
            "http://10.0.0.8:8000/v1?x=1",
            "http://10.0.0.8:8000/../admin",
            "ftp://10.0.0.8/v1",
        ] {
            assert_eq!(service_url(raw), None, "{raw}");
        }
    }
    #[test]
    fn source_metrics_redact_labels_and_reject_stale_samples() {
        let now = chrono::Utc::now().timestamp();
        let out = metrics(
            &json!({"groups":[{"key":"cpu_usage","series":[{"labels":{"instance":"private-host"},"points":[[now,12.5]]}]},{"key":"mem_usage","series":[{"points":[[now-1000,90]]}]},{"key":"private_metric","series":[]}]}),
        );
        assert_eq!(out["measurements"]["cpu_usage"][0]["value"], 12.5);
        assert_eq!(out["measurements"]["mem_usage"], json!([]));
        assert!(!out.to_string().contains("private"));
        assert_eq!(out["execution_enabled"], false);
    }
    #[test]
    fn discovery_never_copies_ssh_credentials_or_trusts_capacity_claims() {
        let row = json!({"id":"spark-1","name":"Spark 1","arch":"aarch64","gpu_model":"NVIDIA GB10","gpu_devices":[{"index":0,"name":"GB10","memory_total_mib":131072}],"busy":true,"ssh_user":"private-user","password":"private-secret","has_key":true,"monitor_last_error":"private-error","capacities":{"gpu_slots":999}});
        let out = inventory(&[row.clone()]).unwrap();
        let text = out.to_string();
        assert!(
            !text.contains("private-")
                && !text.contains("ssh_user")
                && !text.contains("capacities")
        );
        assert_eq!(out[0]["busy"], true);
        assert!(inventory(&[row.clone(), row]).is_err());
    }
}
