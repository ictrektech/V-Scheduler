use super::*;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
fn null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Upstream {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub role: String,
    pub base_url: String,
    pub model_prefix: String,
    pub api_key: String,
    pub protocol: String,
    pub visibility: String,
    pub owner: String,
    pub healthy: bool,
    pub last_error: String,
    pub source: String,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: Option<DateTime<Utc>>,
    pub scheduler: Option<Value>,
    pub async_task: Option<Value>,
}
impl Upstream {
    pub fn prefix(&self) -> &str {
        if !self.model_prefix.is_empty() {
            &self.model_prefix
        } else if [
            "model-hub",
            "image-studio",
            "v-voice",
            "v-music",
            "ml-server",
        ]
        .contains(&self.source.as_str())
        {
            &self.source
        } else if !self.kind.is_empty() {
            &self.kind
        } else {
            "manual"
        }
    }
    pub fn visible(&self, id: &Identity) -> bool {
        self.visibility != "private"
            || id.admin
            || (!self.owner.is_empty() && self.owner == id.owner)
    }
    pub fn manageable(&self, id: &Identity) -> bool {
        id.admin || self.owner.is_empty() || self.owner == id.owner
    }
    pub fn live(&self) -> bool {
        self.healthy && self.expires_at.is_none_or(|at| at > Utc::now())
    }
    pub fn chat_only(&self) -> bool {
        self.protocol == "chat_completions"
            || self.protocol.is_empty() && self.source == "model-hub"
    }
}
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Model {
    pub id: String,
    pub qualified: String,
    pub upstream_id: String,
    pub kind: String,
    #[serde(deserialize_with = "null_default")]
    pub capabilities: Vec<String>,
    #[serde(deserialize_with = "null_default")]
    pub endpoints: Vec<String>,
    pub owned_by: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Routing {
    pub weight: i64,
    pub max_inflight: usize,
    pub disabled: bool,
}
impl Default for Routing {
    fn default() -> Self {
        Self {
            weight: 1,
            max_inflight: 0,
            disabled: false,
        }
    }
}
#[derive(Default, Deserialize, Serialize)]
#[serde(default)]
pub(super) struct Disk {
    pub version: u32,
    #[serde(deserialize_with = "null_default")]
    pub upstreams: Vec<Upstream>,
    #[serde(deserialize_with = "null_default")]
    pub models: BTreeMap<String, Vec<Model>>,
    #[serde(deserialize_with = "null_default")]
    pub routing: BTreeMap<String, Routing>,
    #[serde(deserialize_with = "null_default")]
    pub resources: Vec<Value>,
}
#[derive(Default)]
pub(super) struct Registry {
    pub upstreams: BTreeMap<String, Upstream>,
    pub models: BTreeMap<String, Vec<Model>>,
    pub routing: BTreeMap<String, Routing>,
    pub inflight: BTreeMap<String, usize>,
    pub fair: BTreeMap<String, BTreeMap<String, i64>>,
    pub breaker: BTreeMap<String, (u32, DateTime<Utc>)>,
    pub resources: Vec<Value>,
    pub services: BTreeMap<String, String>,
}
impl Registry {
    pub fn open(path: &std::path::Path) -> anyhow::Result<Self> {
        let disk: Disk = match std::fs::read(path) {
            Ok(raw) => serde_json::from_slice(&raw)?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Disk::default(),
            Err(e) => return Err(e.into()),
        };
        let mut result = Self {
            models: disk.models,
            routing: disk.routing,
            resources: disk.resources,
            ..Self::default()
        };
        for mut u in disk.upstreams {
            if u.expires_at
                .is_some_and(|at| at.timestamp() <= -62135596800)
            {
                u.expires_at = None;
            }
            result.upstreams.insert(u.id.clone(), u);
        }
        for p in result.routing.values_mut() {
            anyhow::ensure!(
                (0..=1000).contains(&p.weight) && p.max_inflight <= 100000,
                "invalid persisted routing policy"
            );
            p.weight = p.weight.max(1);
        }
        Ok(result)
    }
    pub fn persist(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let upstreams: Vec<_> = self
            .upstreams
            .values()
            .filter(|u| u.source == "api")
            .cloned()
            .collect();
        let disk = Disk {
            version: 2,
            models: self
                .models
                .iter()
                .filter(|(id, _)| upstreams.iter().any(|u| &u.id == *id))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect(),
            upstreams,
            routing: self.routing.clone(),
            resources: self.resources.clone(),
        };
        let parent = path.parent().unwrap_or(std::path::Path::new("."));
        std::fs::create_dir_all(parent)?;
        let temp = path.with_extension("json.tmp");
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        file.write_all(&serde_json::to_vec_pretty(&disk)?)?;
        file.sync_all()?;
        std::fs::rename(temp, path)?;
        Ok(())
    }
    pub fn put(&mut self, mut u: Upstream, mut models: Vec<Model>) {
        if u.source != "registered" {
            self.breaker.remove(&u.id);
        }
        u.created_at = self
            .upstreams
            .get(&u.id)
            .and_then(|old| old.created_at)
            .or(Some(Utc::now()));
        for m in &mut models {
            normalize_model(m);
            m.upstream_id = u.id.clone();
            m.qualified = format!("{}/{}", u.prefix(), m.id);
        }
        self.models.insert(u.id.clone(), models);
        self.upstreams.insert(u.id.clone(), u);
    }
    pub fn result(&mut self, id: &str, success: bool) {
        if success {
            self.breaker.remove(id);
            return;
        }
        let b = self.breaker.entry(id.into()).or_insert((0, Utc::now()));
        b.0 += 1;
        if b.0 >= 3 {
            b.1 = Utc::now() + chrono::Duration::seconds((30i64 << (b.0 - 3).min(4)).min(300));
        }
    }
    pub fn status(&self, u: &Upstream) -> Value {
        let p = self.routing.get(&u.id).cloned().unwrap_or_default();
        let state = if p.disabled {
            "paused"
        } else if u.expires_at.is_some_and(|at| at <= Utc::now()) {
            "lease_expired"
        } else if let Some((n, until)) = self
            .breaker
            .get(&u.id)
            .filter(|(_, until)| *until > Utc::now())
        {
            return json!({"state":"circuit_open","failures":n,"circuit_open_until":until});
        } else if !u.healthy {
            if u.last_error.contains("401") || u.last_error.contains("403") {
                "auth_failed"
            } else {
                "discovery_failed"
            }
        } else if p.max_inflight > 0
            && self.inflight.get(&u.id).copied().unwrap_or(0) >= p.max_inflight
        {
            "full"
        } else if self.models.get(&u.id).is_none_or(Vec::is_empty) {
            "no_models"
        } else {
            "healthy"
        };
        json!({"state":state,"detail":u.last_error})
    }
    pub fn list_models(&self, identity: &Identity) -> Vec<Model> {
        let mut models: BTreeMap<String, Model> = BTreeMap::new();
        for u in self
            .upstreams
            .values()
            .filter(|u| u.live() && u.visible(identity))
        {
            for m in self.models.get(&u.id).into_iter().flatten() {
                models
                    .entry(m.qualified.clone())
                    .or_insert_with(|| m.clone());
            }
        }
        models.into_values().collect()
    }
    pub fn acquire(
        &mut self,
        requested: &str,
        path: &str,
        pinned: &str,
        id: &Identity,
    ) -> Result<(Upstream, String), Failure> {
        let mut choices: Vec<(Upstream, String)> = vec![];
        let (mut found, mut live, mut compatible) = (false, false, false);
        for u in self.upstreams.values().filter(|u| u.visible(id)) {
            if !pinned.is_empty() && u.id != pinned {
                continue;
            }
            let model = if !pinned.is_empty() {
                Some(Model {
                    id: requested.into(),
                    ..Model::default()
                })
            } else {
                self.models
                    .get(&u.id)
                    .and_then(|ms| ms.iter().find(|m| m.qualified == requested))
                    .cloned()
            };
            let Some(model) = model else {
                continue;
            };
            found = true;
            let policy = self.routing.get(&u.id).cloned().unwrap_or_default();
            if !u.live()
                || policy.disabled
                || pinned.is_empty()
                    && self
                        .breaker
                        .get(&u.id)
                        .is_some_and(|(_, until)| *until > Utc::now())
            {
                continue;
            }
            live = true;
            if pinned.is_empty() && !supports(u, &model, path) {
                continue;
            }
            compatible = true;
            if policy.max_inflight > 0
                && self.inflight.get(&u.id).copied().unwrap_or(0) >= policy.max_inflight
            {
                continue;
            }
            choices.push((u.clone(), model.id));
        }
        if choices.is_empty() {
            return Err(Failure::new(
                if !found {
                    404
                } else if !live {
                    503
                } else if !compatible {
                    400
                } else {
                    429
                },
                if !found {
                    "unknown model or upstream"
                } else if !live {
                    "upstream is not ready"
                } else if !compatible {
                    "no matching upstream supports this endpoint"
                } else {
                    "all matching upstreams are at their concurrency limit"
                },
            ));
        }
        choices.sort_by(|(a, _), (b, _)| {
            let load_a = self.inflight.get(&a.id).copied().unwrap_or(0) as i64;
            let load_b = self.inflight.get(&b.id).copied().unwrap_or(0) as i64;
            let weight_a = self.routing.get(&a.id).map_or(1, |p| p.weight.max(1));
            let weight_b = self.routing.get(&b.id).map_or(1, |p| p.weight.max(1));
            (load_a * weight_b)
                .cmp(&(load_b * weight_a))
                .then(a.id.cmp(&b.id))
        });
        let first = &choices[0].0;
        let first_load = self.inflight.get(&first.id).copied().unwrap_or(0) as i64;
        let first_weight = self.routing.get(&first.id).map_or(1, |p| p.weight.max(1));
        choices.retain(|(u, _)| {
            self.inflight.get(&u.id).copied().unwrap_or(0) as i64 * first_weight
                == first_load * self.routing.get(&u.id).map_or(1, |p| p.weight.max(1))
        });
        let pool = format!("{requested}\0{path}");
        let fair = self.fair.entry(pool).or_default();
        let mut total = 0;
        for (u, _) in &choices {
            let weight = self.routing.get(&u.id).map_or(1, |p| p.weight.max(1));
            total += weight;
            *fair.entry(u.id.clone()).or_default() += weight;
        }
        let index = choices
            .iter()
            .enumerate()
            .max_by_key(|(_, (u, _))| fair.get(&u.id).copied().unwrap_or(0))
            .map(|(i, _)| i)
            .unwrap_or(0);
        let chosen = choices.swap_remove(index);
        *fair.entry(chosen.0.id.clone()).or_default() -= total;
        *self.inflight.entry(chosen.0.id.clone()).or_default() += 1;
        Ok(chosen)
    }
}
pub(super) fn supports(u: &Upstream, m: &Model, path: &str) -> bool {
    if ["v-scheduler", "model-hub"].contains(&u.source.as_str()) {
        if u.role == "rerank" && path != "/rerank" {
            return false;
        }
        if path == "/rerank" && ["generate", "embedding", "embed"].contains(&u.role.as_str()) {
            return false;
        }
        if ["embedding", "embed"].contains(&u.role.as_str())
            && [
                "/chat/completions",
                "/responses",
                "/messages",
                "/completions",
            ]
            .contains(&path)
        {
            return false;
        }
        if u.role == "generate" && path == "/embeddings" {
            return false;
        }
    }
    if path == "/messages"
        && !(u.chat_only() || ["anthropic", "offline_asr"].contains(&u.protocol.as_str()))
    {
        return false;
    }
    if path != "/messages" && u.protocol == "anthropic" {
        return false;
    }
    if !m.endpoints.is_empty() {
        let wanted = endpoint(path);
        return m.endpoints.iter().any(|v| {
            endpoint(v) == wanted
                || endpoint(v) == "chat_completions"
                    && u.chat_only()
                    && ["/responses", "/messages"].contains(&path)
        });
    }
    let needed: &[&str] = match path {
        "/chat/completions" | "/responses" | "/messages" | "/completions" => &[
            "chat",
            "completion",
            "completions",
            "generate",
            "generation",
            "tools",
            "thinking",
        ],
        "/embeddings" => &["embedding", "embeddings"],
        "/rerank" => &["rerank", "reranking"],
        "/images/generations" => &["image", "images", "image_generation"],
        "/images/edits" => &["image_edit"],
        "/images/variations" => &["image_variation"],
        "/videos/generations" => &["video", "videos", "video_generation"],
        "/audio/speech" => &["tts", "speech"],
        "/audio/transcriptions" => &["asr", "transcription", "transcriptions"],
        "/audio/translations" => &["translation", "translations"],
        "/audio/music/generations" => &["music", "text-to-music", "instrumental"],
        "/moderations" => &["moderation", "moderations"],
        "/ml/predictions" => &[
            "vision",
            "image_embedding",
            "face_recognition",
            "person_reid",
            "ocr",
        ],
        "/ml/video/structure" | "/ml/video/structure/tasks" => &["video_structure"],
        _ => &[],
    };
    if !m.capabilities.is_empty() && !needed.is_empty() {
        return m
            .capabilities
            .iter()
            .any(|c| needed.contains(&c.to_ascii_lowercase().as_str()));
    }
    match path {
        "/chat/completions" | "/responses" | "/messages" | "/completions" => m.kind != "embedding",
        "/embeddings" => m.kind != "chat",
        _ => !u.chat_only(),
    }
}
pub(super) fn endpoint(v: &str) -> String {
    let value = v.trim().to_ascii_lowercase().replace(['-', ' '], "_");
    let value = value.trim_start_matches('/').trim_start_matches("v1/");
    match value {
        "chat" | "chat_completions" | "chat/completions" => "chat_completions",
        "response" | "responses" => "responses",
        "message" | "messages" | "anthropic_messages" | "anthropic/messages" => "messages",
        "completion" | "completions" => "completions",
        "embedding" | "embeddings" => "embeddings",
        "image" | "images" | "images/generations" | "image_generation" => "images/generations",
        "images/edits" | "image_edit" => "images/edits",
        "images/variations" | "image_variation" => "images/variations",
        "video" | "videos" | "videos/generations" | "video_generation" => "videos/generations",
        "audio/speech" | "speech" | "tts" => "audio/speech",
        "audio/transcriptions" | "transcriptions" | "asr" => "audio/transcriptions",
        "audio/translations" | "translations" => "audio/translations",
        "audio/music/generations" | "music" | "music_generation" | "text_to_music" => {
            "audio/music/generations"
        }
        "moderation" | "moderations" => "moderations",
        "ml/predictions" | "ml_prediction" | "ml_predictions" => "ml/predictions",
        "ml/video/structure" | "ml_video_structure" => "ml/video/structure",
        "ml/video/structure/tasks" | "ml_video_structure_tasks" => "ml/video/structure/tasks",
        "rerank" | "reranking" => "rerank",
        _ => value,
    }
    .into()
}
pub(super) fn normalize_model(model: &mut Model) {
    model.endpoints = model
        .endpoints
        .iter()
        .map(|e| endpoint(e))
        .filter(|s| !s.is_empty())
        .collect();
    model.endpoints.sort();
    model.endpoints.dedup();
    for ep in &model.endpoints {
        let cap = match ep.as_str() {
            "chat_completions" | "responses" | "messages" | "completions" => "completion",
            "embeddings" => "embedding",
            "images/generations" => "image_generation",
            "images/edits" => "image_edit",
            "images/variations" => "image_variation",
            "videos/generations" => "video_generation",
            "audio/speech" => "tts",
            "audio/transcriptions" => "asr",
            "audio/translations" => "translation",
            "audio/music/generations" => "text-to-music",
            "moderations" => "moderation",
            "rerank" => "rerank",
            "ml/predictions" => "vision",
            "ml/video/structure" | "ml/video/structure/tasks" => "video_structure",
            _ => "",
        };
        if !cap.is_empty() {
            model.capabilities.push(cap.into());
        }
    }
    model.capabilities = model
        .capabilities
        .iter()
        .map(|c| c.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    model.capabilities.sort();
    model.capabilities.dedup();
    let chat = model.capabilities.iter().any(|c| {
        [
            "chat",
            "completion",
            "completions",
            "generate",
            "generation",
            "tools",
            "thinking",
        ]
        .contains(&c.as_str())
    });
    let embed = model
        .capabilities
        .iter()
        .any(|c| ["embedding", "embeddings"].contains(&c.as_str()));
    if chat && !embed {
        model.kind = "chat".into();
    } else if embed && !chat {
        model.kind = "embedding".into();
    } else if model.kind.is_empty() {
        model.kind = "unknown".into();
    }
}
