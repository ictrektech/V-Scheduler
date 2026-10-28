//! Models.dev is metadata, never executable provider code or credentials.
use super::*;
use std::{collections::BTreeMap, io::Read, sync::OnceLock};
const MAX_BYTES: usize = 16 * 1024 * 1024;
#[derive(Clone)]
pub(super) struct Entry {
    pub id: String,
    pub name: String,
    pub env: Vec<String>,
    pub models: BTreeMap<String, Spec>,
}
#[derive(Clone)]
pub(super) struct Spec {
    pub model: Model,
    pub name: String,
    pub apis: BTreeMap<String, Value>,
    pub limit: Value,
    pub cost: Value,
    pub status: String,
}
#[derive(Clone)]
pub(super) struct Catalog {
    pub providers: BTreeMap<String, Entry>,
    pub revision: String,
}
// Saved identities remain stable; aliases only resolve metadata.
pub(super) fn canonical(id: &str) -> &str {
    match id {
        "ant-ling" => "bailing",
        "fireworks" => "fireworks-ai",
        "together" => "togetherai",
        "vercel-ai-gateway" => "vercel",
        "qwen-token-plan" | "qwen-token-plan-individual" => "alibaba-token-plan",
        "qwen-token-plan-cn" => "alibaba-token-plan-cn",
        "kimi-coding" => "kimi-code-plan-global",
        "openai-codex" => "openai",
        "zai-coding-cn" => "zhipuai-coding-plan",
        _ => id,
    }
}
fn flag(v: &Value, key: &str) -> bool {
    v[key] == true || v[key]["supported"] == true
}
fn api_endpoint(api: &str) -> Option<&'static str> {
    Some(match api {
        "radius-messages" | "chat-completions" | "converse" | "generate-content"
        | "cohere-chat" | "interactions" => "chat_completions",
        "responses" => "responses",
        "messages" => "messages",
        "images" => "images/generations",
        "workers-ai-run" | "systemone" | "decisions" => "systemone",
        "embeddings" => "embeddings",
        "rerank" => "rerank",
        "moderations" => "moderations",
        "ocr" => "ml/predictions",
        _ => return None,
    })
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
}
impl Entry {
    pub fn base(&self) -> String {
        let mut bases: BTreeMap<String, usize> = BTreeMap::new();
        for spec in self.models.values() {
            for api in spec.apis.values() {
                if let Some(base) = api["base_url"].as_str() {
                    *bases.entry(base.into()).or_default() += 1;
                }
            }
        }
        bases
            .into_iter()
            .max_by_key(|(_, n)| *n)
            .map(|(v, _)| v)
            .unwrap_or_default()
    }
}
impl Catalog {
    pub fn parse(bytes: &[u8]) -> anyhow::Result<Self> {
        use sha2::{Digest, Sha256};
        anyhow::ensure!(bytes.len() <= MAX_BYTES, "catalog exceeds limit");
        let root: Value = serde_json::from_slice(bytes)?;
        let values = root
            .get("providers")
            .unwrap_or(&root)
            .as_object()
            .ok_or_else(|| anyhow::anyhow!("provider object required"))?;
        let mut providers = BTreeMap::new();
        for (id, p) in values {
            if !identifier(id) || !p.is_object() {
                continue;
            }
            let Some(source) = p["models"].as_object() else {
                continue;
            };
            let mut models = BTreeMap::new();
            for (key, m) in source {
                let name = m["id"].as_str().unwrap_or(key);
                if name.is_empty() || name.len() > 2048 || name.chars().any(char::is_control) {
                    continue;
                }
                let mut apis: BTreeMap<String, Value> = m["api"]
                    .as_object()
                    .map(|v| v.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                    .unwrap_or_default();
                // The v1 API puts the SDK and common URL at provider level.
                if apis.is_empty() {
                    if let Some(npm) = m["provider"]["npm"].as_str().or_else(|| p["npm"].as_str()) {
                        let base = m["provider"]["api"]
                            .as_str()
                            .or_else(|| p["api"].as_str())
                            .unwrap_or("");
                        let kind = match m["type"].as_str() {
                            Some("image") => "images",
                            Some("decision") => "systemone",
                            _ => match npm {
                                "@ai-sdk/anthropic" => "messages",
                                "@ai-sdk/google" | "@ai-sdk/google-vertex" => "generate-content",
                                "@ai-sdk/amazon-bedrock" => "converse",
                                "@ai-sdk/openai" | "@ai-sdk/azure" | "@ai-sdk/xai"
                                    if m["provider"]["shape"]
                                        .as_str()
                                        .is_none_or(|v| v == "responses") =>
                                {
                                    "responses"
                                }
                                _ => "chat-completions",
                            },
                        };
                        apis.insert(kind.into(), json!({"base_url":base,"sdk":{"aisdk":npm}}));
                    }
                }
                let kind = match m["type"].as_str().unwrap_or("chat") {
                    "image" => "image_generation",
                    "decision" => "decision",
                    "embedding" => "embedding",
                    "rerank" => "rerank",
                    "chat" => "chat",
                    _ => continue,
                };
                let mut capabilities = match kind {
                    "image_generation" => vec!["image_generation".into()],
                    "decision" => vec!["decision".into()],
                    "embedding" => vec!["embedding".into()],
                    "rerank" => vec!["rerank".into()],
                    _ => vec!["chat".into(), "completion".into()],
                };
                let caps = &m["capabilities"];
                if flag(caps, "tools") || m["tool_call"] == true {
                    capabilities.push("tools".into());
                }
                if flag(caps, "reasoning") || m["reasoning"] == true {
                    capabilities.push("thinking".into());
                }
                let modalities = m.get("modalities").unwrap_or(&Value::Null).clone();
                if modalities["input"].as_array().is_some_and(|a| {
                    a.iter().any(|x| {
                        x.as_str()
                            .is_some_and(|v| v == "image" || v.starts_with("image/"))
                    })
                }) {
                    capabilities.push("vision".into());
                }
                let mut endpoints: Vec<String> = apis
                    .keys()
                    .filter_map(|a| api_endpoint(a).map(str::to_owned))
                    .collect();
                endpoints.sort();
                endpoints.dedup();
                if endpoints.is_empty() {
                    continue;
                }
                let model = Model {
                    id: name.into(),
                    kind: kind.into(),
                    capabilities,
                    endpoints,
                    ..Default::default()
                };
                models.insert(
                    name.into(),
                    Spec {
                        model,
                        name: m["name"].as_str().unwrap_or(name).into(),
                        apis,
                        limit: m["limit"].clone(),
                        cost: m["cost"].clone(),
                        status: m["status"].as_str().unwrap_or("").into(),
                    },
                );
            }
            providers.insert(
                id.clone(),
                Entry {
                    id: id.clone(),
                    name: p["name"].as_str().unwrap_or(id).into(),
                    env: strings(&p["env"]),
                    models,
                },
            );
        }
        anyhow::ensure!(!providers.is_empty(), "empty or invalid provider catalog");
        Ok(Self {
            providers,
            revision: format!("{:x}", Sha256::digest(bytes)),
        })
    }
    pub fn bundled() -> Arc<Self> {
        static VALUE: OnceLock<Arc<Catalog>> = OnceLock::new();
        VALUE
            .get_or_init(|| {
                let mut decoder = flate2::read::GzDecoder::new(
                    &include_bytes!("../../../providers/models-dev/snapshot.json.gz")[..],
                );
                let mut bytes = Vec::new();
                decoder
                    .by_ref()
                    .take((MAX_BYTES + 1) as u64)
                    .read_to_end(&mut bytes)
                    .expect("bundled catalog decompression");
                Arc::new(Self::parse(&bytes).expect("valid bundled Models.dev catalog"))
            })
            .clone()
    }
    pub fn detail(&self, id: &str) -> Option<Value> {
        let p = self.providers.get(canonical(id))?;
        Some(
            json!({"source":"models.dev","revision":self.revision,"id":p.id,"name":p.name,"env":p.env,"base":p.base(),"models":p.models.values().map(|s|json!({"id":s.model.id,"name":s.name,"kind":s.model.kind,"capabilities":s.model.capabilities,"endpoints":s.model.endpoints,"limit":s.limit,"cost":s.cost,"status":s.status})).collect::<Vec<_>>()}),
        )
    }
    pub fn summary(&self) -> Value {
        let providers:Vec<Value>=self.providers.values().map(|p|{
            let base=p.base();
            let portable=if ["amazon-bedrock","google-vertex","github-copilot","azure","cloudflare-ai-gateway"].contains(&p.id.as_str()){None}else{["chat-completions","messages"].into_iter().find_map(|api|{let mut counts:BTreeMap<String,usize>=BTreeMap::new();for s in p.models.values(){if let Some(base)=s.apis.get(api).and_then(|v|v["base_url"].as_str()){*counts.entry(base.into()).or_default()+=1;}}counts.into_iter().max_by_key(|(_,n)|*n).map(|(base,_)|(api,base))})};
            let (protocol,portable_base)=portable.map(|(api,base)|(if api=="messages"{"anthropic"}else{"chat_completions"},base)).unwrap_or(("",String::new()));
            json!({"id":p.id,"name":p.name,"base":base,"env":p.env,"models_count":p.models.len(),"registrationProtocol":protocol,"registrationBase":portable_base})
        }).collect();
        json!({"source":"models.dev","revision":self.revision,"providers":providers})
    }
}
pub(super) struct Store {
    pub value: RwLock<Arc<Catalog>>,
}
impl Default for Store {
    fn default() -> Self {
        Self {
            value: RwLock::new(Catalog::bundled()),
        }
    }
}
impl Store {
    pub fn open(state: &Path) -> Self {
        let store = Self::default();
        let path = state
            .parent()
            .unwrap_or(Path::new("."))
            .join("models-dev-cache.json");
        if let Ok(v) = private_json(&path, MAX_BYTES as u64) {
            if let Ok(catalog) = Catalog::parse(&serde_json::to_vec(&v).unwrap_or_default()) {
                *store.value.write().unwrap() = Arc::new(catalog);
            }
        }
        store
    }
    pub fn snapshot(&self) -> Arc<Catalog> {
        self.value.read().unwrap().clone()
    }
}
pub(super) fn start(g: Arc<Gateway>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let url = env(
                "V_SCHEDULER_MODELS_DEV_URL",
                "https://models.dev/experimental/v2.0/api.json?type=chat,decision,image",
            );
            let result = async {
                let response = g
                    .client
                    .get(url)
                    .timeout(Duration::from_secs(30))
                    .send()
                    .await
                    .map_err(|_| ())?;
                if !response.status().is_success() {
                    return Err(());
                }
                let bytes = read_limited(response, MAX_BYTES).await.map_err(|_| ())?;
                let catalog = Catalog::parse(&bytes).map_err(|_| ())?;
                let parent = g.state_path.parent().unwrap_or(Path::new("."));
                use std::os::unix::fs::OpenOptionsExt;
                std::fs::create_dir_all(parent).map_err(|_| ())?;
                let tmp = parent.join(format!(".models-dev-{}.tmp", uuid::Uuid::new_v4()));
                let written = (|| -> std::io::Result<()> {
                    use std::io::Write;
                    let mut f = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&tmp)?;
                    f.write_all(&bytes)?;
                    f.sync_all()?;
                    std::fs::rename(&tmp, parent.join("models-dev-cache.json"))
                })();
                if written.is_err() {
                    let _ = std::fs::remove_file(&tmp);
                    return Err(());
                }
                *g.models_dev.value.write().unwrap() = Arc::new(catalog);
                Ok(())
            }
            .await;
            if result.is_err() {
                eprintln!("models.dev catalog refresh unavailable; previous metadata retained");
            }
            tokio::time::sleep(Duration::from_secs(6 * 3600)).await;
        }
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_catalog_has_broad_provider_coverage_without_cross_provider_deduplication() {
        let c = Catalog::bundled();
        assert!(c.providers.len() > 200);
        assert!(c.providers.contains_key("siliconflow"));
        assert!(c.providers.contains_key("deepinfra"));
        let a = &c.providers["deepinfra"].models;
        let b = &c.providers["openrouter"].models;
        assert!(a.keys().any(|id| b.contains_key(id)));
    }
    #[test]
    fn model_capabilities_do_not_invent_tools_or_turn_decisions_into_chat() {
        let c=Catalog::parse(br#"{"p":{"models":{"same":{"type":"decision","api":{"systemone":{"base_url":"https://p/v1"}},"capabilities":{"tools":{"supported":false}}}}},"q":{"models":{"same":{"type":"image","api":{"images":{"base_url":"https://q/v1"}}}}}}"#).unwrap();
        assert_eq!(
            c.providers["p"].models["same"].model.capabilities,
            vec!["decision"]
        );
        assert_eq!(
            c.providers["q"].models["same"].model.endpoints,
            vec!["images/generations"]
        );
    }
    #[test]
    fn invalid_or_oversized_catalog_is_rejected() {
        assert!(Catalog::parse(b"{}").is_err());
        assert!(Catalog::parse(b"[]").is_err());
        assert!(Catalog::parse(&vec![b' '; MAX_BYTES + 1]).is_err());
    }
    #[test]
    fn legacy_api_metadata_remains_readable() {
        let c=Catalog::parse(br#"{"p":{"api":"https://example.test/v1","npm":"@ai-sdk/anthropic","models":{"model":{"tool_call":true,"reasoning":false}}}}"#).unwrap();
        let m = &c.providers["p"].models["model"].model;
        assert!(m.capabilities.contains(&"tools".into()));
        assert!(!m.capabilities.contains(&"thinking".into()));
        assert_eq!(m.endpoints, vec!["messages"]);
    }
}

// Metadata complements existing transports. It never supplies account authorization.
fn provider_id(u: &Upstream) -> &str {
    if u.protocol == "pi" && u.provider == "zai" && u.base_url.contains("/coding/") {
        "zai-coding-plan"
    } else {
        canonical(&u.provider)
    }
}
pub(super) fn enrich_model(g: &Gateway, u: &Upstream, m: &mut Model) {
    if m.metadata_override || u.provider.is_empty() {
        return;
    }
    let catalog = g.models_dev.snapshot();
    let Some(spec) = catalog
        .providers
        .get(provider_id(u))
        .and_then(|p| p.models.get(&m.id))
    else {
        return;
    };
    if spec.model.kind != "chat" {
        return;
    }
    if u.protocol != "pi"
        && !spec.apis.contains_key(if u.protocol == "anthropic" {
            "messages"
        } else {
            "chat-completions"
        })
    {
        return;
    }

    m.kind = spec.model.kind.clone();
    m.capabilities = spec.model.capabilities.clone();
    m.endpoints = vec![
        if u.protocol == "anthropic" {
            "messages"
        } else {
            "chat_completions"
        }
        .into(),
    ];
}
pub(super) fn enrich_native(g: &Gateway, u: &Upstream, models: &mut [Model]) {
    for model in models {
        enrich_model(g, u, model);
    }
}
pub(super) fn fallback_models(g: &Gateway, u: &Upstream) -> Vec<Model> {
    if u.provider.is_empty()
        || !["openai", "chat_completions", "anthropic"].contains(&u.protocol.as_str())
    {
        return vec![];
    }
    let catalog = g.models_dev.snapshot();
    let Some(provider) = catalog.providers.get(provider_id(u)) else {
        return vec![];
    };
    provider
        .models
        .values()
        .filter(|s| s.model.kind == "chat")
        .filter(|s| {
            s.apis.contains_key(if u.protocol == "anthropic" {
                "messages"
            } else {
                "chat-completions"
            })
        })
        .map(|s| {
            let mut model = s.model.clone();
            model.endpoints = vec![
                if u.protocol == "anthropic" {
                    "messages"
                } else {
                    "chat_completions"
                }
                .into(),
            ];
            model
        })
        .collect()
}
