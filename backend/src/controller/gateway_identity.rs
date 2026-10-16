//! Fixed HybRAG background inference identity. This never grants a scheduler
//! executor, a management role, or a caller-selected callback address.
use super::app_bindings::{bounded_json, read_private, save_private};
use super::{Context, Digest, PathBuf, Sha256, Uuid, Value, ensure, json};
use std::{path::Path, time::Duration};
const APP: &str = "com.ictrek.hybrag";
const ORIGIN: &str = "http://hybrag-app:8080";
const ENDPOINT: &str = "/api/v1/scheduler/gateway-binding";
const URL: &str = "http://v-router:8080/v1";
const SCOPE: &str = "gateway-inference-v1";

fn binding(old: Option<&Value>) -> anyhow::Result<Value> {
    if let Some(state) = old {
        let value = &state["binding"];
        ensure!(
            value["protocol_version"] == 1
                && value["app_id"] == APP
                && value["url"] == URL
                && value["scope"] == SCOPE,
            "invalid persisted gateway identity"
        );
        let revision = value["binding_revision"]
            .as_u64()
            .filter(|v| *v > 0)
            .context("gateway identity revision required")?;
        let token = value["token"]
            .as_str()
            .context("gateway identity credential required")?;
        ensure!(
            token.starts_with("vs-app-")
                && token.len() == 71
                && token[7..].bytes().all(|c| c.is_ascii_hexdigit()),
            "invalid persisted gateway credential"
        );
        if state["active"] == true {
            return Ok(value.clone());
        }
        return Ok(new_binding(
            revision
                .checked_add(1)
                .context("gateway identity revision exhausted")?,
        ));
    }
    Ok(new_binding(1))
}
fn new_binding(revision: u64) -> Value {
    json!({"protocol_version":1,"binding_revision":revision,"app_id":APP,"url":URL,"token":format!("vs-app-{}{}",Uuid::new_v4().simple(),Uuid::new_v4().simple()),"scope":SCOPE})
}
fn snapshot(binding: &Value, enabled: bool) -> anyhow::Result<Value> {
    let token = binding["token"]
        .as_str()
        .context("gateway credential missing")?;
    Ok(
        json!({"protocol_version":1,"binding_revision":binding["binding_revision"],"app_id":APP,"scope":SCOPE,"enabled":enabled,"token_hash":format!("{:x}",Sha256::digest(token.as_bytes()))}),
    )
}
fn declaration(value: &Value) -> anyhow::Result<()> {
    ensure!(
        value["protocol_version"] == 1
            && value["app_id"] == APP
            && value["binding_endpoint"] == ENDPOINT
            && value["capabilities"]
                .as_array()
                .is_some_and(|caps| caps.iter().any(|v| v == SCOPE)),
        "HybRAG inference identity capability required"
    );
    ensure!(
        ["scheduler", "model-hub"].contains(&value["backend"].as_str().unwrap_or("")),
        "HybRAG backend declaration required"
    );
    Ok(())
}
fn deactivate(path: &Path, gateway: &Path, old: Option<&Value>) -> anyhow::Result<()> {
    if let Some(old) = old {
        save_private(gateway, &snapshot(&old["binding"], false)?)?;
        let mut state = old.clone();
        state["active"] = json!(false);
        save_private(path, &state)?;
    }
    Ok(())
}
async fn reconcile(configuration: &Path, client: &reqwest::Client) -> anyhow::Result<()> {
    let path = configuration
        .parent()
        .context("configuration directory required")?
        .join("gateway-bindings/hybrag.json");
    let gateway = PathBuf::from(
        std::env::var("SCHEDULER_GATEWAY_DATA").unwrap_or_else(|_| "/gateway-data".into()),
    )
    .join("app-identities/hybrag.json");
    let old = if path.exists() {
        Some(read_private(&path)?)
    } else {
        None
    };
    let token = std::fs::read_to_string("/var/lib/vos-context/access-token")?;
    let vos = crate::identity::base().context("VOS identity endpoint required")?;
    let identity = bounded_json(
        client
            .get(format!(
                "{}/v1000/oauth2/userinfo",
                vos.trim_end_matches('/')
            ))
            .bearer_auth(token.trim())
            .send()
            .await?,
    )
    .await?;
    ensure!(
        identity["sub"] == "com.ictrek.v-scheduler" && identity["enable"] == 1,
        "Scheduler application identity required"
    );
    let installed = bounded_json(
        client
            .get(format!(
                "{}/v1000/app/detail?app_id={APP}",
                vos.trim_end_matches('/')
            ))
            .bearer_auth(token.trim())
            .send()
            .await?,
    )
    .await?;
    if installed["code"] == 10067
        || (installed["code"] == 0 && installed["data"]["state"] != "installed")
    {
        return deactivate(&path, &gateway, old.as_ref());
    }
    ensure!(
        installed["code"] == 0 && installed["data"]["state"] == "installed",
        "HybRAG must be installed on this VOS"
    );
    let discovery = bounded_json(client.get(format!("{ORIGIN}{ENDPOINT}")).send().await?).await?;
    declaration(&discovery)?;
    if discovery["backend"] == "model-hub" {
        return deactivate(&path, &gateway, old.as_ref());
    }
    let binding = binding(old.as_ref())?;
    let state = json!({"binding":binding,"active":true});
    if old.as_ref() != Some(&state) {
        save_private(&path, &state)?;
    }
    // Activate the new digest before handing the key to its sole fixed owner.
    // A lost callback response is retried with the same persisted revision/key.
    save_private(&gateway, &snapshot(&binding, true)?)?;
    let acknowledgement = bounded_json(
        client
            .post(format!("{ORIGIN}{ENDPOINT}"))
            .bearer_auth(token.trim())
            .json(&binding)
            .send()
            .await?,
    )
    .await?;
    let acknowledgement = acknowledgement.get("data").unwrap_or(&acknowledgement);
    ensure!(
        acknowledgement["configured"] == true
            && acknowledgement["binding_revision"] == binding["binding_revision"],
        "HybRAG gateway identity pending acceptance"
    );
    Ok(())
}
pub(super) fn start(configuration: PathBuf) {
    if std::env::var_os("VOS_APP_VERSION").is_none() {
        return;
    }
    tokio::spawn(async move {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(8))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("gateway identity client");
        loop {
            if reconcile(&configuration, &client).await.is_err() {
                eprintln!("HybRAG inference identity pending");
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn gateway_identity_is_scoped_persistent_and_rotates_after_deactivation() {
        let first = binding(None).unwrap();
        let state = json!({"binding":first,"active":true});
        assert_eq!(binding(Some(&state)).unwrap(), first);
        let digest = snapshot(&first, true).unwrap();
        assert!(digest.get("token").is_none());
        assert_eq!(digest["token_hash"].as_str().unwrap().len(), 64);
        assert!(
            !digest
                .to_string()
                .contains(first["token"].as_str().unwrap())
        );
        let mut disabled = state.clone();
        disabled["active"] = json!(false);
        let next = binding(Some(&disabled)).unwrap();
        assert_eq!(next["binding_revision"], 2);
        assert_ne!(next["token"], first["token"]);
        let mut invalid = state;
        invalid["binding"]["url"] = json!("http://other/v1");
        assert!(binding(Some(&invalid)).is_err());
    }
    #[test]
    fn only_fixed_hybrag_capability_can_receive_identity() {
        let mut declared = json!({"protocol_version":1,"app_id":APP,"binding_endpoint":ENDPOINT,"capabilities":[SCOPE],"backend":"scheduler"});
        assert!(declaration(&declared).is_ok());
        declared["backend"] = json!("model-hub");
        assert!(declaration(&declared).is_ok());
        declared["binding_endpoint"] = json!("http://other/receive");
        assert!(declaration(&declared).is_err());
        declared["binding_endpoint"] = json!(ENDPOINT);
        declared["capabilities"] = json!([]);
        assert!(declaration(&declared).is_err());
    }
}
