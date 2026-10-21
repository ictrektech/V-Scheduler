use super::*;
use sha2::{Digest, Sha256};
async fn get(g: &Gateway, url: String, token: Option<&str>) -> Option<Value> {
    let request = g.client.get(url).timeout(Duration::from_secs(5));
    let response = if let Some(token) = token {
        request.bearer_auth(token)
    } else {
        request
    }
    .send()
    .await
    .ok()?;
    if !response.status().is_success() {
        return None;
    }
    read_json(response, 4 << 20).await.ok()
}
fn models(rows: &Value) -> Vec<Model> {
    rows.as_array()
        .into_iter()
        .flatten()
        .filter_map(|v| {
            let id = text(v, "id");
            let endpoints = strings(&v["endpoints"]);
            if id.is_empty() || endpoints.is_empty() {
                return None;
            }
            Some(Model {
                id: id.into(),
                kind: match text(v, "kind") {
                    "" => "unknown",
                    v => v,
                }
                .into(),
                endpoints,
                capabilities: strings(&v["capabilities"]),
                owned_by: text(v, "owned_by").into(),
                ..Default::default()
            })
        })
        .collect()
}
fn remove(g: &Gateway, id: &str) {
    let mut r = g.registry.lock().unwrap();
    r.upstreams.remove(id);
    r.models.remove(id);
}
fn publish(g: &Gateway, name: &str, value: Value) {
    let path = g.state_path.parent().unwrap().join(name);
    let temp = path.with_extension("pending");
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let result = (|| -> anyhow::Result<()> {
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&temp)?;
        f.write_all(&serde_json::to_vec(&value)?)?;
        std::fs::rename(temp, path)?;
        Ok(())
    })();
    if result.is_err() {
        eprintln!("gateway discovery snapshot write failed");
    }
}
fn duplicate(g: &Gateway, u: &Upstream) -> bool {
    g.registry.lock().unwrap().upstreams.values().any(|old| {
        old.id != u.id
            && ["api", "registered"].contains(&old.source.as_str())
            && old.base_url.trim_end_matches('/') == u.base_url.trim_end_matches('/')
    })
}
fn modelinfer_id(deployment: &str) -> String {
    format!("modelinfer-{deployment}")
}
/// Reconcile the ModelInfer catalog. Entries are owned by the discovered app:
/// no lease, no executor, no reservation — running deployments already hold
/// their resources and must not be budgeted twice.
pub(crate) fn sync_modelinfer(services: Vec<ModelinferService>) {
    let Some(g) = MANAGED_GATEWAY.get().and_then(std::sync::Weak::upgrade) else {
        return;
    };
    let mut r = g.registry.lock().unwrap();
    let keep: std::collections::BTreeSet<String> = services
        .iter()
        .map(|s| modelinfer_id(&s.deployment))
        .collect();
    let stale: Vec<String> = r
        .upstreams
        .iter()
        .filter(|(id, u)| u.source == "modelinfer" && !keep.contains(*id))
        .map(|(id, _)| id.clone())
        .collect();
    for id in stale {
        r.upstreams.remove(&id);
        r.models.remove(&id);
    }
    for service in services {
        let id = modelinfer_id(&service.deployment);
        let upstream = Upstream {
            id: id.clone(),
            name: service.name,
            kind: "local".into(),
            role: "generate".into(),
            base_url: service.base_url.clone(),
            model_prefix: "modelinfer".into(),
            api_key: service.api_key,
            protocol: "chat_completions".into(),
            visibility: "public".into(),
            owner: "com.ictrek.modelinfer".into(),
            healthy: true,
            source: "modelinfer".into(),
            ..Default::default()
        };
        // An explicit registration or administrator upstream for the same
        // service keeps precedence; discovery only fills the gap.
        let duplicate = r.upstreams.values().any(|old| {
            old.id != id
                && ["api", "registered"].contains(&old.source.as_str())
                && old.base_url.trim_end_matches('/')
                    == upstream.base_url.trim_end_matches('/')
        });
        if duplicate {
            r.upstreams.remove(&id);
            r.models.remove(&id);
            continue;
        }
        let model = Model {
            id: service.model,
            kind: "chat".into(),
            capabilities: vec!["chat".into(), "completion".into()],
            endpoints: vec![
                "chat_completions".into(),
                "responses".into(),
                "messages".into(),
            ],
            owned_by: "modelinfer".into(),
            ..Default::default()
        };
        r.put(upstream, vec![model]);
    }
}
pub(super) fn start(g: Arc<Gateway>) -> Vec<tokio::task::JoinHandle<()>> {
    let mut tasks = vec![];
    let raw = env("V_ROUTER_DISCOVERY_INTERVAL", "30s");
    let seconds = raw
        .trim_end_matches('s')
        .parse::<u64>()
        .unwrap_or(30)
        .max(1);
    for role in ["qa", "embedding", "rerank"] {
        let g = g.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                ollama(&g, role).await;
                g.initial_discovery.lock().unwrap().remove(role);
                tokio::time::sleep(Duration::from_secs(seconds)).await;
            }
        }));
    }
    for (key, default, id, source) in [
        (
            "V_ROUTER_V_VOICE_URLS",
            "http://v-voice-backend:7070,http://host.docker.internal:22700",
            "app-v-voice",
            "v-voice",
        ),
        (
            "V_ROUTER_V_MUSIC_URLS",
            "http://v-music-backend:7080,http://host.docker.internal:22701",
            "app-v-music",
            "v-music",
        ),
    ] {
        let g = g.clone();
        tasks.push(tokio::spawn(async move {
            loop {
                application(&g, key, default, id, source).await;
                tokio::time::sleep(Duration::from_secs(seconds)).await;
            }
        }));
    }
    let image = g.clone();
    tasks.push(tokio::spawn(async move {
        loop {
            image_studio(&image).await;
            tokio::time::sleep(Duration::from_secs(seconds)).await;
        }
    }));
    let ml = g.clone();
    tasks.push(tokio::spawn(async move {
        loop {
            ml_server(&ml).await;
            tokio::time::sleep(Duration::from_secs(seconds)).await;
        }
    }));
    tasks.push(tokio::spawn(async move {
        loop {
            video_structure(&g).await;
            tokio::time::sleep(Duration::from_secs(seconds)).await;
        }
    }));
    tasks
}
async fn ollama(g: &Gateway, role: &str) {
    let base = env(
        &format!("V_SCHEDULER_OLLAMA_{}_URL", role.to_ascii_uppercase()),
        &format!("http://v-scheduler-ollama-{role}:11535"),
    )
    .trim_end_matches('/')
    .to_string();
    if base.is_empty() {
        return;
    }
    let id = format!("v-scheduler/ollama-{role}");
    let mut u = Upstream {
        id: id.clone(),
        name: id.clone(),
        source: "v-scheduler".into(),
        kind: "local".into(),
        role: if role == "qa" { "generate" } else { role }.into(),
        base_url: format!("{base}/v1"),
        model_prefix: id,
        protocol: if role == "qa" {
            "chat_completions"
        } else {
            "openai"
        }
        .into(),
        ..Default::default()
    };
    let Some(tags) = get(g, format!("{base}/api/tags"), None).await else {
        u.last_error = "built-in Ollama unavailable".into();
        g.registry.lock().unwrap().put(u, vec![]);
        return;
    };
    let listed = manage::models(g, &u).await;
    let Ok(mut rows) = listed else {
        u.last_error = "built-in Ollama models unavailable".into();
        g.registry.lock().unwrap().put(u, vec![]);
        return;
    };
    for m in &mut rows {
        if let Some(tag) = tags["models"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|t| text(t, "name") == m.id || text(t, "model") == m.id)
        {
            m.capabilities = strings(&tag["capabilities"]);
        }
        if m.capabilities.is_empty() {
            if let Ok(response) = g
                .client
                .post(format!("{base}/api/show"))
                .json(&json!({"model":m.id}))
                .timeout(Duration::from_secs(5))
                .send()
                .await
            {
                if response.status().is_success() {
                    if let Ok(v) = read_json(response, 1 << 20).await {
                        m.capabilities = strings(&v["capabilities"]);
                    }
                }
            }
        }
        m.kind = if m
            .capabilities
            .iter()
            .any(|c| ["embedding", "embeddings"].contains(&c.as_str()))
        {
            "embedding"
        } else {
            "chat"
        }
        .into();
    }
    rows.retain(|m| {
        !m.capabilities.is_empty()
            && registry::supports(
                &u,
                m,
                match role {
                    "embedding" => "/embeddings",
                    "rerank" => "/rerank",
                    _ => "/chat/completions",
                },
            )
            && (role == "rerank"
                || !m
                    .capabilities
                    .iter()
                    .any(|c| ["rerank", "reranking"].contains(&c.as_str())))
    });
    u.healthy = true;
    g.registry.lock().unwrap().put(u, rows);
}
async fn application(g: &Gateway, key: &str, default: &str, id: &str, source: &str) {
    for candidate in env(key, default)
        .split(',')
        .map(|v| v.trim().trim_end_matches('/'))
        .filter(|v| !v.is_empty())
    {
        let Some(doc) = get(g, format!("{candidate}/api/v1/discovery"), None).await else {
            continue;
        };
        let u = Upstream {
            id: id.into(),
            name: match text(&doc, "name") {
                "" => source,
                v => v,
            }
            .into(),
            kind: "local".into(),
            source: source.into(),
            base_url: format!("{candidate}/v1"),
            model_prefix: source.into(),
            protocol: if doc["protocol"] == "chat_completions" {
                "chat_completions"
            } else {
                "openai"
            }
            .into(),
            visibility: if doc["visibility"] == "private" {
                "private"
            } else {
                "public"
            }
            .into(),
            healthy: true,
            ..Default::default()
        };
        if duplicate(g, &u) {
            remove(g, id);
            return;
        }
        if source == "v-voice" {
            publish(
                g,
                "v-voice-discovery.json",
                json!({"source":candidate,"observed_at":Utc::now(),"scheduler":doc["scheduler"],"models":doc["models"]}),
            );
        }
        g.registry.lock().unwrap().put(u, models(&doc["models"]));
        return;
    }
    remove(g, id);
    if source == "v-voice" {
        publish(
            g,
            "v-voice-discovery.json",
            json!({"source":"","observed_at":Utc::now(),"scheduler":null,"models":[]}),
        );
    }
}
pub(super) async fn vsub(g: &Gateway, token: &str, id: &Identity) {
    if id.owner.is_empty() {
        return;
    }
    let upstream = format!("app-v-sub-{:x}", Sha256::digest(id.owner.as_bytes()));
    for candidate in env(
        "V_ROUTER_V_SUB_URLS",
        "http://v-sub-backend:7090,http://host.docker.internal:22703",
    )
    .split(',')
    .map(|v| v.trim().trim_end_matches('/'))
    .filter(|v| !v.is_empty())
    {
        let Some(doc) = get(g, format!("{candidate}/api/v1/discovery"), Some(token)).await else {
            continue;
        };
        let namespace = format!("{:x}", Sha256::digest(format!("v-sub\0{}", id.owner)));
        let expected = format!("/inference/{namespace}/v1");
        if text(&doc, "owner") != id.owner
            || text(&doc, "base_path") != expected
            || text(&doc, "api_key").is_empty()
        {
            continue;
        }
        let u = Upstream {
            id: upstream.clone(),
            name: "V-Sub local models".into(),
            source: "v-sub".into(),
            kind: "local".into(),
            owner: id.owner.clone(),
            visibility: "private".into(),
            protocol: "openai".into(),
            model_prefix: "v-sub".into(),
            base_url: format!("{candidate}{expected}"),
            api_key: text(&doc, "api_key").into(),
            healthy: true,
            expires_at: Some(Utc::now() + chrono::Duration::seconds(90)),
            ..Default::default()
        };
        let mut r = g.registry.lock().unwrap();
        let duplicates: Vec<_> = r
            .upstreams
            .values()
            .filter(|old| {
                old.source == "registered" && old.owner == u.owner && old.base_url == u.base_url
            })
            .map(|u| u.id.clone())
            .collect();
        for key in duplicates {
            r.upstreams.remove(&key);
            r.models.remove(&key);
        }
        r.put(u, models(&doc["models"]));
        return;
    }
    remove(g, &upstream);
}
async fn image_studio(g: &Gateway) {
    let base = env(
        "V_ROUTER_IMAGE_STUDIO_URL",
        "http://image-studio-backend:5000",
    )
    .trim_end_matches('/')
    .to_string();
    if base.is_empty() {
        return;
    }
    let api = if base.ends_with("/api/v1") {
        base.clone()
    } else {
        format!("{base}/api/v1")
    };
    let Some(doc) = get(g, format!("{api}/models"), None).await else {
        remove(g, "app-image-studio");
        return;
    };
    let rows = image_models(&doc);
    let u = Upstream {
        id: "app-image-studio".into(),
        name: "Image Studio".into(),
        source: "image-studio".into(),
        kind: "local".into(),
        base_url: base,
        protocol: "image_studio".into(),
        model_prefix: "image-studio".into(),
        healthy: true,
        ..Default::default()
    };
    if duplicate(g, &u) {
        remove(g, &u.id);
        return;
    }
    g.registry.lock().unwrap().put(u, rows);
}
async fn ml_server(g: &Gateway) {
    for base in env(
        "V_ROUTER_ML_SERVER_URLS",
        "http://ml-server:3003,http://host.docker.internal:3003",
    )
    .split(',')
    .map(|v| v.trim().trim_end_matches('/'))
    .filter(|v| !v.is_empty())
    {
        let Some(info) = get(g, format!("{base}/"), None).await else {
            continue;
        };
        if info["service"] != "immich-ml-server" || info["status"] != "running" {
            continue;
        }
        let Some(spec) = get(g, format!("{base}/openapi.json"), None).await else {
            continue;
        };
        let mut rows = vec![];
        if spec["paths"].get("/predict").is_some() {
            for (name, kind, caps, endpoints) in [
                (
                    "qwen3-embedding-0.6b",
                    "embedding",
                    vec!["embedding"],
                    vec!["embeddings"],
                ),
                (
                    "ViT-B-16-SigLIP2__webli",
                    "embedding",
                    vec!["embedding", "image_embedding", "vision"],
                    vec!["embeddings", "ml/predictions"],
                ),
                (
                    "buffalo_l",
                    "vision",
                    vec!["face_recognition", "vision"],
                    vec!["ml/predictions"],
                ),
                (
                    "osnet_ain_x1_0",
                    "vision",
                    vec!["person_reid", "vision"],
                    vec!["ml/predictions"],
                ),
                (
                    "PP-OCRv5_server",
                    "vision",
                    vec!["ocr", "vision"],
                    vec!["ml/predictions"],
                ),
            ] {
                rows.push(Model {
                    id: name.into(),
                    kind: kind.into(),
                    capabilities: caps.into_iter().map(str::to_owned).collect(),
                    endpoints: endpoints.into_iter().map(str::to_owned).collect(),
                    owned_by: "immich-ml-server".into(),
                    ..Default::default()
                });
            }
        }
        if spec["paths"].get("/api/v1/video/structure").is_some()
            && spec["paths"]
                .get("/api/v1/video/structure/tasks/{task_id}")
                .is_some()
        {
            rows.push(Model {
                id: "video-structure".into(),
                kind: "video".into(),
                capabilities: vec!["video_structure".into()],
                endpoints: vec![
                    "ml/video/structure".into(),
                    "ml/video/structure/tasks".into(),
                ],
                ..Default::default()
            });
        }
        let u = Upstream {
            id: "app-ml-server".into(),
            name: "ML Server".into(),
            source: "ml-server".into(),
            kind: "local".into(),
            base_url: base.into(),
            protocol: "ml_server".into(),
            model_prefix: "ml-server".into(),
            healthy: true,
            ..Default::default()
        };
        if duplicate(g, &u) {
            remove(g, &u.id);
            return;
        }
        g.registry.lock().unwrap().put(u, rows);
        return;
    }
    remove(g, "app-ml-server");
}
async fn video_structure(g: &Gateway) {
    for (key, default, id, service) in [
        (
            "V_ROUTER_VIDEO_STRUCTURE_DETECT_URLS",
            "http://video-structure-detect:8910,http://host.docker.internal:48910",
            "video-structure-detect",
            "scene-detection",
        ),
        (
            "V_ROUTER_VIDEO_STRUCTURE_URLS",
            "http://video-structure:8911,http://host.docker.internal:48911",
            "video-structure-structure",
            "structure",
        ),
    ] {
        let mut found = None;
        for base in env(key, default)
            .split(',')
            .map(|v| v.trim().trim_end_matches('/'))
            .filter(|v| !v.is_empty())
        {
            if let Some(doc) = get(g, format!("{base}/api/health"), None).await {
                if doc["code"] == 0
                    && doc["data"]["status"] == "ok"
                    && doc["data"]["service"] == service
                {
                    found = Some(base.to_string());
                    break;
                }
            }
        }
        let mut r = g.registry.lock().unwrap();
        if let Some(base) = found {
            r.services.insert(id.into(), base);
        } else {
            r.services.remove(id);
        }
    }
}

pub(super) fn image_models(doc: &Value) -> Vec<Model> {
    let mut rows = vec![];
    for row in doc.as_array().into_iter().flatten() {
        let mut variants = vec![(text(row, "key").to_string(), row.clone())];
        for v in row["variants"].as_array().into_iter().flatten() {
            let variant = ["id", "key", "variant_id"]
                .into_iter()
                .map(|k| text(v, k))
                .find(|v| !v.is_empty())
                .unwrap_or("");
            if !variant.is_empty() {
                variants.push((format!("{}@{variant}", text(row, "key")), v.clone()));
            }
        }
        for (fallback, item) in variants {
            let meta = &item["v_router"];
            let status = if text(meta, "status").is_empty() {
                if text(&item, "status").is_empty() {
                    text(row, "status")
                } else {
                    text(&item, "status")
                }
            } else {
                text(meta, "status")
            };
            if status != "ready" {
                continue;
            }
            let native = if text(meta, "model").is_empty() {
                fallback
            } else {
                text(meta, "model").into()
            };
            let mut endpoints = strings(&meta["endpoints"]);
            let mut capabilities = strings(&meta["capabilities"]);
            if endpoints.is_empty() {
                capabilities = strings(&item["capabilities"]);
                if capabilities.is_empty() {
                    capabilities = strings(&row["capabilities"]);
                }
                for c in &capabilities {
                    match c.as_str() {
                        "image" | "txt2img" | "text2image" | "text_to_image"
                        | "image_generation" => endpoints.push("images/generations".into()),
                        "video" | "txt2vid" | "txt2video" | "text2video" | "text_to_video"
                        | "img2video" | "image2video" | "image_to_video" | "video_generation" => {
                            endpoints.push("videos/generations".into())
                        }
                        "img2img" | "image2image" | "image_to_image" | "edit" | "image_edit" => {
                            endpoints.push("images/edits".into())
                        }
                        _ => {}
                    }
                }
            }
            if !endpoints.is_empty() {
                rows.push(Model {
                    id: native,
                    kind: match text(meta, "model_kind") {
                        "" => "unknown",
                        v => v,
                    }
                    .into(),
                    capabilities,
                    endpoints,
                    owned_by: "image-studio".into(),
                    ..Default::default()
                });
            }
        }
    }
    rows
}
