use super::*;
use axum::Json;
fn view(r: &Registry, u: &Upstream, id: &Identity) -> Value {
    let mut v = serde_json::to_value(u).unwrap();
    v["api_key"] = json!("");
    v["has_key"] = json!(!u.api_key.is_empty());
    v["model_ids"] = json!(
        r.models
            .get(&u.id)
            .into_iter()
            .flatten()
            .map(|m| m.id.clone())
            .collect::<Vec<_>>()
    );
    v["models"] = json!(r.models.get(&u.id).map_or(0, Vec::len));
    v["routing"] = json!(r.routing.get(&u.id).cloned().unwrap_or_default());
    v["inflight"] = json!(r.inflight.get(&u.id).copied().unwrap_or(0));
    v["status"] = r.status(u);
    v["manageable"] = json!(u.manageable(id));
    v
}
fn writable(r: &Registry, key: &str, id: &Identity) -> Api<Upstream> {
    let u = r
        .upstreams
        .get(key)
        .filter(|u| u.visible(id))
        .ok_or_else(|| Failure::new(404, "upstream not found"))?;
    if !u.manageable(id) {
        return Err(Failure::new(
            403,
            "upstream is managed by its owner or an administrator",
        ));
    }
    Ok(u.clone())
}
fn business_views(r: &Registry) -> (Vec<Value>, Vec<Value>) {
    let mut upstreams = vec![];
    let mut models = vec![];
    for (id, name) in [
        ("video-structure-detect", "视频结构化 · 场景检测"),
        ("video-structure-structure", "视频结构化 · 结构化与描述"),
    ] {
        let Some(base) = r.services.get(id) else {
            continue;
        };
        let mut count = 0;
        for route in route_specs().iter().filter(|route| {
            route["group"] == "video-structure"
                && text(route, "path") != "/api/v1/video-structure/status"
        }) {
            let path = text(route, "path");
            let service = if path.contains("/scenes/") {
                "video-structure-detect"
            } else {
                "video-structure-structure"
            };
            if service != id {
                continue;
            }
            count += 1;
            let qualified = format!("{} {path}", text(route, "method"));
            models.push(json!({"id":qualified,"qualified":qualified,"short":false,"kind":"service","capabilities":["video_structure"],"endpoints":[path],"upstream_id":id,"upstream_name":name,"upstream_kind":"local","upstream_source":"video-structure"}));
        }
        upstreams.push(json!({"id":id,"name":name,"kind":"local","base_url":base,"protocol":"business_api","source":"video-structure","healthy":true,"api_key":"","has_key":false,"model_ids":[],"models":count,"inflight":0,"routing":{"weight":1,"max_inflight":0,"disabled":false},"status":{"state":"healthy"},"manageable":false}));
    }
    (upstreams, models)
}
fn upstream_views(r: &Registry, id: &Identity) -> Vec<Value> {
    let mut rows = r
        .upstreams
        .values()
        .filter(|u| u.visible(id))
        .map(|u| view(r, u, id))
        .collect::<Vec<_>>();
    rows.extend(business_views(r).0);
    rows
}
fn model_views(r: &Registry, id: &Identity) -> Vec<Value> {
    let mut rows = r
        .list_models(id)
        .iter()
        .map(|m| {
            let mut v = json!(m);
            let u = &r.upstreams[&m.upstream_id];
            v["short"] = json!(false);
            v["upstream_name"] = json!(u.name);
            v["upstream_kind"] = json!(u.kind);
            v["upstream_source"] = json!(u.source);
            v["model_prefix"] = json!(u.prefix());
            if u.source == "api" && m.kind == "chat" {
                v["kind"] = json!("unknown");
            }
            v
        })
        .collect::<Vec<_>>();
    rows.extend(business_views(r).1);
    rows
}
pub(super) fn normalize(value: &Value) -> Api<Upstream> {
    for key in [
        "name",
        "kind",
        "base_url",
        "api_key",
        "protocol",
        "visibility",
        "test_model",
        "model",
        "service_id",
        "model_kind",
    ] {
        if value
            .get(key)
            .is_some_and(|v| !v.is_null() && !v.is_string())
        {
            return Err(Failure::new(400, format!("{key} must be text")));
        }
    }
    let mut base = text(value, "base_url")
        .trim()
        .trim_end_matches('/')
        .to_string();
    if !base.contains("://") {
        base = format!("https://{base}");
    }
    let protocol = match text(value, "protocol") {
        "" => "openai",
        p => p,
    }
    .to_ascii_lowercase();
    if ![
        "openai",
        "chat_completions",
        "anthropic",
        "offline_asr",
        "image_studio",
        "async_task",
        "ml_server",
    ]
    .contains(&protocol.as_str())
    {
        return Err(Failure::new(400, "unsupported upstream protocol"));
    }
    let parsed = reqwest::Url::parse(&base)
        .map_err(|_| Failure::new(400, "base_url must be an HTTP(S) URL"))?;
    if !["http", "https"].contains(&parsed.scheme())
        || parsed.host_str().is_none()
        || !parsed.username().is_empty()
        || parsed.password().is_some()
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(Failure::new(400, "invalid upstream origin"));
    }
    let host = parsed.host_str().unwrap();
    if [
        "v-router",
        "v-router-backend",
        "v-scheduler-gateway",
        "v-scheduler",
        "v-scheduler-backend",
        "v-scheduler-frontend",
    ]
    .contains(&host)
        || ["localhost", "127.0.0.1", "::1"].contains(&host)
        || host.ends_with(".vos_default")
        || host.starts_with("model-hub-")
    {
        return Err(Failure::new(400, "upstream cannot point to this gateway"));
    }
    if matches!(
        protocol.as_str(),
        "openai" | "chat_completions" | "anthropic"
    ) && parsed.path() == "/"
    {
        base.push_str("/v1");
    }
    let kind = match text(value, "kind") {
        "cloud" => "cloud",
        "manual" | "" => "manual",
        _ => return Err(Failure::new(400, "kind must be manual or cloud")),
    };
    let visibility = match text(value, "visibility") {
        "private" => "private",
        "public" | "" => "public",
        _ => return Err(Failure::new(400, "visibility must be public or private")),
    };
    let prefix = if kind == "cloud" {
        host.trim_start_matches("www.")
            .split('.')
            .rev()
            .collect::<Vec<_>>()
            .join(".")
    } else {
        "manual".into()
    };
    if protocol == "async_task" {
        validate_async(value)?;
    }
    Ok(Upstream {
        name: text(value, "name").trim().into(),
        kind: kind.into(),
        base_url: base,
        api_key: text(value, "api_key").into(),
        protocol,
        visibility: visibility.into(),
        model_prefix: prefix,
        async_task: value.get("async_task").filter(|v| !v.is_null()).cloned(),
        ..Default::default()
    })
}
fn validate_async(value: &Value) -> Api<()> {
    if text(value, "model").is_empty() {
        return Err(Failure::new(400, "async_task requires a model"));
    }
    let c = &value["async_task"];
    for key in [
        "create_path",
        "query_path",
        "task_id_path",
        "status_path",
        "result_path",
    ] {
        if text(c, key).trim().is_empty() {
            return Err(Failure::new(400, format!("async_task.{key} is required")));
        }
    }
    if strings(&c["success_states"]).is_empty() || strings(&c["failure_states"]).is_empty() {
        return Err(Failure::new(
            400,
            "async task success and failure states are required",
        ));
    }
    if !c["poll_interval_ms"]
        .as_u64()
        .is_some_and(|n| (100..=60000).contains(&n))
        || !c["timeout_seconds"]
            .as_u64()
            .is_some_and(|n| (1..=3600).contains(&n))
    {
        return Err(Failure::new(
            400,
            "invalid async polling interval or timeout",
        ));
    }
    Ok(())
}
fn async_models(value: &Value) -> Vec<Model> {
    vec![Model {
        id: text(value, "model").into(),
        kind: "unknown".into(),
        capabilities: vec!["image_generation".into()],
        endpoints: vec!["images/generations".into()],
        ..Default::default()
    }]
}
pub(super) async fn models(g: &Gateway, u: &Upstream) -> Api<Vec<Model>> {
    if u.protocol == "image_studio" {
        let base = u.base_url.trim_end_matches('/');
        let api = if base.ends_with("/api/v1") {
            base.to_string()
        } else {
            format!("{base}/api/v1")
        };
        let response = proxy::outgoing(
            g,
            u,
            axum::http::Method::GET,
            format!("{api}/models"),
            &HeaderMap::new(),
            None,
        )
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| Failure::new(502, "Image Studio metadata unavailable"))?;
        if !response.status().is_success() {
            return Err(Failure::new(502, "Image Studio metadata probe failed"));
        }
        return Ok(discovery::image_models(
            &read_json(response, 4 << 20).await?,
        ));
    }
    let mut next = format!("{}/models", u.base_url.trim_end_matches('/'));
    if u.protocol == "anthropic" {
        next.push_str("?limit=100");
    }
    let mut result = vec![];
    for _ in 0..100 {
        let response = proxy::outgoing(
            g,
            u,
            axum::http::Method::GET,
            next.clone(),
            &HeaderMap::new(),
            None,
        )
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| Failure::new(502, "upstream models endpoint unavailable"))?;
        if !response.status().is_success() {
            return Err(Failure::new(
                502,
                format!(
                    "upstream models endpoint returned HTTP {}",
                    response.status().as_u16()
                ),
            ));
        }
        let value = read_json(response, 1 << 20).await?;
        let rows = value["data"]
            .as_array()
            .or(value["models"].as_array())
            .or(value.as_array())
            .ok_or_else(|| Failure::new(502, "upstream did not return a model list"))?;
        for v in rows {
            let native = v.as_str().unwrap_or_else(|| text(v, "id"));
            if native.is_empty() {
                continue;
            }
            result.push(Model {
                id: native.into(),
                qualified: format!("{}/{native}", u.prefix()),
                upstream_id: u.id.clone(),
                kind: match text(v, "kind") {
                    "" => "unknown",
                    v => v,
                }
                .into(),
                capabilities: strings(&v["capabilities"]),
                endpoints: strings(&v["endpoints"]),
                owned_by: text(v, "owned_by").into(),
            });
        }
        if u.protocol != "anthropic" || value["has_more"] != true {
            return Ok(result);
        }
        let last = text(&value, "last_id");
        if rows.is_empty() || last.is_empty() {
            return Err(Failure::new(502, "invalid Anthropic model pagination"));
        }
        let mut target =
            reqwest::Url::parse(&format!("{}/models", u.base_url.trim_end_matches('/')))
                .map_err(|_| Failure::new(502, "invalid models endpoint"))?;
        target
            .query_pairs_mut()
            .append_pair("limit", "100")
            .append_pair("after_id", last);
        if target.as_str() == next {
            return Err(Failure::new(
                502,
                "Anthropic model pagination did not advance",
            ));
        }
        next = target.to_string();
    }
    Err(Failure::new(
        502,
        "Anthropic model pagination exceeded 100 pages",
    ))
}
async fn probe_model(g: &Gateway, u: &Upstream, model: &str) -> Api<()> {
    let suffix = if u.protocol == "anthropic" {
        "/messages"
    } else {
        "/chat/completions"
    };
    let response = proxy::outgoing(
        g,
        u,
        axum::http::Method::POST,
        format!("{}{suffix}", u.base_url),
        &HeaderMap::new(),
        None,
    )
    .timeout(Duration::from_secs(10))
    .json(&json!({"model":model,"max_tokens":1,"messages":[{"role":"user","content":"ping"}]}))
    .send()
    .await
    .map_err(|_| Failure::new(502, "model test connection failed"))?;
    if !response.status().is_success() {
        return Err(Failure::new(
            502,
            format!("model test returned HTTP {}", response.status().as_u16()),
        ));
    }
    Ok(())
}
pub(super) async fn handle(g: &Arc<Gateway>, id: &Identity, request: Request) -> Api<Response> {
    let path = request.uri().path().to_string();
    let method = request.method().as_str().to_string();
    if method == "GET" {
        let r = g.registry.lock().unwrap();
        match path.as_str() {
            "/api/v1/auth/me"=>return Ok(Json(json!({"identity":{"sub":id.owner,"username":id.username,"email":id.email,"admin":id.admin},"auth_method":if env("V_ROUTER_AUTH_DISABLED","")=="true"{"auth_disabled"}else if id.admin{"gateway_key"}else{"vos_oidc"},"permissions":["inference","management","upstream_resources"],"resource_isolation":true,"registration_scopes":["public","private"]})).into_response()),
            "/api/v1/upstreams"=>return Ok(Json(json!({"upstreams":upstream_views(&r,id)})).into_response()),
            "/api/v1/models"=>return Ok(Json(json!({"models":model_views(&r,id)})).into_response()),
            "/api/v1/status"=>return Ok(Json(json!({"version":version(),"upstreams_total":r.upstreams.values().filter(|u|u.visible(id)).count(),"upstreams_healthy":r.upstreams.values().filter(|u|u.visible(id)&&u.live()).count(),"models":r.list_models(id).len(),"requests_total":g.usage.lock().unwrap().len(),"key_required":env("V_ROUTER_REQUIRE_KEY","")=="true","oidc_enabled":true,"user":id.username,"is_admin":id.admin,"base_urls":gateway_base_urls()})).into_response()),
            "/api/v1/usage"=>return Ok(Json(g.usage.lock().unwrap().snapshot(request.uri().query().unwrap_or(""))).into_response()),
            "/api/v1/apidoc"=>{let query=request.uri().query().unwrap_or("");let mut response=Json(documentation(query)).into_response();response.headers_mut().insert("cache-control","no-store".parse().unwrap());response.headers_mut().insert("content-language",if usage::url_query(query,"lang").is_some_and(|s|s.to_ascii_lowercase().starts_with("en")){"en"}else{"zh-CN"}.parse().unwrap());return Ok(response);},
            "/api/v1/openapi.json"=>return Ok(Json(openapi(request.uri().query().unwrap_or(""))).into_response()),
            _=>{}
        }
    }
    let bytes = axum::body::to_bytes(request.into_body(), 1 << 20)
        .await
        .map_err(|_| Failure::new(413, "management request body too large"))?;
    let payload: Value = if bytes.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(&bytes).map_err(|_| Failure::new(400, "invalid JSON request"))?
    };
    if path == "/api/v1/registrations" && method == "POST" {
        return register(g, id, payload).await;
    }
    if path == "/api/v1/upstreams" && method == "POST"
        || path == "/api/v1/upstreams/probe" && method == "POST"
    {
        let probe = path.ends_with("/probe");
        let mut u = normalize(&payload)?;
        if u.name.is_empty() && !probe {
            return Err(Failure::new(400, "name is required"));
        }
        let fetched = if u.protocol == "async_task" {
            let model = text(&payload, "model");
            if model.is_empty() || u.async_task.is_none() {
                return Err(Failure::new(
                    400,
                    "async_task requires model and task configuration",
                ));
            }
            Ok(async_models(&payload))
        } else {
            models(g, &u).await
        };
        if probe {
            let mut report = json!({"ok":fetched.is_ok(),"base_url":u.base_url,"model_prefix":u.prefix(),"protocol":u.protocol,"models":fetched.as_ref().map(|ms|ms.iter().map(|m|m.id.clone()).collect::<Vec<_>>()).unwrap_or_default(),"error":fetched.err().map(|e|e.message).unwrap_or_default()});
            let test = text(&payload, "test_model");
            if !test.is_empty() {
                match probe_model(g, &u, test).await {
                    Ok(()) => report["test_ok"] = json!(true),
                    Err(error) => report["test_error"] = json!(error.message),
                }
            }
            return Ok(Json(report).into_response());
        }
        let models = match fetched {
            Ok(m) => {
                u.healthy = true;
                m
            }
            Err(e) if payload["allow_probe_failure"] == true => {
                u.healthy = true;
                u.last_error = e.message;
                if text(&payload, "test_model").is_empty() {
                    vec![]
                } else {
                    vec![Model {
                        id: text(&payload, "test_model").into(),
                        kind: "unknown".into(),
                        ..Default::default()
                    }]
                }
            }
            Err(e) => return Err(e),
        };
        u.owner = id.owner.clone();
        u.source = "api".into();
        let mut r = g.registry.lock().unwrap();
        let slug = u
            .name
            .to_ascii_lowercase()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect::<String>();
        let slug = slug.trim_matches('-');
        let slug = if slug.is_empty() { "upstream" } else { slug };
        u.id = slug.into();
        let mut serial = 2;
        while r.upstreams.contains_key(&u.id) {
            u.id = format!("{slug}-{serial}");
            serial += 1;
        }
        let key = u.id.clone();
        r.put(u, models);
        r.persist(&g.state_path)
            .map_err(|_| Failure::new(500, "cannot persist upstream"))?;
        return Ok((StatusCode::CREATED, Json(view(&r, &r.upstreams[&key], id))).into_response());
    }
    if let Some(tail) = path.strip_prefix("/api/v1/upstreams/") {
        let (key, action) = tail.split_once('/').unwrap_or((tail, ""));
        let mut u = {
            let r = g.registry.lock().unwrap();
            writable(&r, key, id)?
        };
        if action == "routing" && method == "PUT" {
            let mut policy: registry::Routing = serde_json::from_value(payload)
                .map_err(|_| Failure::new(400, "invalid routing policy"))?;
            if !(0..=1000).contains(&policy.weight) || policy.max_inflight > 100000 {
                return Err(Failure::new(400, "invalid routing limits"));
            }
            policy.weight = policy.weight.max(1);
            let mut r = g.registry.lock().unwrap();
            r.routing.insert(key.into(), policy.clone());
            r.persist(&g.state_path)
                .map_err(|_| Failure::new(500, "cannot persist routing"))?;
            return Ok(Json(
                json!({"routing":policy,"inflight":r.inflight.get(key).copied().unwrap_or(0)}),
            )
            .into_response());
        }
        if u.source != "api" {
            return Err(Failure::new(
                409,
                "only API-created upstreams can be changed here",
            ));
        }
        if method == "DELETE" && action.is_empty() {
            let mut r = g.registry.lock().unwrap();
            r.upstreams.remove(key);
            r.models.remove(key);
            r.resources.retain(|v| text(v, "upstream") != key);
            r.persist(&g.state_path)
                .map_err(|_| Failure::new(500, "cannot persist upstream deletion"))?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
        if method == "PUT" && action.is_empty() {
            let mut updated = payload.clone();
            for (key, existing) in [
                ("kind", u.kind.as_str()),
                ("visibility", u.visibility.as_str()),
            ] {
                if text(&updated, key).is_empty() {
                    updated[key] = json!(existing);
                }
            }
            let mut replacement = normalize(&updated)?;
            if replacement.name.is_empty() {
                return Err(Failure::new(400, "name is required"));
            }
            replacement.id = u.id.clone();
            replacement.owner = u.owner.clone();
            replacement.source = u.source.clone();
            replacement.created_at = u.created_at;
            if replacement.api_key.is_empty() {
                replacement.api_key = u.api_key;
            }
            u = replacement;
        } else if !(method == "POST" && action == "refresh") {
            return Err(Failure::new(405, "method not allowed"));
        }
        let models = if u.protocol == "async_task" {
            if action == "refresh" {
                g.registry
                    .lock()
                    .unwrap()
                    .models
                    .get(key)
                    .cloned()
                    .unwrap_or_default()
            } else {
                async_models(&payload)
            }
        } else {
            match models(g, &u).await {
                Ok(rows) => rows,
                Err(error) if payload["allow_probe_failure"] == true => {
                    u.last_error = error.message;
                    if text(&payload, "test_model").is_empty() {
                        vec![]
                    } else {
                        vec![Model {
                            id: text(&payload, "test_model").into(),
                            ..Default::default()
                        }]
                    }
                }
                Err(error) => return Err(error),
            }
        };
        u.healthy = true;
        if payload["allow_probe_failure"] != true {
            u.last_error.clear();
        }
        let mut r = g.registry.lock().unwrap();
        r.put(u, models);
        r.persist(&g.state_path)
            .map_err(|_| Failure::new(500, "cannot persist upstream"))?;
        return Ok(Json(view(&r, &r.upstreams[key], id)).into_response());
    }
    if let Some(tail) = path.strip_prefix("/api/v1/registrations/") {
        let (key, action) = tail.split_once('/').unwrap_or((tail, ""));
        let mut r = g.registry.lock().unwrap();
        let mut u =
            writable(&r, key, id).map_err(|_| Failure::new(404, "registration not found"))?;
        if u.source != "registered" {
            return Err(Failure::new(409, "upstream is not a registration"));
        }
        if action == "heartbeat" && method == "POST" {
            if !u.live() {
                return Err(Failure::new(
                    404,
                    "registration missing or expired; register again",
                ));
            }
            let ttl = ttl(&payload)?;
            u.expires_at = Some(Utc::now() + chrono::Duration::seconds(ttl));
            r.upstreams.insert(key.into(), u.clone());
            r.breaker.remove(key);
            r.persist(&g.state_path)
                .map_err(|_| Failure::new(500, "cannot persist registration"))?;
            return Ok(Json(json!({"upstream_id":key,"expires_at":u.expires_at,"heartbeat_after_seconds":ttl/3})).into_response());
        }
        if action.is_empty() && method == "DELETE" {
            r.upstreams.remove(key);
            r.models.remove(key);
            r.persist(&g.state_path)
                .map_err(|_| Failure::new(500, "cannot persist registration"))?;
            return Ok(StatusCode::NO_CONTENT.into_response());
        }
    }
    Err(Failure::new(404, "unknown API endpoint"))
}
fn ttl(v: &Value) -> Api<i64> {
    if v.get("ttl_seconds")
        .is_some_and(|v| !v.is_null() && v.as_i64().is_none())
    {
        return Err(Failure::new(400, "ttl_seconds must be an integer"));
    }
    let ttl = v["ttl_seconds"].as_i64().filter(|n| *n != 0).unwrap_or(90);
    if !(30..=3600).contains(&ttl) {
        Err(Failure::new(400, "ttl_seconds must be 30..3600"))
    } else {
        Ok(ttl)
    }
}
async fn register(g: &Gateway, id: &Identity, p: Value) -> Api<Response> {
    let service = text(&p, "service_id");
    if service.is_empty()
        || service.len() > 96
        || !service
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c))
        || !service.as_bytes()[0].is_ascii_alphanumeric()
    {
        return Err(Failure::new(400, "invalid service_id"));
    }
    let ttl = ttl(&p)?;
    let mut u = normalize(&p)?;
    u.id = format!("registered-{service}");
    u.name = if u.name.is_empty() {
        service.into()
    } else {
        u.name
    };
    u.kind = "registered".into();
    u.source = "registered".into();
    u.model_prefix = "registered".into();
    u.owner = id.owner.clone();
    u.healthy = true;
    u.expires_at = Some(Utc::now() + chrono::Duration::seconds(ttl));
    if u.visibility == "private" && id.owner.is_empty() {
        return Err(Failure::new(401, "private registration requires identity"));
    }
    if let Some(policy) = p.get("scheduler").filter(|p| !p.is_null()) {
        if !id.admin {
            return Err(Failure::new(
                403,
                "scheduler policy requires administrator approval",
            ));
        }
        let mut policy = policy.clone();
        if !["cloud", "permit", "delegated"].contains(&text(&policy, "mode")) {
            return Err(Failure::new(400, "invalid scheduler mode"));
        }
        if policy["mode"] == "permit"
            && (text(&policy, "executor_id").is_empty() || policy["completion"] != "synchronous")
        {
            return Err(Failure::new(
                400,
                "permit requires executor and synchronous completion",
            ));
        }
        policy["upstream_url"] = json!(u.base_url);
        u.scheduler = Some(policy);
    }
    {
        let r = g.registry.lock().unwrap();
        if let Some(old) = r.upstreams.get(&u.id) {
            if old.source != "registered" || !old.manageable(id) {
                return Err(Failure::new(
                    409,
                    "service_id belongs to another owner or source",
                ));
            }
        }
    }
    let mut items = p["models"].as_array().cloned().unwrap_or_default();
    if !text(&p, "model").is_empty() {
        items.push(json!({"id":p["model"],"model_kind":p["model_kind"]}));
    }
    let explicit = !items.is_empty();
    let mut models = if explicit {
        items
            .iter()
            .map(|m| Model {
                id: if text(m, "id").is_empty() {
                    text(m, "model")
                } else {
                    text(m, "id")
                }
                .into(),
                kind: if text(m, "model_kind").is_empty() {
                    text(&p, "model_kind")
                } else {
                    text(m, "model_kind")
                }
                .into(),
                capabilities: strings(&m["capabilities"]),
                endpoints: strings(&m["endpoints"]),
                owned_by: text(m, "owned_by").into(),
                ..Default::default()
            })
            .collect()
    } else {
        models(g, &u).await?
    };
    for m in &mut models {
        if m.id.is_empty() {
            return Err(Failure::new(400, "registered model id is required"));
        }
        m.capabilities.extend(strings(&p["capabilities"]));
        m.capabilities
            .extend(strings(&p["model_capabilities"][&m.id]));
        m.endpoints.extend(strings(&p["endpoints"]));
        m.endpoints.extend(strings(&p["model_endpoints"][&m.id]));
        m.endpoints = m.endpoints.iter().map(|s| registry::endpoint(s)).collect();
        m.endpoints.sort();
        m.endpoints.dedup();
        m.capabilities.sort();
        m.capabilities.dedup();
        if explicit && m.endpoints.is_empty() {
            return Err(Failure::new(
                400,
                "registered models must declare endpoints",
            ));
        }
        if m.kind.is_empty() {
            m.kind = "unknown".into();
        }
        if !["chat", "embedding", "unknown"].contains(&m.kind.as_str()) {
            return Err(Failure::new(400, "invalid registered model kind"));
        }
    }
    let mut r = g.registry.lock().unwrap();
    let key = u.id.clone();
    r.put(u, models);
    Ok(
        Json(json!({"upstream":view(&r,&r.upstreams[&key],id),"heartbeat_after_seconds":ttl/3}))
            .into_response(),
    )
}
fn gateway_base_urls() -> Value {
    json!({
        "internal": "http://v-scheduler:8080/v1",
        "compatibility_aliases": ["http://v-scheduler-gateway:8080/v1", "http://v-router:8080/v1", "http://v-router-backend:8080/v1"],
        "lan": format!("http://<vos-host>:{}/v1", env("V_ROUTER_HOST_PORT", "22888")),
        "same_origin": "/api/com.ictrek.v-scheduler/v1"
    })
}
fn documentation(query: &str) -> Value {
    let english =
        usage::url_query(query, "lang").is_some_and(|s| s.to_ascii_lowercase().starts_with("en"));
    let source = if english {
        include_str!("../../../api/router-documentation-en.json")
    } else {
        include_str!("../../../api/router-documentation-zh-CN.json")
    };
    let mut doc: Value = serde_json::from_str(source).expect("validated gateway documentation");
    doc["version"] = json!(version());
    doc["base_urls"] = gateway_base_urls();
    doc["key_required"] = json!(env("V_ROUTER_REQUIRE_KEY", "") == "true");
    doc["oidc_enabled"] = json!(env("V_ROUTER_AUTH_DISABLED", "") != "true");
    doc
}
fn openapi(query: &str) -> Value {
    let english =
        usage::url_query(query, "lang").is_some_and(|s| s.to_ascii_lowercase().starts_with("en"));
    let mut document: Value = serde_json::from_str(if english {
        include_str!("../../../api/router-openapi-en.json")
    } else {
        include_str!("../../../api/router-openapi-zh-CN.json")
    })
    .expect("validated Router OpenAPI");
    document["info"]["version"] = json!(version());
    document
}
