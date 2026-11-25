use super::*;
use anyhow::{Context, ensure};
use std::{collections::BTreeMap, io::Write, os::unix::fs::OpenOptionsExt};

const ROLES: [&str; 3] = ["qa", "embedding", "rerank"];

pub(super) struct Policy {
    path: PathBuf,
    entries: Mutex<BTreeMap<&'static str, Entry>>,
}

struct Entry {
    model: String,
    enabled: bool,
    override_value: Option<bool>,
    generation: u64,
    phase: &'static str,
    error: Option<String>,
}

impl Policy {
    pub fn open(state: &Path) -> anyhow::Result<Self> {
        let path = state.with_file_name("ollama-preload.json");
        let saved: BTreeMap<String, bool> = if path.exists() {
            serde_json::from_value(private_json(&path, 4096)?)?
        } else {
            BTreeMap::new()
        };
        let entries = ROLES
            .into_iter()
            .zip(["qwen3.5:2b", "bge-m3", "qllama/bge-reranker-v2-m3:q8_0"])
            .map(|(role, default_model)| {
                let upper = role.to_ascii_uppercase();
                let enabled = saved.get(role).copied().unwrap_or_else(|| {
                    env(&format!("OLLAMA_{upper}_PRELOAD_ENABLED"), "false") == "true"
                });
                (
                    role,
                    Entry {
                        model: env(&format!("OLLAMA_{upper}_MODEL"), default_model),
                        enabled,
                        override_value: saved.get(role).copied(),
                        generation: 0,
                        phase: if enabled { "pending" } else { "disabled" },
                        error: None,
                    },
                )
            })
            .collect();
        Ok(Self {
            path,
            entries: Mutex::new(entries),
        })
    }

    pub fn snapshot(&self) -> Value {
        let entries = self.entries.lock().unwrap();
        json!({"items": ROLES.map(|role| {
            let entry = &entries[role];
            json!({"role":role,"model":entry.model,"enabled":entry.enabled,
                "state":entry.phase,"error":entry.error})
        })})
    }

    pub fn configure(&self, role: &str, enabled: bool) -> Api<()> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries
            .get(role)
            .ok_or_else(|| Failure::new(422, "invalid Ollama role"))?;
        if enabled == entry.enabled {
            return Ok(());
        }
        let saved: BTreeMap<_, _> = entries
            .iter()
            .filter_map(|(key, value)| {
                if *key == role {
                    Some((*key, enabled))
                } else {
                    value.override_value.map(|enabled| (*key, enabled))
                }
            })
            .collect();
        let temporary = self.path.with_extension("json.pending");
        let write = (|| -> anyhow::Result<()> {
            std::fs::create_dir_all(self.path.parent().unwrap_or(Path::new(".")))?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .mode(0o600)
                .open(&temporary)?;
            file.write_all(&serde_json::to_vec(&saved)?)?;
            file.sync_all()?;
            std::fs::rename(&temporary, &self.path)?;
            Ok(())
        })();
        if write.is_err() {
            let _ = std::fs::remove_file(&temporary);
            return Err(Failure::new(
                503,
                "Ollama preload configuration could not be saved",
            ));
        }
        let entry = entries.get_mut(role).unwrap();
        entry.enabled = enabled;
        entry.override_value = Some(enabled);
        entry.generation = entry.generation.wrapping_add(1);
        entry.phase = if enabled { "pending" } else { "disabled" };
        entry.error = None;
        Ok(())
    }

    fn claim(&self, role: &str) -> Option<(u64, String)> {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.get_mut(role).unwrap();
        if entry.phase != "pending" {
            return None;
        }
        entry.phase = "preparing";
        Some((entry.generation, entry.model.clone()))
    }

    fn current(&self, role: &str, generation: u64) -> bool {
        let entries = self.entries.lock().unwrap();
        entries[role].enabled && entries[role].generation == generation
    }

    fn finish(&self, role: &str, generation: u64, result: anyhow::Result<()>) {
        let mut entries = self.entries.lock().unwrap();
        let entry = entries.get_mut(role).unwrap();
        if entry.generation == generation {
            entry.phase = if result.is_ok() {
                "completed"
            } else {
                "failed"
            };
            entry.error = result.err().map(|error| error.to_string());
        }
    }
}

fn canonical(model: &str) -> String {
    if model
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains(':'))
    {
        model.to_owned()
    } else {
        format!("{model}:latest")
    }
}

async fn prepare(g: &Gateway, role: &str, generation: u64, model: &str) -> anyhow::Result<()> {
    ensure!(
        !model.trim().is_empty() && model.len() <= 512,
        "invalid configured Ollama model"
    );
    ensure!(
        g.residency.seconds() > 0,
        "enable model idle retention before preloading"
    );
    let origin = format!("http://v-scheduler-ollama-{role}:11434");
    let tags = format!("{origin}/api/tags");
    let wait = async {
        loop {
            if !g.preload.current(role, generation) {
                return None;
            }
            if let Ok(response) = g
                .client
                .get(&tags)
                .timeout(Duration::from_secs(4))
                .send()
                .await
                && let Ok(response) = response.error_for_status()
                && let Ok(data) = response.json::<Value>().await
            {
                return Some(data);
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    };
    let Some(data) = tokio::time::timeout(Duration::from_secs(90), wait)
        .await
        .context("Ollama runtime did not become ready")?
    else {
        return Ok(());
    };
    let model = canonical(model);
    let downloaded = data["models"].as_array().is_some_and(|rows| {
        rows.iter().any(|row| {
            row["name"]
                .as_str()
                .is_some_and(|name| canonical(name) == model)
        })
    });
    if !g.preload.current(role, generation) {
        return Ok(());
    }
    if !downloaded {
        let response = g
            .client
            .post(format!("{origin}/api/pull"))
            .timeout(Duration::from_secs(1800))
            .json(&json!({"model":model,"stream":false}))
            .send()
            .await
            .context("Ollama model download request failed")?;
        ensure!(
            response.status().is_success(),
            "Ollama download failed (HTTP {})",
            response.status()
        );
        ensure!(
            response.json::<Value>().await?["status"] == "success",
            "Ollama did not confirm the download"
        );
    }
    if !g.preload.current(role, generation) {
        return Ok(());
    }
    discovery::ollama(g, role).await;
    if !g.preload.current(role, generation) {
        return Ok(());
    }
    let result = builtin_model_action(role, &model, true).await?;
    ensure!(
        result["confirmed"] == true,
        "Ollama did not confirm model residency"
    );
    Ok(())
}

pub(super) fn start(g: Arc<Gateway>) -> Vec<tokio::task::JoinHandle<()>> {
    ROLES
        .into_iter()
        .map(|role| {
            let g = g.clone();
            tokio::spawn(async move {
                loop {
                    if let Some((generation, model)) = g.preload.claim(role) {
                        let result = prepare(&g, role, generation, &model).await;
                        g.preload.finish(role, generation, result);
                    }
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn persisted_disable_wins_and_stale_completion_cannot_restore_it() {
        let dir = crate::router::tests::temp();
        let state = dir.join("state.json");
        let policy = Policy::open(&state).unwrap();
        policy.configure("qa", true).unwrap();
        let (generation, _) = policy.claim("qa").unwrap();
        policy.configure("qa", true).unwrap();
        assert!(policy.claim("qa").is_none());
        policy.configure("qa", false).unwrap();
        assert!(!policy.current("qa", generation));
        policy.finish("qa", generation, Ok(()));
        assert_eq!(policy.snapshot()["items"][0]["state"], "disabled");
        let restored = Policy::open(&state).unwrap();
        assert_eq!(restored.snapshot()["items"][0]["enabled"], false);
        let overrides = private_json(&dir.join("ollama-preload.json"), 4096).unwrap();
        assert_eq!(overrides, json!({"qa":false}));
        assert!(restored.configure("external", true).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[tokio::test]
    async fn preload_api_writes_one_role_and_rejects_inference_keys() {
        let dir = crate::router::tests::temp();
        let gateway = crate::router::tests::gateway(&dir, Router::new());
        let admin = Identity {
            admin: true,
            ..Default::default()
        };
        let request = |method, payload: Value| {
            Request::builder()
                .method(method)
                .uri("/api/v1/ollama-preload")
                .body(Body::from(payload.to_string()))
                .unwrap()
        };
        let valid = json!({"role":"embedding","enabled":true});
        let response = manage::handle(&gateway, &admin, request("PUT", valid.clone()))
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let snapshot: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(snapshot["items"][1]["state"], "pending");
        let invalid = json!({"role":"embedding","enabled":"yes"});
        assert_eq!(
            manage::handle(&gateway, &admin, request("PUT", invalid))
                .await
                .unwrap_err()
                .status,
            422
        );
        let key = Identity {
            api_key_id: Some("inference-key".into()),
            ..Default::default()
        };
        assert_eq!(
            manage::handle(&gateway, &key, request("PUT", valid))
                .await
                .unwrap_err()
                .status,
            403
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
