use super::*;
use axum::body::Bytes;
use futures_util::{Stream, StreamExt};
use std::{
    pin::Pin,
    task::{Context, Poll},
    time::Instant,
};
pub(super) struct Exchange {
    pub g: Arc<Gateway>,
    pub u: Upstream,
    pub id: Identity,
    pub model: String,
    pub native: String,
    pub path: String,
    start: Instant,
    pub status: u16,
    stream: bool,
    _slot: Option<tokio::sync::OwnedSemaphorePermit>,
    buffer: Option<tokio::sync::OwnedSemaphorePermit>,
    pub lease: Option<lease::Lease>,
    retained: bool,
    _model_guard: Option<tokio::sync::OwnedRwLockReadGuard<()>>,
    pub usage: Value,
    bridge_request: Value,
    pub async_job: Option<Value>,
}
impl Drop for Exchange {
    fn drop(&mut self) {
        let mut r = self.g.registry.lock().unwrap();
        if let Some(n) = r.inflight.get_mut(&self.u.id) {
            *n = n.saturating_sub(1);
        }
        drop(r);
        let input = self.usage["prompt_tokens"]
            .as_i64()
            .or(self.usage["input_tokens"].as_i64())
            .unwrap_or(0);
        let output = self.usage["completion_tokens"]
            .as_i64()
            .or(self.usage["output_tokens"].as_i64())
            .unwrap_or(0);
        self.g.usage.lock().unwrap().record(json!({"time":Utc::now(),"user":if self.id.admin{"gateway-admin"}else{&self.id.username},"model":self.model,"native_id":self.native,"upstream_id":self.u.id,"upstream_name":self.u.name,"endpoint":self.path.trim_start_matches('/'),"status":self.status,"latency_ms":self.start.elapsed().as_millis()as u64,"stream":self.stream,"usage_reported":self.usage.is_object(),"input_tokens":input,"output_tokens":output,"total_tokens":self.usage["total_tokens"].as_i64().unwrap_or(input+output),"cached_tokens":self.usage["prompt_tokens_details"]["cached_tokens"].as_i64().unwrap_or(0),"reasoning_tokens":self.usage["completion_tokens_details"]["reasoning_tokens"].as_i64().unwrap_or(0)}));
    }
}
fn clean_headers(headers: &HeaderMap) -> HeaderMap {
    let mut out = headers.clone();
    if let Some(connection) = headers.get("connection").and_then(|v| v.to_str().ok()) {
        for name in connection.split(',') {
            out.remove(name.trim());
        }
    }
    for name in [
        "host",
        "authorization",
        "x-api-key",
        "content-length",
        "connection",
        "keep-alive",
        "proxy-authenticate",
        "proxy-authorization",
        "te",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "x-vscheduler-permit-id",
        "x-vscheduler-execution-token",
        "x-vscheduler-executor-id",
        "x-vrouter-upstream",
        "x-vscheduler-queue-wait-seconds",
        "x-vscheduler-priority",
        "x-vscheduler-request-id",
        "cookie",
        // Browser origin belongs to this gateway; upstream calls are server-to-server.
        "origin",
        // Bridges parse JSON/SSE and must receive an uncompressed provider body.
        "accept-encoding",
    ] {
        out.remove(name);
    }
    out
}
pub(super) fn outgoing(
    g: &Gateway,
    u: &Upstream,
    method: axum::http::Method,
    url: String,
    headers: &HeaderMap,
    lease: Option<&lease::Lease>,
) -> reqwest::RequestBuilder {
    let mut headers = clean_headers(headers);
    if let Some(lease) = lease {
        headers.insert("x-vscheduler-permit-id", lease.id.parse().unwrap());
        headers.insert("x-vscheduler-execution-token", lease.proof.parse().unwrap());
        headers.insert("x-vscheduler-executor-id", lease.executor.parse().unwrap());
    }
    let has_version = headers.contains_key("anthropic-version");
    let mut builder = g.client.request(method, url).headers(headers);
    if !u.api_key.is_empty() {
        if u.protocol == "anthropic" {
            builder = builder.header("x-api-key", &u.api_key);
        } else {
            builder = builder.bearer_auth(&u.api_key);
        }
    }
    if u.protocol == "anthropic" && !has_version {
        builder = builder.header("anthropic-version", "2023-06-01");
    }
    builder
}
pub(super) async fn management(g: &Gateway, mut request: Request, path: &str) -> Api<Response> {
    let raw = env(
        "V_SCHEDULER_MODEL_MANAGEMENT_URL",
        "http://v-scheduler:6005",
    );
    let base = reqwest::Url::parse(&raw)
        .map_err(|_| Failure::new(503, "Model management endpoint configuration is invalid"))?;
    if !["http", "https"].contains(&base.scheme())
        || base.host_str().is_none()
        || !base.username().is_empty()
        || base.password().is_some()
        || base.query().is_some()
        || base.fragment().is_some()
        || base.path() != "/"
    {
        return Err(Failure::new(
            503,
            "Model management endpoint configuration is invalid",
        ));
    }
    let target = format!(
        "{}{}{}",
        raw.trim_end_matches('/'),
        path.trim_start_matches("/model-hub"),
        request
            .uri()
            .query()
            .map(|q| format!("?{q}"))
            .unwrap_or_default()
    );
    let headers = clean_headers(request.headers());
    let method = request.method().clone();
    let body = std::mem::replace(request.body_mut(), Body::empty());
    let response = g
        .client
        .request(method, target)
        .headers(headers)
        .body(reqwest::Body::wrap_stream(body.into_data_stream()))
        .send()
        .await
        .map_err(|_| Failure::new(503, "Model management is unavailable"))?;
    let status = response.status();
    let headers = clean_headers(response.headers());
    let mut out = Response::new(Body::from_stream(response.bytes_stream()));
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    Ok(out)
}
fn json_body(bytes: &[u8]) -> Api<Value> {
    serde_json::from_slice(bytes).map_err(|_| Failure::new(400, "invalid JSON request"))
}
pub(super) struct Part {
    pub name: String,
    pub header: Vec<u8>,
    pub start: usize,
    pub end: usize,
}
pub(super) fn multipart(bytes: &[u8], content_type: &str) -> Api<Vec<Part>> {
    let boundary = content_type
        .split(';')
        .find_map(|s| s.trim().strip_prefix("boundary="))
        .map(|s| s.trim_matches('"'))
        .filter(|s| !s.is_empty() && s.len() <= 200)
        .ok_or_else(|| Failure::new(400, "multipart boundary is required"))?;
    let delimiter = format!("\r\n--{boundary}").into_bytes();
    let mut offset = format!("--{boundary}\r\n").len();
    let mut parts = vec![];
    if !bytes.starts_with(format!("--{boundary}\r\n").as_bytes()) {
        return Err(Failure::new(400, "invalid multipart body"));
    }
    loop {
        let rest = bytes
            .get(offset..)
            .ok_or_else(|| Failure::new(400, "invalid multipart body"))?;
        let head_end = rest
            .windows(4)
            .position(|s| s == b"\r\n\r\n")
            .ok_or_else(|| Failure::new(400, "invalid multipart headers"))?;
        let header = rest[..head_end].to_vec();
        let text = std::str::from_utf8(&header)
            .map_err(|_| Failure::new(400, "invalid multipart headers"))?;
        let name = text
            .lines()
            .find(|s| s.to_ascii_lowercase().starts_with("content-disposition:"))
            .and_then(|s| {
                s.split(';')
                    .find_map(|part| part.trim().strip_prefix("name=\""))
            })
            .and_then(|s| s.split('"').next())
            .unwrap_or("")
            .to_string();
        let start = offset + head_end + 4;
        let end = bytes[start..]
            .windows(delimiter.len())
            .position(|s| s == delimiter)
            .map(|n| start + n)
            .ok_or_else(|| Failure::new(400, "unterminated multipart body"))?;
        parts.push(Part {
            name,
            header,
            start,
            end,
        });
        let after = end + delimiter.len();
        if bytes.get(after..after + 2) == Some(b"--") {
            break;
        }
        if bytes.get(after..after + 2) != Some(b"\r\n") {
            return Err(Failure::new(400, "invalid multipart separator"));
        }
        offset = after + 2;
    }
    Ok(parts)
}
fn rewrite_multipart(bytes: &[u8], parts: &[Part], u: &Upstream, native: &str) -> Api<Vec<u8>> {
    let mut out = bytes.to_vec();
    let mut model_count = 0;
    for part in parts.iter().rev() {
        let replacement = match part.name.as_str() {
            "model" => {
                model_count += 1;
                Some(native.to_owned())
            }
            "speaker_model" if u.source == "v-voice" => {
                let value = std::str::from_utf8(&bytes[part.start..part.end])
                    .map_err(|_| Failure::new(400, "invalid secondary model"))?;
                Some(
                    value
                        .strip_prefix(&format!("{}/", u.prefix()))
                        .unwrap_or(value)
                        .into(),
                )
            }
            _ => None,
        };
        if let Some(v) = replacement {
            out.splice(part.start..part.end, v.bytes());
        }
    }
    if model_count > 1 {
        return Err(Failure::new(400, "duplicate model field"));
    }
    Ok(out)
}
fn check_owner(g: &Gateway, u: &Upstream, path: &str, id: &Identity) -> Api<()> {
    let path = percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .map_err(|_| Failure::new(400, "invalid resource path"))?;
    if path.split('/').any(|s| s == "." || s == "..") {
        return Err(Failure::new(400, "invalid resource path"));
    }
    let mut segments = path.trim_start_matches("/v1/").split('/');
    let family = segments.next().unwrap_or("");
    let resource = segments.next().unwrap_or("");
    if !resource.is_empty() {
        let r = g.registry.lock().unwrap();
        if r.resources.iter().any(|v| {
            text(v, "upstream") == u.id
                && text(v, "family") == family
                && text(v, "resource_id") == resource
                && !text(v, "owner").is_empty()
                && text(v, "owner") != id.owner
        }) && !id.admin
        {
            return Err(Failure::new(404, "resource not found"));
        }
    }
    Ok(())
}
fn record_owner(exchange: &Exchange, value: &Value) {
    if exchange.id.owner.is_empty() || exchange.status >= 300 {
        return;
    }
    let family = exchange
        .path
        .trim_start_matches('/')
        .split('/')
        .next()
        .unwrap_or("");
    if ![
        "files",
        "uploads",
        "batches",
        "videos",
        "fine_tuning",
        "vector_stores",
        "containers",
        "assistants",
        "threads",
        "conversations",
        "responses",
        "skills",
    ]
    .contains(&family)
    {
        return;
    }
    let resource = text(value, "id");
    if resource.is_empty() {
        return;
    }
    let mut r = exchange.g.registry.lock().unwrap();
    r.resources.retain(|v| {
        !(v["upstream"] == exchange.u.id && v["family"] == family && v["resource_id"] == resource)
    });
    r.resources.push(json!({"upstream":exchange.u.id,"family":family,"resource_id":resource,"owner":exchange.id.owner,"created_at":Utc::now()}));
    if r.resources.len() > 20000 {
        r.resources.remove(0);
    }
    if r.persist(&exchange.g.state_path).is_err() {
        eprintln!("gateway resource ownership persistence failed");
    }
}
pub(super) async fn handle(g: Arc<Gateway>, id: Identity, request: Request) -> Api<Response> {
    let path = request.uri().path().to_string();
    let pinned = request
        .headers()
        .get("x-vrouter-upstream")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if path == "/api/v1/video-structure/status" {
        let r = g.registry.lock().unwrap();
        let detect = r.services.contains_key("video-structure-detect");
        let structure = r.services.contains_key("video-structure-structure");
        return Ok(axum::Json(json!({"available":detect&&structure,"services":{"scene_detect":detect,"structure":structure}})).into_response());
    }
    if path == "/v1/models" && pinned.is_empty() {
        if g.catalog_initializing() && g.registry.lock().unwrap().list_models(&id).is_empty() {
            return Err(super::readiness::initializing());
        }
        return Ok(axum::Json(json!({"object":"list","data":g.registry.lock().unwrap().list_models(&id).iter().map(|m|json!({"id":m.qualified,"object":"model","created":0,"owned_by":m.owned_by})).collect::<Vec<_>>()})).into_response());
    }
    let slot = g
        .slots
        .clone()
        .try_acquire_owned()
        .map_err(|_| Failure::new(429, "gateway request capacity exhausted"))?;
    let query = request.uri().query().unwrap_or("").to_string();
    let method = request.method().clone();
    let headers = request.headers().clone();
    let is_websocket = headers
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|s| s.eq_ignore_ascii_case("websocket"));
    let is_query = is_websocket
        || method == "GET"
            && [
                "/v1/audio/voices",
                "/v1/audio/speakers",
                "/v1/audio/streaming/ws",
            ]
            .contains(&path.as_str())
        || path == "/v1/audio/streaming/transcriptions"
        || method == "GET" && path.starts_with("/v1/ml/video/structure/tasks/");
    let business = path.starts_with("/api/v2/") || path.starts_with("/api/v1/scenes/");
    let (request_parts, incoming) = request.into_parts();
    let mut incoming = Some(incoming);
    let (buffer, bytes, parts, mut value, requested) = if is_query {
        (
            None,
            Bytes::new(),
            vec![],
            Value::Null,
            usage::url_query(&query, "model").unwrap_or_default(),
        )
    } else {
        let length = headers
            .get("content-length")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<usize>().ok())
            .unwrap_or(32 << 20);
        if length > 32 << 20 {
            return Err(Failure::new(413, "inference upload exceeds 32 MiB"));
        }
        let buffer = g
            .buffered
            .clone()
            .try_acquire_many_owned(length.div_ceil(1024).max(1) as u32)
            .map_err(|_| Failure::new(429, "gateway buffered input capacity exhausted"))?;
        let bytes = axum::body::to_bytes(incoming.take().unwrap(), 32 << 20)
            .await
            .map_err(|_| Failure::new(413, "inference upload exceeds 32 MiB"))?;
        let ct = headers
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if ct.starts_with("multipart/form-data") {
            let parts = multipart(&bytes, ct)?;
            let models = parts
                .iter()
                .filter(|p| p.name == "model")
                .collect::<Vec<_>>();
            if models.len() > 1 {
                return Err(Failure::new(400, "duplicate model field"));
            }
            let model = models
                .first()
                .map(|p| String::from_utf8_lossy(&bytes[p.start..p.end]).into_owned())
                .unwrap_or_default();
            (Some(buffer), bytes, parts, Value::Null, model)
        } else {
            let value = if bytes.is_empty() {
                json!({})
            } else if ct.contains("json") || !business && pinned.is_empty() {
                json_body(&bytes)?
            } else {
                Value::Null
            };
            let model = text(&value, "model").to_string();
            (Some(buffer), bytes, vec![], value, model)
        }
    };
    if !pinned.is_empty() {
        auth::identity(&g, &headers, true).await?;
    }
    let suffix = path.strip_prefix("/v1").unwrap_or(&path).to_string();
    let (mut u, native) = if business {
        let service = if path.contains("/scenes/") {
            "video-structure-detect"
        } else {
            "video-structure-structure"
        };
        let base = g
            .registry
            .lock()
            .unwrap()
            .services
            .get(service)
            .cloned()
            .ok_or_else(|| Failure::new(503, "video structure service is not available"))?;
        (
            Upstream {
                id: service.into(),
                name: service.into(),
                kind: "local".into(),
                base_url: base,
                protocol: "video_structure".into(),
                healthy: true,
                ..Default::default()
            },
            String::new(),
        )
    } else {
        if pinned.is_empty() && requested.is_empty() {
            return Err(Failure::new(400, "model is required"));
        }
        let family = suffix
            .trim_start_matches('/')
            .split('/')
            .next()
            .unwrap_or("");
        if !pinned.is_empty()
            && ![
                "messages",
                "skills",
                "responses",
                "chat",
                "completions",
                "embeddings",
                "files",
                "uploads",
                "batches",
                "images",
                "audio",
                "videos",
                "fine_tuning",
                "vector_stores",
                "assistants",
                "threads",
                "conversations",
                "realtime",
                "containers",
                "models",
                "moderations",
                "rerank",
                "ml",
            ]
            .contains(&family)
        {
            return Err(Failure::new(404, "unknown provider API family"));
        }
        let routing_path = if suffix == "/audio/voices" {
            "/audio/speech"
        } else if suffix.starts_with("/ml/video/structure/tasks/") {
            "/ml/video/structure/tasks"
        } else {
            &suffix
        };
        g.local_model_readiness(&requested)?;
        g.registry
            .lock()
            .unwrap()
            .acquire(&requested, routing_path, &pinned, &id)?
    };
    let mut exchange = Exchange {
        g: g.clone(),
        u: u.clone(),
        id: id.clone(),
        model: requested,
        native: native.clone(),
        path: suffix.clone(),
        start: Instant::now(),
        status: 499,
        stream: value["stream"] == true,
        _slot: Some(slot),
        buffer,
        lease: None,
        retained: false,
        _model_guard: None,
        usage: Value::Null,
        bridge_request: Value::Null,
        async_job: None,
    };
    lease::classify_cloud(&mut u).await?;
    exchange.u = u.clone();
    check_owner(&g, &u, &path, &id)?;
    let approval = lease::approval(&g, &u, &native, method.as_str(), &suffix, is_websocket)?;
    let asynchronous = jobs::creation(&u, &suffix, &value, method.as_str());
    let conversion = if u.chat_only() && ["/responses", "/messages"].contains(&suffix.as_str()) {
        suffix.trim_start_matches('/')
    } else {
        ""
    };
    if !conversion.is_empty() {
        value = bridge::request(&value, conversion)?;
    }
    if let Some(approval) = &approval {
        if u.source == "v-scheduler" && !value.is_object() {
            return Err(Failure::new(422, "built-in Ollama inference requires JSON"));
        }
        if approval.policy["completion"] != "synchronous" && !asynchronous {
            return Err(Failure::new(
                503,
                "local permit requires an approved synchronous completion contract",
            ));
        }
        if u.source == "v-voice" {
            if let Some(model) = value["speaker_model"].as_str() {
                let native = model.strip_prefix("v-voice/").unwrap_or(model).to_string();
                value["speaker_model"] = json!(native);
            }
        }
        if !value.is_null() {
            lease::validate(approval, &u, &mut value, bytes.len())?;
        } else {
            let mut fields = json!({});
            for part in &parts {
                let name = part.name.as_str();
                if !lease::RESOURCE_PARAMETERS.contains(&name)
                    && !["size", "speaker_model"].contains(&name)
                {
                    continue;
                }
                if fields.get(name).is_some() {
                    return Err(Failure::new(422, "duplicate resource parameter"));
                }
                if part.end - part.start > 4096
                    || String::from_utf8_lossy(&part.header).contains("filename=")
                {
                    return Err(Failure::new(
                        422,
                        "resource parameter must be a bounded scalar",
                    ));
                }
                let raw = std::str::from_utf8(&bytes[part.start..part.end])
                    .map_err(|_| Failure::new(422, "invalid resource parameter"))?;
                let raw = if name == "speaker_model" && u.source == "v-voice" {
                    raw.strip_prefix("v-voice/").unwrap_or(raw)
                } else {
                    raw
                };
                fields[name] = json!(raw);
            }
            lease::validate(approval, &u, &mut fields, bytes.len())?;
        }
        if let Some(reference) = usage::url_query(&query, "speaker_model") {
            let native = if u.source == "v-voice" {
                reference.strip_prefix("v-voice/").unwrap_or(&reference)
            } else {
                &reference
            };
            if !strings(&approval.policy["secondary_models"])
                .iter()
                .any(|s| s == native)
            {
                return Err(Failure::new(
                    422,
                    "secondary model resource budget not approved",
                ));
            }
        }
        if residency::native_origin(&u).is_some() {
            exchange._model_guard=crate::modelhub::inference_guard(&native).await.map_err(|e|Failure::new(409,e.to_string()))?;
            value["keep_alive"]=json!(g.residency.seconds());
        }
        exchange.retained = u.source=="v-scheduler" && value["keep_alive"].as_u64().unwrap_or(0)>0;
        // Waiting bodies stay bounded by the input buffer, but do not consume
        // an execution slot: queued background callers must not prevent a new
        // interactive caller from entering the priority queue.
        let queued=headers.get("x-vscheduler-queue-wait-seconds").and_then(|v|v.to_str().ok()).and_then(|v|v.parse::<u64>().ok()).is_some_and(|v|v>0)
            && !(approval.policy["mode"]=="delegated" && headers.get("x-vscheduler-task-id").and_then(|v|v.to_str().ok()).is_some_and(|v|!v.is_empty()));
        if queued {exchange._slot.take();}
        exchange.lease = Some(
            lease::acquire(
                g.clone(),
                approval,
                &u,
                &native,
                &headers,
                &id,
                asynchronous,
            )
            .await?,
        );
        if let Some(slot)=exchange.lease.as_mut().and_then(|lease|lease.request_slot.take()) {exchange._slot=Some(slot);}
    }
    exchange.bridge_request = bridge::context(&value);
    let actual_suffix = if conversion.is_empty() {
        suffix.as_str()
    } else {
        "/chat/completions"
    };
    let mut rewritten = bytes.to_vec();
    if !parts.is_empty() {
        rewritten = rewrite_multipart(&bytes, &parts, &u, &native)?;
    } else if value.is_object() && (!bytes.is_empty() || !native.is_empty()) {
        if !native.is_empty() {
            value["model"] = json!(native);
        }
        rewritten = serde_json::to_vec(&value).unwrap();
    }
    let mut url = format!("{}{}", u.base_url.trim_end_matches('/'), actual_suffix);
    if !query.is_empty() {
        let mut target = reqwest::Url::parse(&format!("{url}?{query}"))
            .map_err(|_| Failure::new(400, "invalid request query"))?;
        if is_query {
            let entries = target
                .query_pairs()
                .map(|(k, v)| {
                    (
                        k.clone().into_owned(),
                        if k == "model" {
                            native.clone()
                        } else if k == "speaker_model" && u.source == "v-voice" {
                            v.strip_prefix("v-voice/").unwrap_or(&v).to_string()
                        } else {
                            v.into_owned()
                        },
                    )
                })
                .collect::<Vec<_>>();
            target.query_pairs_mut().clear().extend_pairs(entries);
        }
        url = target.to_string();
    }
    if is_websocket {
        return websocket(g, exchange, url, headers, approval, request_parts).await;
    }
    if ["offline_asr", "image_studio", "async_task", "ml_server"].contains(&u.protocol.as_str()) {
        return super::adapters::forward(g, exchange, method, headers, suffix, rewritten, value)
            .await;
    }
    if asynchronous {
        exchange.async_job = jobs::prepare(&exchange, url.clone(), &headers)?;
    }
    if method == axum::http::Method::GET {
        if let Some(response) = jobs::cached(&g, &id, &u, &url)? {
            exchange.status = response.status().as_u16();
            return Ok(response);
        }
    }
    let mut builder = outgoing(&g, &u, method, url, &headers, exchange.lease.as_ref());
    if is_query {
        // Query-routed audio and business requests retain streaming uploads.
        let remaining = approval
            .as_ref()
            .and_then(|a| a.policy["max_input_bytes"].as_u64())
            .unwrap_or(u64::MAX);
        builder = builder.body(reqwest::Body::wrap_stream(BudgetUpload {
            inner: Box::pin(incoming.take().unwrap().into_data_stream()),
            remaining,
        }));
    } else {
        builder = builder.body(rewritten);
    }
    if let Some(lease) = exchange.lease.as_ref() {
        lease.begin();
    }
    let response = match lease::guard(exchange.lease.as_ref(), async {
        builder
            .send()
            .await
            .map_err(|_| Failure::new(502, "upstream connection failed"))
    })
    .await
    {
        Ok(r) => {
            g.registry.lock().unwrap().result(&u.id, true);
            r
        }
        Err(error) => {
            if error.status == StatusCode::BAD_GATEWAY {
                g.registry.lock().unwrap().result(&u.id, false);
            }
            exchange.status = error.status.as_u16();
            return Err(error);
        }
    };
    finish_response(exchange, response, conversion).await
}
pub(super) async fn finish_response(
    mut exchange: Exchange,
    response: reqwest::Response,
    conversion: &str,
) -> Api<Response> {
    exchange.status = response.status().as_u16();
    let mut headers = clean_headers(response.headers());
    let status = response.status();
    let content_type = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if exchange.u.protocol == "video_structure" {
        headers.insert(
            "x-vrouter-service",
            exchange
                .u
                .id
                .parse()
                .map_err(|_| Failure::new(502, "invalid service identifier"))?,
        );
    }
    headers.insert(
        "x-vrouter-upstream",
        exchange
            .u
            .id
            .parse()
            .map_err(|_| Failure::new(502, "invalid upstream identifier"))?,
    );
    headers.insert("x-vrouter-routing", "least-inflight".parse().unwrap());
    if content_type.contains("text/event-stream") {
        exchange.stream = true;
        exchange.buffer.take();
        let decoder = bridge::Stream::new(
            if status.is_success() { conversion } else { "" },
            &exchange.native,
        )
        .with_request(exchange.bridge_request.clone());
        let renewal = renewal_watch(&exchange);
        let stream = ResponseStream {
            completion: None,
            finished: false,
            renewal,
            inner: Box::pin(response.bytes_stream()),
            exchange: Some(exchange),
            decoder,
            sse: true,
        };
        let mut out = Response::new(Body::from_stream(stream));
        *out.status_mut() = status;
        *out.headers_mut() = headers;
        return Ok(out);
    }
    if !conversion.is_empty() && status.is_success() {
        let bytes = lease::guard(exchange.lease.as_ref(), read_limited(response, 32 << 20)).await?;
        if bytes.len() > 32 << 20 {
            return Err(Failure::new(502, "upstream response exceeds 32 MiB"));
        }
        complete_lease(&mut exchange,"synchronous_response_completed").await;
        let mut chat: Value = serde_json::from_slice(&bytes)
            .map_err(|_| Failure::new(502, "upstream returned invalid chat JSON"))?;
        bridge::validate_response(&chat)?;
        chat["model"] = json!(exchange.native);
        exchange.usage = chat["usage"].clone();
        let mut out = axum::Json(bridge::response(
            &chat,
            conversion,
            &exchange.bridge_request,
        ))
        .into_response();
        out.headers_mut().extend(headers);
        return Ok(out);
    }
    if content_type.contains("json") {
        let bytes = lease::guard(exchange.lease.as_ref(), read_limited(response, 32 << 20)).await?;
        if bytes.len() > 32 << 20 {
            return Err(Failure::new(502, "upstream response exceeds 32 MiB"));
        }
        let asynchronous = exchange.async_job.is_some();
        if let Ok(value) = serde_json::from_slice::<Value>(&bytes) {
            exchange.usage = value["usage"].clone();
            record_owner(&exchange, &value);
            if status.is_success() {
                let job = exchange.async_job.take();
                jobs::receipt(&mut exchange, job, &value).await?;
            }
        } else if asynchronous && status.is_success() {
            return Err(Failure::new(503, "async receipt is invalid"));
        }
        if !asynchronous {
            complete_lease(&mut exchange,"synchronous_response_completed").await;
        }
        let mut out = Response::new(Body::from(bytes));
        *out.status_mut() = status;
        *out.headers_mut() = headers;
        return Ok(out);
    }
    exchange.buffer.take();
    let decoder = bridge::Stream::new("", &exchange.native);
    let renewal = renewal_watch(&exchange);
    let stream = ResponseStream {
        completion: None,
        finished: false,
        renewal,
        inner: Box::pin(response.bytes_stream()),
        exchange: Some(exchange),
        decoder,
        sse: false,
    };
    let mut out = Response::new(Body::from_stream(stream));
    *out.status_mut() = status;
    *out.headers_mut() = headers;
    Ok(out)
}
async fn complete_lease(exchange: &mut Exchange, evidence: &'static str) {
    if let Some(lease) = exchange.lease.take() {
        residency::complete(
            exchange.g.clone(),
            exchange.u.clone(),
            exchange.native.clone(),
            exchange.status,
            exchange.retained,
            lease,
            evidence,
        )
        .await;
    }
}
fn renewal_watch(
    exchange: &Exchange,
) -> Option<Pin<Box<dyn std::future::Future<Output = ()> + Send>>> {
    exchange.lease.as_ref().map(|lease| {
        let mut failed = lease.failed.clone();
        Box::pin(async move {
            loop {
                let current = *failed.borrow();
                if current {
                    break;
                }
                if failed.changed().await.is_err() {
                    break;
                }
            }
        }) as Pin<Box<dyn std::future::Future<Output = ()> + Send>>
    })
}
struct ResponseStream {
    completion: Option<Pin<Box<dyn std::future::Future<Output = Bytes> + Send>>>,
    finished: bool,
    renewal: Option<Pin<Box<dyn std::future::Future<Output = ()> + Send>>>,
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, reqwest::Error>> + Send>>,
    exchange: Option<Exchange>,
    decoder: bridge::Stream,
    sse: bool,
}
impl Stream for ResponseStream {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(completion) = self.completion.as_mut() {
            match completion.as_mut().poll(cx) {
                Poll::Pending => return Poll::Pending,
                Poll::Ready(bytes) => {
                    self.completion = None;
                    if !bytes.is_empty() {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                }
            }
        }
        if self.finished {
            return Poll::Ready(None);
        }
        if self.decoder.failed {
            self.exchange.take();
            return Poll::Ready(None);
        }
        if self
            .renewal
            .as_mut()
            .is_some_and(|future| future.as_mut().poll(cx).is_ready())
        {
            self.renewal = None;
            self.exchange.take();
            return Poll::Ready(Some(Err(std::io::Error::other(
                "scheduler lease renewal failed",
            ))));
        }
        if let Some(lease) = self.exchange.as_ref().and_then(|e| e.lease.as_ref()) {
            if *lease.failed.borrow() {
                self.exchange.take();
                return Poll::Ready(Some(Err(std::io::Error::other(
                    "scheduler lease renewal failed",
                ))));
            }
        }
        loop {
            match self.inner.as_mut().poll_next(cx) {
                Poll::Ready(Some(Ok(bytes))) => {
                    if !self.sse {
                        return Poll::Ready(Some(Ok(bytes)));
                    }
                    let output = match self.decoder.feed(&bytes) {
                        Ok(v) => v,
                        Err(_) => {
                            self.exchange.take();
                            return Poll::Ready(Some(Err(std::io::Error::other(
                                "invalid upstream SSE frame",
                            ))));
                        }
                    };
                    let terminal = self.decoder.terminal;
                    let usage = self.decoder.usage.clone();
                    if terminal {
                        self.renewal = None;
                    }
                    if let Some(exchange) = self.exchange.as_mut() {
                        exchange.usage = usage;
                        if terminal {
                            if let Some(lease) = exchange.lease.take() {
                                let (g,u,model,status,retained)=(exchange.g.clone(),exchange.u.clone(),exchange.native.clone(),exchange.status,exchange.retained);
                                self.completion = Some(Box::pin(async move {
                                    residency::complete(g,u,model,status,retained,lease,"upstream_sse_terminal_received").await;
                                    Bytes::from(output)
                                }));
                                return self.poll_next(cx);
                            }
                        }
                    }
                    if !output.is_empty() {
                        return Poll::Ready(Some(Ok(Bytes::from(output))));
                    }
                }
                Poll::Ready(Some(Err(_))) => {
                    self.exchange.take();
                    return Poll::Ready(Some(Err(std::io::Error::other("upstream stream failed"))));
                }
                Poll::Ready(None) => {
                    self.renewal = None;
                    let sse = self.sse;
                    let final_events = if sse { self.decoder.eof() } else { vec![] };
                    let terminal = self.decoder.terminal;
                    if let Some(mut exchange) = self.exchange.take() {
                        if !sse || terminal {
                            if let Some(lease) = exchange.lease.take() {
                                self.finished = true;
                                self.completion = Some(Box::pin(async move {
                                    residency::complete(exchange.g.clone(),exchange.u.clone(),exchange.native.clone(),exchange.status,exchange.retained,lease,"synchronous_response_completed").await;
                                    Bytes::from(final_events)
                                }));
                                return self.poll_next(cx);
                            }
                        }
                    }
                    if !final_events.is_empty() {
                        return Poll::Ready(Some(Ok(Bytes::from(final_events))));
                    }
                    return Poll::Ready(None);
                }
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}
async fn websocket(
    g: Arc<Gateway>,
    mut exchange: Exchange,
    url: String,
    headers: HeaderMap,
    approval: Option<lease::Approval>,
    mut parts: axum::http::request::Parts,
) -> Api<Response> {
    use axum::extract::{
        FromRequestParts,
        ws::{Message as DownMessage, WebSocketUpgrade},
    };
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::{Message as UpMessage, client::IntoClientRequest};
    if approval
        .as_ref()
        .is_some_and(|a| a.policy["websocket_completion"] != "server_close")
    {
        return Err(Failure::new(
            503,
            "WebSocket requires an approved server-close completion contract",
        ));
    }
    let upgrade = WebSocketUpgrade::from_request_parts(&mut parts, &())
        .await
        .map_err(|_| Failure::new(400, "invalid WebSocket upgrade request"))?;
    let url = url
        .replacen("http://", "ws://", 1)
        .replacen("https://", "wss://", 1);
    let mut outgoing = url
        .into_client_request()
        .map_err(|_| Failure::new(400, "invalid WebSocket upstream URL"))?;
    for (name, value) in clean_headers(&headers) {
        if let Some(name) = name {
            if !name.as_str().starts_with("sec-websocket-")
                || name.as_str() == "sec-websocket-protocol"
            {
                outgoing.headers_mut().insert(name, value);
            }
        }
    }
    if !exchange.u.api_key.is_empty() {
        outgoing.headers_mut().insert(
            "authorization",
            format!("Bearer {}", exchange.u.api_key).parse().unwrap(),
        );
    }
    if let Some(lease) = exchange.lease.as_ref() {
        outgoing
            .headers_mut()
            .insert("x-vscheduler-permit-id", lease.id.parse().unwrap());
        outgoing
            .headers_mut()
            .insert("x-vscheduler-execution-token", lease.proof.parse().unwrap());
        outgoing
            .headers_mut()
            .insert("x-vscheduler-executor-id", lease.executor.parse().unwrap());
    }
    if let Some(lease) = exchange.lease.as_ref() {
        lease.begin();
    }
    let (upstream, handshake) = match lease::guard(exchange.lease.as_ref(), async {
        tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(outgoing),
        )
        .await
        .map_err(|_| Failure::new(502, "upstream WebSocket upgrade timed out"))?
        .map_err(|_| Failure::new(502, "upstream WebSocket upgrade failed"))
    })
    .await
    {
        Ok(value) => value,
        Err(error) => {
            if error.status == StatusCode::BAD_GATEWAY {
                g.registry.lock().unwrap().result(&exchange.u.id, false);
            }
            return Err(error);
        }
    };
    let upgrade = if let Some(selected) = handshake
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
    {
        upgrade.protocols([selected.to_owned()])
    } else {
        upgrade
    };
    exchange.status = 101;
    exchange.buffer.take();
    g.registry.lock().unwrap().result(&exchange.u.id, true);
    let maximum = approval
        .as_ref()
        .and_then(|a| a.policy["max_input_bytes"].as_u64())
        .unwrap_or(u64::MAX);
    Ok(upgrade.on_upgrade(move|mut downstream|async move{
        let(mut upstream_tx,mut upstream_rx)=upstream.split();let mut uploaded=0u64;let mut closing=false;let mut close_deadline=std::pin::pin!(tokio::time::sleep(Duration::from_secs(5)));
        loop{if exchange.lease.as_ref().is_some_and(|lease|*lease.failed.borrow()){break;}tokio::select!{
            frame=downstream.recv(),if !closing=>{
                let Some(Ok(frame))=frame else{break;};
                let frame=match frame{DownMessage::Text(t)=>{uploaded+=t.len()as u64;UpMessage::Text(t.as_str().into())},DownMessage::Binary(b)=>{uploaded+=b.len()as u64;UpMessage::Binary(b)},DownMessage::Ping(b)=>UpMessage::Ping(b),DownMessage::Pong(b)=>UpMessage::Pong(b),DownMessage::Close(c)=>{closing=true;close_deadline.as_mut().reset(tokio::time::Instant::now()+Duration::from_secs(5));UpMessage::Close(c.map(|c|tokio_tungstenite::tungstenite::protocol::CloseFrame{code:c.code.into(),reason:c.reason.as_str().into()}))}};
                if uploaded>maximum{let _=downstream.send(DownMessage::Close(Some(axum::extract::ws::CloseFrame{code:4422,reason:"input exceeds approved byte budget".into()}))).await;break;}if upstream_tx.send(frame).await.is_err(){break;}
            },
            frame=upstream_rx.next()=>{
                let Some(Ok(frame))=frame else{break;};
                let frame=match frame{UpMessage::Text(t)=>DownMessage::Text(t.as_str().into()),UpMessage::Binary(b)=>DownMessage::Binary(b),UpMessage::Ping(b)=>DownMessage::Ping(b),UpMessage::Pong(b)=>DownMessage::Pong(b),UpMessage::Close(c)=>{if let Some(lease)=exchange.lease.as_mut(){lease.complete("websocket_server_close_received");lease.release_confirmed("websocket_server_close_received").await;}let _=downstream.send(DownMessage::Close(c.map(|c|axum::extract::ws::CloseFrame{code:c.code.into(),reason:c.reason.as_str().into()}))).await;break;},UpMessage::Frame(_)=>continue};
                if downstream.send(frame).await.is_err(){break;}
            },
            _=&mut close_deadline,if closing=>break,
            _=tokio::time::sleep(Duration::from_secs(1))=>{if exchange.lease.as_ref().is_some_and(|l|*l.failed.borrow()){break;}},
        }}
        drop(exchange);
    }))
}

struct BudgetUpload {
    inner: Pin<Box<dyn Stream<Item = Result<Bytes, axum::Error>> + Send>>,
    remaining: u64,
}
impl Stream for BudgetUpload {
    type Item = Result<Bytes, std::io::Error>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        match self.inner.as_mut().poll_next(cx) {
            Poll::Ready(Some(Ok(chunk))) => {
                if chunk.len() as u64 > self.remaining {
                    self.remaining = 0;
                    Poll::Ready(Some(Err(std::io::Error::other(
                        "approved model input budget exceeded",
                    ))))
                } else {
                    self.remaining -= chunk.len() as u64;
                    Poll::Ready(Some(Ok(chunk)))
                }
            }
            Poll::Ready(Some(Err(_))) => {
                Poll::Ready(Some(Err(std::io::Error::other("input upload failed"))))
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Pending => Poll::Pending,
        }
    }
}
