use super::*;
use axum::body::Bytes;
use axum::{Json, routing::post};
use futures_util::StreamExt;
use std::collections::BTreeMap;
#[tokio::test]
async fn vos_gateway_administrator_requires_server_verified_enabled_role() {
    for (status, value, expected) in [
        (200, json!({"code":0,"data":{"id":"admin-id","role_code":"system_admin","enable":1,"delete":0}}), true),
        (200, json!({"code":0,"data":{"id":"user-id","role_code":"user","enable":1,"delete":0}}), false),
        (200, json!({"code":0,"data":{"id":"admin-id","role_code":"system_admin","enable":0,"delete":0}}), false),
        (200, json!({"code":0,"data":{"id":"admin-id","role_code":"system_admin","enable":1,"delete":1}}), false),
        (200, json!({"code":10001,"data":{"id":"admin-id","role_code":"system_admin","enable":1,"delete":0}}), false),
        (401, json!({"code":10001}), false),
    ] {
        let server = Router::new().route("/v1000/user/check", axum::routing::get(move |headers: HeaderMap| async move {
            assert_eq!(headers["authorization"], "Bearer fixture-token");
            (StatusCode::from_u16(status).unwrap(), Json(value))
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(async move { axum::serve(listener, server).await.unwrap(); });
        assert_eq!(auth::administrator(&reqwest::Client::new(), &format!("http://{address}/v1000/oauth2/userinfo"), "fixture-token").await, expected);
        task.abort();
    }
}
pub(super) fn principal(name: &str) -> Identity {
    Identity {
        owner: name.into(),
        username: name.into(),
        ..Default::default()
    }
}
pub(super) fn upstream(id: &str) -> Upstream {
    Upstream {
        id: id.into(),
        name: id.into(),
        kind: "manual".into(),
        source: "api".into(),
        base_url: "http://fixture/v1".into(),
        protocol: "chat_completions".into(),
        model_prefix: "manual".into(),
        healthy: true,
        ..Default::default()
    }
}
pub(super) fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        kind: "chat".into(),
        ..Default::default()
    }
}

#[test]
fn provider_metadata_defaults_are_conservative_and_persisted() {
    let root = temp();
    let mut r = Registry::default();
    r.put(upstream("chat"), vec![Model {id:"unknown".into(),kind:"unknown".into(),..Default::default()}]);
    let row = &r.models["chat"][0];
    assert_eq!(row.endpoints, ["chat_completions"]);
    assert!(row.capabilities.contains(&"chat".into()));
    assert!(!row.capabilities.contains(&"tools".into()));
    assert!(!row.capabilities.contains(&"vision".into()));
    let mut decision = upstream("jev"); decision.protocol="typesafe".into();
    r.put(decision.clone(), vec![model("jev-latest")]);
    let row=&r.models["jev"][0];
    assert_eq!(row.endpoints,["systemone"]);
    assert_eq!(row.capabilities,["decision"]);
    assert!(registry::supports(&decision,row,"/systemone"));
    assert!(!registry::supports(&decision,row,"/chat/completions"));
    for cap in ["embedding","rerank","image_generation","asr","decision"] {
        let mut row=Model{id:cap.into(),kind:"unknown".into(),capabilities:vec![cap.into()],..Default::default()};
        registry::normalize_upstream_model(&mut row,&upstream("specialized"));
        assert!(!row.capabilities.contains(&"chat".into()),"{cap}");
        assert_ne!(row.endpoints,["chat_completions"]);
    }
    r.persist(&root.join("state.json")).unwrap();
    // Simulate a model saved by older versions with no discoverable metadata.
    let mut disk:Value=serde_json::from_slice(&std::fs::read(root.join("state.json")).unwrap()).unwrap();
    disk["models"]["chat"][0]["kind"]=json!("unknown");
    disk["models"]["chat"][0]["endpoints"]=json!([]);
    disk["models"]["chat"][0]["capabilities"]=json!([]);
    std::fs::write(root.join("state.json"),serde_json::to_vec(&disk).unwrap()).unwrap();
    let restored=Registry::open(&root.join("state.json")).unwrap();
    assert_eq!(restored.models["chat"][0].endpoints,["chat_completions"]);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn typesafe_native_catalog_and_proxy_preserve_typed_contract_and_credentials() {
    let root=temp();
    let server=Router::new()
        .route("/v1/models",axum::routing::get(|headers:HeaderMap|async move {
            assert_eq!(headers["authorization"],"Bearer fixture-typesafe-key");
            Json(json!({"models":[{"name":"jev-latest","description":"System One","release_date":"2026-09-15"}]}))
        }))
        .route("/v1/systemone",post(|headers:HeaderMap,Json(body):Json<Value>|async move {
            assert_eq!(headers["authorization"],"Bearer fixture-typesafe-key");
            assert_eq!(headers["typesafe-danger-zone-images"],"true");
            assert_eq!(body["model"],"jev-latest");
            assert_eq!(body["state"],json!({"ticket":"charged twice"}));
            assert_eq!(body["questions"]["billing"]["type"],"noul");
            assert!(body.get("messages").is_none());
            Json(json!({"model":"jev-latest","answers":{"billing":{"type":"noul","noul":0.98}},"usage":{"input_tokens":12,"output_tokens":3}}))
        }));
    let listener=tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move {axum::serve(listener,server).await.unwrap();});
    let g=gateway(&root,Router::new());
    let mut u=manage::normalize(&json!({"name":"TypeSafe","kind":"cloud","protocol":"typesafe","base_url":format!("http://{address}/v1"),"api_key":"fixture-typesafe-key","visibility":"private"})).unwrap();
    u.id="typesafe".into();u.model_prefix="ai.typesafe.api".into();u.owner="alice".into();u.healthy=true;u.source="api".into();
    let rows=manage::models(&g,&u).await.unwrap();
    assert_eq!(rows[0].id,"jev-latest");assert_eq!(rows[0].endpoints,["systemone"]);
    g.registry.lock().unwrap().put(u,rows);
    let request=||axum::http::Request::builder().method("POST").uri("/v1/systemone").header("content-type","application/json").header("authorization","Bearer caller-secret").header("typesafe-danger-zone-images","true")
        .body(Body::from(json!({"model":"ai.typesafe.api/jev-latest","state":{"ticket":"charged twice"},"questions":{"billing":{"type":"noul","instructions":"Is this about billing?"}}}).to_string())).unwrap();
    let response=proxy::handle(g.clone(),principal("alice"),request()).await.unwrap();
    assert_eq!(response.status(),StatusCode::OK);
    let body=axum::body::to_bytes(response.into_body(),65536).await.unwrap();
    let result:Value=serde_json::from_slice(&body).unwrap();
    assert_eq!(result["answers"]["billing"]["noul"],0.98);
    assert_eq!(result["usage"]["input_tokens"],12);
    assert!(!String::from_utf8_lossy(&body).contains("fixture-typesafe-key"));
    assert!(proxy::handle(g,principal("bob"),request()).await.is_err());
    task.abort();std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn ollama_capabilities_are_discovered_from_server_and_manual_overrides_survive_refresh() {
    let root=temp();
    let server=Router::new()
        .route("/v1/models",axum::routing::get(||async {Json(json!({"data":[{"id":"plain-name"},{"id":"not-obviously-an-embedding"},{"id":"old-vision"},{"id":"native-decision"},{"id":"transient-show-error"}]}))}))
        .route("/api/version",axum::routing::get(||async {Json(json!({"version":"fixture"}))}))
        .route("/api/show",post(|headers:HeaderMap,Json(body):Json<Value>|async move {
            assert_eq!(headers["authorization"],"Bearer fixture-ollama-key");
            if body["model"] == "transient-show-error" {
                return (StatusCode::BAD_GATEWAY, Json(json!({"error":"temporary"})));
            }
            (StatusCode::OK, Json(match body["model"].as_str().unwrap() {
                "plain-name"=>json!({"capabilities":["completion","vision","tools","thinking"]}),
                "not-obviously-an-embedding"=>json!({"capabilities":["embedding"]}),
                "old-vision"=>json!({"projector_info":{"general.architecture":"clip"}}),
                "native-decision"=>json!({"capabilities":["decision"]}),
                _=>panic!("unexpected model"),
            }))
        }));
    let server = server.route("/v1/systemone",post(|Json(body):Json<Value>|async move {
        assert_eq!(body["model"],"native-decision");
        assert_eq!(body["questions"]["ready"]["type"],"noul");
        Json(json!({"answers":{"ready":{"type":"noul","noul":0.9}}}))
    }));
    let listener=tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move {axum::serve(listener,server).await.unwrap();});
    let g=gateway(&root,Router::new());
    let mut u=upstream("ollama");u.base_url=format!("http://{address}/v1");u.owner="alice".into();u.api_key="fixture-ollama-key".into();
    g.registry.lock().unwrap().put(u.clone(), vec![Model {
        id:"transient-show-error".into(),capabilities:vec!["vision".into()],..Default::default()
    }]);
    // Existing OpenAI-compatible entries are recognized without re-creating them.
    let rows=manage::models(&g,&u).await.unwrap();
    assert!(rows[0].capabilities.iter().any(|c|c=="vision"));
    assert!(rows[0].capabilities.iter().any(|c|c=="tools"));
    assert!(rows[0].capabilities.iter().any(|c|c=="thinking"));
    assert_eq!(rows[1].endpoints,["embeddings"]);
    assert!(!rows[1].capabilities.iter().any(|c|c=="chat"));
    assert!(rows[2].capabilities.iter().any(|c|c=="vision"));
    assert!(rows[4].capabilities.contains(&"vision".into()), "temporary metadata errors must preserve prior capabilities");
    assert_eq!(rows[3].endpoints,["systemone"]);
    assert!(registry::supports(&u,&rows[3],"/systemone"));
    assert!(!registry::supports(&u,&rows[3],"/chat/completions"));
    g.registry.lock().unwrap().put(u,rows);
    let inference = axum::http::Request::builder().method("POST").uri("/v1/systemone")
        .header("content-type","application/json")
        .body(Body::from(json!({"model":"manual/native-decision","state":"Ready","questions":{"ready":{"type":"noul","instructions":"Ready?"}}}).to_string())).unwrap();
    let response = proxy::handle(g.clone(),principal("alice"),inference).await.unwrap();
    assert_eq!(response.status(),StatusCode::OK);
    let result:Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(),65536).await.unwrap()).unwrap();
    assert_eq!(result["answers"]["ready"]["noul"],0.9);
    let req=|method:&str,path:&str,value:Value|axum::http::Request::builder().method(method).uri(path).header("content-type","application/json").body(Body::from(value.to_string())).unwrap();
    let _=manage::handle(&g,&principal("alice"),req("PATCH","/api/v1/models",json!({"model":"manual/plain-name","upstream_id":"ollama","capabilities":["completion"],"endpoints":["chat_completions"]}))).await.unwrap();
    let _=manage::handle(&g,&principal("alice"),req("POST","/api/v1/upstreams/ollama/refresh",json!({}))).await.unwrap();
    assert!(!g.registry.lock().unwrap().models["ollama"][0].capabilities.iter().any(|c|c=="vision"));
    let _=manage::handle(&g,&principal("alice"),req("PATCH","/api/v1/models",json!({"model":"manual/plain-name","upstream_id":"ollama","metadata_override":false}))).await.unwrap();
    let _=manage::handle(&g,&principal("alice"),req("POST","/api/v1/upstreams/ollama/refresh",json!({}))).await.unwrap();
    assert!(g.registry.lock().unwrap().models["ollama"][0].capabilities.iter().any(|c|c=="vision"));
    // Explicit Ollama protocol has the same discovery result and chat bridges.
    let mut u=g.registry.lock().unwrap().upstreams["ollama"].clone();u.protocol="ollama".into();
    let rows=manage::models(&g,&u).await.unwrap();assert!(registry::supports(&u,&rows[0],"/responses"));
    let registration = json!({"service_id":"auto-ollama","protocol":"ollama","capabilities":[],"endpoints":[],"base_url":format!("http://{address}/v1"),"api_key":"fixture-ollama-key","models":[{"id":"plain-name"},{"id":"not-obviously-an-embedding"}],"ttl_seconds":90});
    let _ = manage::handle(&g,&principal("alice"),req("POST","/api/v1/registrations",registration.clone())).await.unwrap();
    {
        let r=g.registry.lock().unwrap();
        let rows=&r.models["registered-auto-ollama"];
        assert!(rows[0].capabilities.contains(&"vision".into()));
        assert!(rows[0].capabilities.contains(&"thinking".into()));
        assert_eq!(rows[1].endpoints,["embeddings"]);
    }
    let mut corrected=registration;
    corrected["model_capabilities"]=json!({"plain-name":["rerank"]});
    let _ = manage::handle(&g,&principal("alice"),req("POST","/api/v1/registrations",corrected)).await.unwrap();
    {
        let r=g.registry.lock().unwrap();
        let rows=&r.models["registered-auto-ollama"];
        assert_eq!(rows[0].endpoints,["rerank"]);
        assert!(!rows[0].capabilities.contains(&"chat".into()));
        assert_eq!(rows[1].endpoints,["embeddings"]);
    }
    task.abort();std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn registration_api_infers_chat_and_specialized_native_contracts() {
    let root = temp();
    let g = gateway(&root, Router::new());
    for (service, protocol, caps, endpoint) in [
        ("chat", "chat_completions", vec![], "chat_completions"),
        ("rerank", "openai", vec!["rerank"], "rerank"),
        ("jev", "typesafe", vec![], "systemone"),
    ] {
        let body = json!({"service_id":service,"protocol":protocol,"base_url":"http://fixture/v1","model":"sample","capabilities":caps,"visibility":"private"});
        let request = axum::http::Request::builder().method("POST").uri("/api/v1/registrations")
            .header("content-type","application/json").body(Body::from(body.to_string())).unwrap();
        let _ = manage::handle(&g,&principal("alice"),request).await.unwrap();
        let r = g.registry.lock().unwrap();
        let row = &r.models[&format!("registered-{service}")][0];
        assert_eq!(row.endpoints,[endpoint]);
        assert_eq!(row.capabilities.contains(&"chat".into()),service=="chat");
        assert!(!row.capabilities.contains(&"vision".into()));
        assert!(!row.capabilities.contains(&"tools".into()));
        assert!(!r.upstreams[&format!("registered-{service}")].visible(&principal("bob")));
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
#[ignore = "requires SCHEDULER_TEST_OLLAMA_URL for a real Ollama service; reads metadata only"]
async fn live_ollama_metadata_matches_server_capabilities() {
    let root = temp();
    let base = std::env::var("SCHEDULER_TEST_OLLAMA_URL").unwrap();
    let base = base.trim_end_matches('/');
    let g = gateway(&root, Router::new());
    let mut u = upstream("live-ollama");
    u.protocol = "ollama".into();
    u.base_url = format!("{base}/v1");
    let rows = manage::models(&g, &u).await.unwrap();
    assert!(!rows.is_empty(), "live service must have installed models");
    for row in rows {
        let value: Value = g.client.post(format!("{base}/api/show"))
            .json(&json!({"model":row.id})).send().await.unwrap().json().await.unwrap();
        for cap in strings(&value["capabilities"]) {
            assert!(row.capabilities.contains(&cap), "{} lost {cap}", row.id);
        }
        if value["capabilities"].as_array().is_some_and(|caps| caps.contains(&json!("embedding"))) {
            assert!(row.endpoints.contains(&"embeddings".into()));
            assert!(!row.capabilities.contains(&"chat".into()));
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

pub(super) fn temp() -> PathBuf {
    let path = std::env::temp_dir().join(format!("v-scheduler-router-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}
pub(super) fn gateway(root: &Path, controller: Router) -> Arc<Gateway> {
    Arc::new(Gateway {
        registry: Mutex::new(Registry::default()),
        auto_runtime: Default::default(),
        queue: Mutex::new(Default::default()),
        residency: residency::Residency::disabled(),
        state_path: root.join("state.json"),
        controller: RwLock::new(controller),
        client: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        identities: Mutex::new(Default::default()),
        usage: Mutex::new(usage::Usage::open(&root.join("usage.jsonl")).unwrap()),
        slots: Arc::new(tokio::sync::Semaphore::new(8)),
        buffered: Arc::new(tokio::sync::Semaphore::new(65536)),
        jobs_lock: Mutex::new(()),
        ui: None,
        initial_discovery: Mutex::new(Default::default()),
    })
}
#[test]
fn private_models_and_pinned_calls_respect_owner() {
    let mut r = Registry::default();
    let mut private = upstream("secret");
    private.owner = "alice".into();
    private.visibility = "private".into();
    r.put(private, vec![model("echo")]);
    assert!(r.list_models(&principal("bob")).is_empty());
    assert_eq!(r.list_models(&principal("alice")).len(), 1);
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &principal("bob"))
            .err()
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        r.acquire("echo", "/chat/completions", "secret", &principal("bob"))
            .err()
            .unwrap()
            .status,
        404
    );
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &principal("alice"))
            .unwrap()
            .0
            .id,
        "secret"
    );
}
#[test]
fn weighted_routing_reserves_capacity_and_never_uses_an_unhealthy_replica() {
    let mut r = Registry::default();
    for id in ["a", "b"] {
        r.put(upstream(id), vec![model("echo")]);
    }
    r.routing.insert(
        "a".into(),
        registry::Routing {
            weight: 3,
            max_inflight: 1,
            disabled: false,
            direct: false,
        },
    );
    r.routing.insert(
        "b".into(),
        registry::Routing {
            weight: 1,
            max_inflight: 1,
            disabled: false,
            direct: false,
        },
    );
    let id = principal("alice");
    let mut tally = BTreeMap::<String, usize>::new();
    for _ in 0..40 {
        let chosen = r
            .acquire("manual/echo", "/chat/completions", "", &id)
            .unwrap()
            .0
            .id;
        *tally.entry(chosen.clone()).or_default() += 1;
        *r.inflight.get_mut(&chosen).unwrap() -= 1;
    }
    assert_eq!(tally["a"], 30);
    assert_eq!(tally["b"], 10);
    let a = r
        .acquire("manual/echo", "/chat/completions", "", &id)
        .unwrap()
        .0
        .id;
    let b = r
        .acquire("manual/echo", "/chat/completions", "", &id)
        .unwrap()
        .0
        .id;
    assert_ne!(a, b);
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &id)
            .err()
            .unwrap()
            .status,
        429
    );
    r.inflight.clear();
    r.upstreams.get_mut("a").unwrap().healthy = false;
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &id)
            .unwrap()
            .0
            .id,
        "b"
    );
}
#[test]
fn circuit_breaker_recovers_after_cooldown_instead_of_permanently_rejecting() {
    let mut r = Registry::default();
    r.put(upstream("a"), vec![model("echo")]);
    for _ in 0..3 {
        r.result("a", false);
    }
    assert_eq!(r.status(&r.upstreams["a"])["state"], "circuit_open");
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &principal("u"))
            .err()
            .unwrap()
            .status,
        503
    );
    r.breaker.get_mut("a").unwrap().1 = Utc::now() - chrono::Duration::seconds(1);
    assert!(
        r.acquire("manual/echo", "/chat/completions", "", &principal("u"))
            .is_ok()
    );
    r.result("a", true);
    assert_eq!(r.status(&r.upstreams["a"])["state"], "healthy");
}
#[test]
fn direct_routing_bypasses_capacity_and_circuit_breaker_but_respects_pause() {
    let mut r = Registry::default();
    r.put(upstream("direct"), vec![model("echo")]);
    r.put(upstream("scheduled"), vec![model("echo")]);
    r.routing.insert(
        "direct".into(),
        registry::Routing {
            weight: 1,
            max_inflight: 1,
            disabled: false,
            direct: true,
        },
    );
    r.routing.insert(
        "scheduled".into(),
        registry::Routing {
            weight: 1,
            max_inflight: 1,
            disabled: false,
            direct: false,
        },
    );
    for _ in 0..3 {
        r.result("direct", false);
        r.result("scheduled", false);
    }
    assert_eq!(r.status(&r.upstreams["direct"])["state"], "healthy");
    assert_eq!(r.status(&r.upstreams["scheduled"])["state"], "circuit_open");
    r.inflight.insert("direct".into(), 9);
    let id = principal("alice");
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &id)
            .unwrap()
            .0
            .id,
        "direct"
    );
    r.routing.get_mut("direct").unwrap().disabled = true;
    r.breaker.remove("scheduled");
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &id)
            .unwrap()
            .0
            .id,
        "scheduled"
    );
    r.inflight.clear();
    r.routing.get_mut("direct").unwrap().disabled = false;
    assert_eq!(
        r.acquire("manual/echo", "/chat/completions", "", &id)
            .unwrap()
            .0
            .id,
        "direct"
    );
}
#[test]
fn direct_flag_defaults_off_for_existing_routing_state() {
    let routing: registry::Routing =
        serde_json::from_value(json!({"weight":2,"max_inflight":4})).unwrap();
    assert!(!routing.direct);
    assert!(!routing.disabled);
}
#[test]
fn selected_models_keep_probe_metadata_and_allow_empty_selection() {
    let probed = vec![
        Model {
            id: "a".into(),
            kind: "chat".into(),
            capabilities: vec!["tools".into()],
            endpoints: vec!["chat_completions".into()],
            ..Default::default()
        },
        Model {
            id: "b".into(),
            kind: "unknown".into(),
            ..Default::default()
        },
    ];
    let picked = manage::select_models(&json!({"models": ["a"]}), probed.clone()).unwrap();
    assert_eq!(picked.len(), 1);
    assert_eq!(picked[0].id, "a");
    assert_eq!(picked[0].capabilities, ["tools"]);
    assert_eq!(
        manage::select_models(&json!({}), probed.clone())
            .unwrap()
            .len(),
        2
    );
    assert!(
        manage::select_models(&json!({"models": []}), probed)
            .unwrap()
            .is_empty()
    );
    let declared = manage::select_models(
        &json!({"models": [{"id":"c","kind":"embedding"}]}),
        vec![],
    )
    .unwrap();
    assert_eq!(declared[0].kind, "embedding");
}
#[test]
fn old_state_roundtrips_credentials_routing_models_and_resource_ownership() {
    let root = temp();
    let path = root.join("state.json");
    let mut r = Registry::default();
    let mut u = upstream("old");
    u.owner = "alice".into();
    u.api_key = "private-provider-key".into();
    r.put(u, vec![model("echo")]);
    r.routing.insert(
        "old".into(),
        registry::Routing {
            weight: 9,
            ..Default::default()
        },
    );
    r.resources.push(json!({"upstream":"old","family":"files","resource_id":"file-1","owner":"alice","created_at":Utc::now()}));
    r.persist(&path).unwrap();
    let restored = Registry::open(&path).unwrap();
    assert_eq!(restored.upstreams["old"].api_key, "private-provider-key");
    assert_eq!(restored.routing["old"].weight, 9);
    assert_eq!(restored.models["old"][0].qualified, "manual/echo");
    assert_eq!(restored.resources[0]["resource_id"], "file-1");
    use std::os::unix::fs::PermissionsExt;
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn go_zero_times_and_null_collections_remain_permanent_and_readable() {
    let root = temp();
    let path = root.join("state.json");
    std::fs::write(&path,serde_json::to_vec(&json!({"version":2,"upstreams":[{"id":"old","source":"api","kind":"manual","base_url":"http://fixture/v1","healthy":true,"created_at":"0001-01-01T00:00:00Z","expires_at":"0001-01-01T00:00:00Z"}],"models":{"old":[{"id":"echo","qualified":"manual/echo","upstream_id":"old","capabilities":null,"endpoints":null}]},"resources":null,"routing":null})).unwrap()).unwrap();
    let registry = Registry::open(&path).unwrap();
    assert!(registry.upstreams["old"].live());
    assert_eq!(registry.list_models(&principal("u")).len(), 1);
    std::fs::write(
        &path,
        b"{\"version\":2,\"upstreams\":null,\"models\":{},\"routing\":{}}",
    )
    .unwrap();
    assert!(Registry::open(&path).unwrap().upstreams.is_empty());
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn terminal_tool_events_keep_output_indexes_and_ids_consistent() {
    let stream = concat!(
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":4,\"id\":\"call-1\",\"function\":{\"name\":\"weather\",\"arguments\":\"{\\\"city\\\":\"}}]}}]}\n\n",
        "data: {\"choices\":[{\"index\":0,\"delta\":{\"tool_calls\":[{\"index\":4,\"function\":{\"arguments\":\"\\\"Paris\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\n",
        "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":5,\"completion_tokens\":2}}\n\ndata: [DONE]\n\n"
    );
    let mut decoder = bridge::Stream::new("responses", "native");
    let bytes = decoder.feed(stream.as_bytes()).unwrap();
    let output = String::from_utf8(bytes).unwrap();
    let events = output
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|s| serde_json::from_str::<Value>(s).unwrap())
        .collect::<Vec<_>>();
    let added = events
        .iter()
        .find(|v| v["type"] == "response.output_item.added")
        .unwrap();
    let done = events
        .iter()
        .find(|v| v["type"] == "response.output_item.done")
        .unwrap();
    let completed = events
        .iter()
        .find(|v| v["type"] == "response.completed")
        .unwrap();
    assert_eq!(added["output_index"], 0);
    assert_eq!(done["output_index"], 0);
    assert_eq!(added["item"]["id"], done["item"]["id"]);
    assert_eq!(completed["response"]["output"][0]["id"], done["item"]["id"]);
    assert_eq!(completed["response"]["usage"]["input_tokens"], 5);
    assert_eq!(
        completed["response"]["output"][0]["arguments"],
        "{\"city\":\"Paris\"}"
    );
    assert!(decoder.terminal);
    let mut broken = bridge::Stream::new("messages", "native");
    let output = String::from_utf8(broken.feed(b"data: {invalid}\n\n").unwrap()).unwrap();
    assert!(output.contains("event: error"));
    assert!(!output.contains("message_stop"));
    assert!(broken.failed);
}
#[test]
fn valid_finish_without_done_completes_but_truncated_conversion_never_reports_success() {
    for protocol in ["messages", "responses"] {
        let mut decoder = bridge::Stream::new(protocol, "native");
        decoder.feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"ok\"},\"finish_reason\":\"stop\"}]}\n\n").unwrap();
        let output = String::from_utf8(decoder.eof()).unwrap();
        assert!(decoder.terminal);
        assert!(output.contains(if protocol == "messages" {
            "message_stop"
        } else {
            "response.completed"
        }));
        let mut truncated = bridge::Stream::new(protocol, "native");
        truncated
            .feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
            .unwrap();
        let output = String::from_utf8(truncated.eof()).unwrap();
        assert!(output.contains("event: error"));
        assert!(!output.contains("message_stop"));
        assert!(!output.contains("response.completed"));
    }
}
#[test]
fn legacy_usage_history_is_preserved_and_new_records_survive_restart() {
    let root = temp();
    let path = root.join("history");
    let original=b"{\"time\":\"2026-10-15T10:00:00Z\",\"user\":\"alice\",\"model\":\"manual/echo\",\"native_id\":\"echo\",\"upstream_id\":\"u\",\"status\":200}\n";
    std::fs::write(&path, original).unwrap();
    let mut log = usage::Usage::open(&path).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(
        std::fs::read(root.join("history.d/legacy.jsonl")).unwrap(),
        original
    );
    log.record(json!({"time":Utc::now(),"user":"bob","model":"manual/echo","native_id":"echo","upstream_id":"u","status":503}));
    drop(log);
    let restored = usage::Usage::open(&path).unwrap();
    let summary = restored.snapshot("period=all");
    assert_eq!(summary["totals"]["requests"], 2);
    assert_eq!(summary["totals"]["errors"], 1);
    assert_eq!(summary["per_user"]["alice"]["requests"], 1);
    assert_eq!(summary["per_user"]["bob"]["requests"], 1);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn business_services_are_visible_in_management_without_becoming_models() {
    let root = temp();
    let g = gateway(&root, Router::new());
    {
        let mut registry = g.registry.lock().unwrap();
        registry
            .services
            .insert("video-structure-detect".into(), "http://detect".into());
        registry.services.insert(
            "video-structure-structure".into(),
            "http://structure".into(),
        );
    }
    for (path, field, count) in [
        ("/api/v1/upstreams", "upstreams", 2),
        ("/api/v1/models", "models", 9),
    ] {
        let request = axum::http::Request::builder()
            .uri(path)
            .body(Body::empty())
            .unwrap();
        let response = manage::handle(&g, &principal("alice"), request)
            .await
            .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value[field].as_array().unwrap().len(), count);
    }
    assert!(
        g.registry
            .lock()
            .unwrap()
            .list_models(&principal("alice"))
            .is_empty()
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn backend_roles_and_declared_endpoints_are_authoritative() {
    let mut u = upstream("local");
    u.source = "v-scheduler".into();
    u.role = "embedding".into();
    let m = Model {
        kind: "embedding".into(),
        capabilities: vec!["embedding".into()],
        ..model("e")
    };
    assert!(registry::supports(&u, &m, "/embeddings"));
    assert!(!registry::supports(&u, &m, "/chat/completions"));
    assert!(!registry::supports(&u, &m, "/rerank"));
    u.role = "rerank".into();
    assert!(!registry::supports(&u, &m, "/rerank"));
    assert!(!registry::supports(&u, &m, "/embeddings"));
    let rerank = Model {
        capabilities: vec!["rerank".into()],
        ..model("r")
    };
    assert!(registry::supports(&u, &rerank, "/rerank"));
    u = upstream("api");
    let m = Model {
        endpoints: vec!["chat_completions".into()],
        ..model("chat")
    };
    assert!(registry::supports(&u, &m, "/responses"));
    assert!(!registry::supports(&u, &m, "/audio/speech"));
}
#[test]
fn multipart_model_rewrite_preserves_binary_and_repeated_fields() {
    let original=b"--b\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\nmanual/native\r\n--b\r\nContent-Disposition: form-data; name=\"file\"; filename=\"audio.wav\"\r\n\r\n\0\xffbinary\r\n--b\r\nContent-Disposition: form-data; name=\"hotwords_set\"\r\n\r\none\r\n--b\r\nContent-Disposition: form-data; name=\"hotwords_set\"\r\n\r\ntwo\r\n--b--\r\n";
    let parts = proxy::multipart(original, "multipart/form-data; boundary=b").unwrap();
    assert_eq!(parts.len(), 4);
    assert_eq!(&original[parts[1].start..parts[1].end], b"\0\xffbinary");
    assert_eq!(parts.iter().filter(|p| p.name == "hotwords_set").count(), 2);
}
#[test]
fn responses_and_messages_convert_tools_and_images_without_silently_discarding_features() {
    let response = json!({"model":"a","instructions":"system","input":[{"role":"user","content":[{"type":"input_text","text":"hello"},{"type":"input_image","image_url":"https://fixture/image.png"}]},{"type":"function_call","call_id":"call-1","name":"search","arguments":"{}"},{"type":"function_call_output","call_id":"call-1","output":"result"}],"tools":[{"type":"function","name":"search","parameters":{"type":"object"}}],"max_output_tokens":8,"store":false});
    let converted = bridge::request(&response, "responses").unwrap();
    assert_eq!(converted["messages"][0]["role"], "system");
    assert_eq!(
        converted["messages"][1]["content"][1]["image_url"]["url"],
        "https://fixture/image.png"
    );
    assert_eq!(converted["messages"][3]["tool_call_id"], "call-1");
    assert_eq!(converted["max_tokens"], 8);
    for (key, value) in [
        ("store", json!(true)),
        ("previous_response_id", json!("previous")),
        ("background", json!(true)),
        ("unknown_feature", json!(1)),
    ] {
        let mut bad = response.clone();
        bad[key] = value;
        assert_eq!(
            bridge::request(&bad, "responses").err().unwrap().status,
            400
        );
    }
    let messages = json!({"model":"a","max_tokens":8,"messages":[{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"search","input":{"q":"hello"}}]},{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"found"}]}]});
    let converted = bridge::request(&messages, "messages").unwrap();
    assert_eq!(converted["messages"][0]["tool_calls"][0]["id"], "t1");
    assert_eq!(converted["messages"][1]["tool_call_id"], "t1");
}
#[test]
fn sse_parser_handles_split_utf8_and_distinguishes_eof_from_terminal() {
    let payload="data: {\"choices\":[{\"delta\":{\"content\":\"你好\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".as_bytes();
    for protocol in ["", "messages", "responses"] {
        let mut stream = bridge::Stream::new(protocol, "native");
        let mut result = vec![];
        for b in payload {
            result.extend(stream.feed(&[*b]).unwrap());
        }
        assert!(stream.terminal);
        let result = String::from_utf8(result).unwrap();
        assert!(result.contains("你好"));
        if protocol == "messages" {
            assert!(result.contains("message_stop"));
        }
        if protocol == "responses" {
            assert!(result.contains("response.completed"));
        }
    }
    let mut truncated = bridge::Stream::new("", "a");
    truncated
        .feed(b"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n")
        .unwrap();
    assert!(!truncated.terminal);
}
#[test]
fn model_limits_reject_unapproved_parameters_dimensions_and_secondary_models() {
    let approval = lease::Approval {
        config: json!({}),
        policy: json!({"max_input_bytes":4096,"limits":{"max_tokens":8,"width":512,"height":512},"secondary_models":["speaker-a"]}),
    };
    let u = upstream("m");
    for mut input in [
        json!({"max_tokens":9}),
        json!({"max_tokens":-1}),
        json!({"n":2}),
        json!({"size":"1024x512"}),
        json!({"speaker_model":"speaker-b"}),
        json!({"max_tokens":"NaN"}),
    ] {
        assert_eq!(
            lease::validate(&approval, &u, &mut input, 20)
                .err()
                .unwrap()
                .status,
            422
        );
    }
    assert!(
        lease::validate(
            &approval,
            &u,
            &mut json!({"max_tokens":8,"size":"512x512","speaker_model":"speaker-a"}),
            20
        )
        .is_ok()
    );
    assert_eq!(
        lease::validate(&approval, &u, &mut json!({}), 5000)
            .err()
            .unwrap()
            .status,
        413
    );
}
#[tokio::test]
async fn rust_gateway_handles_native_chat_conversion_and_sse_against_a_real_http_upstream() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app=Router::new().route("/v1/chat/completions",post(|Json(v):Json<Value>|async move{
        if v["stream"]==true{(StatusCode::OK,[("content-type","text/event-stream")],"data: {\"choices\":[{\"delta\":{\"content\":\"rust echo\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").into_response()}
        else{Json(json!({"id":"fixture","created":0,"model":v["model"],"choices":[{"message":{"role":"assistant","content":"rust echo"},"finish_reason":"stop"}],"usage":{"prompt_tokens":2,"completion_tokens":3,"total_tokens":5}})).into_response()}
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let root = temp();
    let g = gateway(&root, Router::new());
    let mut u = upstream("fixture");
    u.base_url = format!("http://127.0.0.1:{port}/v1");
    g.registry.lock().unwrap().put(u, vec![model("echo")]);
    for path in ["/v1/chat/completions", "/v1/responses", "/v1/messages"] {
        for stream in [false, true] {
            let body = if path.ends_with("responses") {
                json!({"model":"manual/echo","input":"hello","stream":stream})
            } else {
                json!({"model":"manual/echo","messages":[{"role":"user","content":"hello"}],"max_tokens":8,"stream":stream})
            };
            let request = axum::http::Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&body).unwrap()))
                .unwrap();
            let response = proxy::handle(g.clone(), principal("alice"), request)
                .await
                .unwrap();
            assert_eq!(response.status(), 200);
            let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
                .await
                .unwrap();
            assert!(String::from_utf8_lossy(&bytes).contains("rust echo"));
        }
    }
    assert_eq!(g.registry.lock().unwrap().inflight["fixture"], 0);
    assert_eq!(g.usage.lock().unwrap().len(), 6);
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn permits_are_released_on_terminal_sse_but_not_on_truncation_or_client_cancel() {
    let releases = Arc::new(Mutex::new(Vec::<Value>::new()));
    let recorded = releases.clone();
    let controller = Router::new()
        .route(
            "/api/v1/permits",
            post(|| async { Json(json!({"data":{"permit_id":uuid::Uuid::new_v4()}})) }),
        )
        .route(
            "/api/v1/permits/{id}/release",
            post(move |Json(v): Json<Value>| {
                let recorded = recorded.clone();
                async move {
                    recorded.lock().unwrap().push(v);
                    Json(json!({"data":{}}))
                }
            }),
        );
    let upstream_headers = Arc::new(Mutex::new(Vec::<HeaderMap>::new()));
    let captured = upstream_headers.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        axum::serve(listener,Router::new().route("/v1/chat/completions",post(move|headers:HeaderMap,Json(v):Json<Value>|{let captured=captured.clone();async move{
        captured.lock().unwrap().push(headers);
        let mode=text(&v,"mode");let payload=if mode=="terminal"{"data: {\"choices\":[{\"delta\":{\"content\":\"answer\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"}else{"data: {\"choices\":[{\"delta\":{\"content\":\"partial\"}}]}\n\n"};
        let first=futures_util::stream::iter([Ok::<_,std::io::Error>(Bytes::from(payload))]);let body=if mode=="truncated"{Body::from_stream(first)}else{Body::from_stream(first.chain(futures_util::stream::pending()))};
        let mut response=Response::new(body);response.headers_mut().insert("content-type","text/event-stream".parse().unwrap());response
    }}))).await.unwrap();
    });
    let root = temp();
    let g = gateway(&root, controller);
    let mut u = upstream("fixture");
    u.base_url = format!("http://127.0.0.1:{port}/v1");
    let cfg = json!({"url":"http://v-scheduler:38120/api/v1","token":"private-scoped-submitter-credential-1234567890","require_model_policy":true,"models":{"fixture/echo":{"mode":"permit","executor_id":"fake-approved-executor","completion":"synchronous","max_input_bytes":4096}}});
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("scheduler.local.json");
    std::fs::write(&path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    g.registry.lock().unwrap().put(u, vec![model("echo")]);
    for (mode, expected) in [
        ("terminal", true),
        ("truncated", false),
        ("cancelled", false),
    ] {
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .header("x-vscheduler-permit-id", "client-forged-permit")
            .header("x-vscheduler-execution-token", "client-forged-proof")
            .body(Body::from(
                serde_json::to_vec(&json!({"model":"manual/echo","stream":true,"mode":mode}))
                    .unwrap(),
            ))
            .unwrap();
        let response = proxy::handle(g.clone(), principal("alice"), request)
            .await
            .unwrap();
        let mut stream = response.into_body().into_data_stream();
        assert!(stream.next().await.unwrap().is_ok());
        if mode == "terminal" {
            for _ in 0..30 {
                if releases.lock().unwrap().len() >= 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            assert_eq!(
                releases.lock().unwrap().len(),
                1,
                "terminal must release before the upstream closes its connection"
            );
        }
        if mode == "truncated" {
            assert!(stream.next().await.is_none());
        }
        drop(stream);
        let expected_count = match mode {
            "terminal" => 1,
            "truncated" => 2,
            _ => 3,
        };
        for _ in 0..50 {
            if releases.lock().unwrap().len() >= expected_count {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert_eq!(
            releases.lock().unwrap().last().unwrap()["stopped"],
            expected
        );
        assert_eq!(g.registry.lock().unwrap().inflight["fixture"], 0);
    }
    for headers in upstream_headers.lock().unwrap().iter() {
        assert_ne!(headers["x-vscheduler-permit-id"], "client-forged-permit");
        assert_ne!(
            headers["x-vscheduler-execution-token"],
            "client-forged-proof"
        );
        assert_eq!(
            headers["x-vscheduler-executor-id"],
            "fake-approved-executor"
        );
    }
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn websocket_frames_close_codes_and_server_stop_evidence_survive_the_rust_gateway() {
    use futures_util::SinkExt;
    use tokio_tungstenite::tungstenite::{
        Message,
        protocol::{CloseFrame, frame::coding::CloseCode},
    };
    let releases = Arc::new(Mutex::new(Vec::<Value>::new()));
    let recorded = releases.clone();
    let controller = Router::new()
        .route(
            "/api/v1/permits",
            post(|| async { Json(json!({"data":{"permit_id":uuid::Uuid::new_v4()}})) }),
        )
        .route(
            "/api/v1/permits/{id}/release",
            post(move |Json(v): Json<Value>| {
                let recorded = recorded.clone();
                async move {
                    recorded.lock().unwrap().push(v);
                    Json(json!({"data":{}}))
                }
            }),
        );
    let native = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let native_port = native.local_addr().unwrap().port();
    let native_server = tokio::spawn(async move {
        let (stream, _) = native.accept().await.unwrap();
        let mut socket = tokio_tungstenite::accept_async(stream).await.unwrap();
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Binary(Bytes::from_static(b"audio"))
        );
        socket
            .send(Message::Text("native final".into()))
            .await
            .unwrap();
        socket
            .send(Message::Close(Some(CloseFrame {
                code: CloseCode::from(4422),
                reason: "native close detail".into(),
            })))
            .await
            .unwrap();
    });
    let root = temp();
    let g = gateway(&root, controller);
    let mut u = upstream("ws");
    u.protocol = "openai".into();
    u.base_url = format!("http://127.0.0.1:{native_port}/v1");
    g.registry.lock().unwrap().put(
        u,
        vec![Model {
            endpoints: vec!["audio/streaming/ws".into()],
            ..model("asr")
        }],
    );
    let config = json!({"url":"http://v-scheduler:38120/api/v1","token":"scoped-websocket-submitter-credential-1234567890","require_model_policy":true,"models":{"ws/asr":{"mode":"permit","executor_id":"approved-ws-executor","completion":"synchronous","websocket_completion":"server_close","max_input_bytes":1024}}});
    let path = root.join("scheduler.local.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = Router::new().fallback(super::handle).with_state(g.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let (mut client, _) = tokio_tungstenite::connect_async(format!(
        "ws://127.0.0.1:{port}/v1/audio/streaming/ws?model=manual%2Fasr"
    ))
    .await
    .unwrap();
    client
        .send(Message::Binary(Bytes::from_static(b"audio")))
        .await
        .unwrap();
    assert_eq!(
        client.next().await.unwrap().unwrap(),
        Message::Text("native final".into())
    );
    let Message::Close(Some(close)) = client.next().await.unwrap().unwrap() else {
        panic!("upstream close frame missing")
    };
    assert_eq!(u16::from(close.code), 4422);
    assert_eq!(close.reason, "native close detail");
    for _ in 0..50 {
        if !releases.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(releases.lock().unwrap()[0]["stopped"], true);
    assert_eq!(g.registry.lock().unwrap().inflight["ws"], 0);
    native_server.await.unwrap();
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn async_receipts_hold_admission_and_cache_consumable_results_across_restart() {
    let releases = Arc::new(Mutex::new(Vec::<Value>::new()));
    let recorded = releases.clone();
    let controller = Router::new()
        .route(
            "/api/v1/permits",
            post(|| async { Json(json!({"data":{"permit_id":uuid::Uuid::new_v4()}})) }),
        )
        .route(
            "/api/v1/permits/{id}/release",
            post(move |Json(v): Json<Value>| {
                let recorded = recorded.clone();
                async move {
                    recorded.lock().unwrap().push(v);
                    Json(json!({"data":{}}))
                }
            }),
        );
    let polls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let captured = polls.clone();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let native = Router::new()
        .route(
            "/api/v1/video/structure",
            post(|| async {
                (
                    StatusCode::ACCEPTED,
                    Json(json!({"data":{"task_id":"task-1","status":"queued"}})),
                )
            }),
        )
        .route(
            "/api/v1/video/structure/tasks/task-1",
            axum::routing::get(move || {
                let captured = captured.clone();
                async move {
                    assert_eq!(
                        captured.fetch_add(1, Ordering::SeqCst),
                        0,
                        "native task result is consumable only once"
                    );
                    Json(json!({"data":{"task_id":"task-1","status":"succeeded","result":"kept"}}))
                }
            }),
        );
    let server = tokio::spawn(async move {
        axum::serve(listener, native).await.unwrap();
    });
    let root = temp();
    let g = gateway(&root, controller);
    let mut u = upstream("ml");
    u.protocol = "ml_server".into();
    u.base_url = format!("http://127.0.0.1:{port}");
    g.registry.lock().unwrap().put(
        u.clone(),
        vec![Model {
            endpoints: vec!["ml/video/structure".into()],
            ..model("video")
        }],
    );
    let config = json!({"url":"http://v-scheduler:38120/api/v1","token":"scoped-async-submitter-credential-1234567890","require_model_policy":true,"models":{"ml/video":{"mode":"permit","executor_id":"approved-video-executor","completion":"asynchronous","max_input_bytes":4096}}});
    let path = root.join("scheduler.local.json");
    std::fs::write(&path, serde_json::to_vec(&config).unwrap()).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let request = axum::http::Request::builder()
        .method("POST")
        .uri("/v1/ml/video/structure")
        .header("content-type", "application/json")
        .body(Body::from(
            serde_json::to_vec(
                &json!({"model":"manual/video","video_url":"https://fixture/video.mp4"}),
            )
            .unwrap(),
        ))
        .unwrap();
    let response = proxy::handle(g.clone(), principal("alice"), request)
        .await
        .unwrap();
    assert_eq!(response.status(), 202);
    axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(
        releases.lock().unwrap().is_empty(),
        "HTTP 202 does not prove native computation stopped"
    );
    for _ in 0..80 {
        if !releases.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(releases.lock().unwrap()[0]["stopped"], true);
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    let restored = gateway(&root, Router::new());
    let url = format!("http://127.0.0.1:{port}/api/v1/video/structure/tasks/task-1");
    assert_eq!(
        jobs::cached(&restored, &principal("bob"), &u, &url)
            .err()
            .unwrap()
            .status,
        404
    );
    let cached = jobs::cached(&restored, &principal("alice"), &u, &url)
        .unwrap()
        .unwrap();
    let raw = axum::body::to_bytes(cached.into_body(), 1 << 20)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&raw).contains("kept"));
    assert_eq!(
        jobs::cached(&restored, &principal("alice"), &u, &url)
            .err()
            .unwrap()
            .status,
        404
    );
    assert_eq!(polls.load(Ordering::SeqCst), 1);
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn static_ui_loads_before_authentication_while_apis_stay_protected() {
    let root = temp();
    std::fs::write(root.join("index.html"), "<title>V-Scheduler</title>").unwrap();
    let mut g = gateway(&root, Router::new());
    Arc::get_mut(&mut g).unwrap().ui = Some(root.clone());
    let request = axum::http::Request::builder()
        .uri("/")
        .header("authorization", "Bearer expired")
        .body(Body::empty())
        .unwrap();
    let response = dispatch(g.clone(), request).await.unwrap();
    assert_eq!(response.status(), 200);
    let body = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert!(String::from_utf8_lossy(&body).contains("V-Scheduler"));
    let request = axum::http::Request::builder()
        .uri("/api/v1/status")
        .body(Body::empty())
        .unwrap();
    assert_eq!(dispatch(g, request).await.err().unwrap().status, 401);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn browser_origin_and_compression_headers_do_not_reach_the_native_provider() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let native=Router::new().route("/v1/chat/completions",post(|headers:HeaderMap,Json(v):Json<Value>|async move{
        // Ollama rejects browser origins outside its own CORS allowlist. The
        // browser belongs to this gateway, so this is a server-to-server hop.
        if headers.contains_key("origin"){return StatusCode::FORBIDDEN.into_response();}
        assert!(!headers.contains_key("accept-encoding"),"protocol bridges must receive uncompressed bodies");
        if v["stream"]==true{([( "content-type","text/event-stream")],"data: {\"choices\":[{\"delta\":{\"content\":\"browser reply\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").into_response()}
        else{Json(json!({"choices":[{"message":{"role":"assistant","content":"browser reply"},"finish_reason":"stop"}]})).into_response()}
    }));
    let server = tokio::spawn(async move {
        axum::serve(listener, native).await.unwrap();
    });
    let root = temp();
    let g = gateway(&root, Router::new());
    let mut u = upstream("browser");
    u.base_url = format!("http://127.0.0.1:{port}/v1");
    g.registry.lock().unwrap().put(u, vec![model("echo")]);
    for stream in [false, true] {
        let request=axum::http::Request::builder().method("POST").uri("/v1/chat/completions").header("content-type","application/json").header("origin","https://vos.example.test").header("accept-encoding","gzip, br").body(Body::from(serde_json::to_vec(&json!({"model":"manual/echo","messages":[{"role":"user","content":"hello"}],"stream":stream})).unwrap())).unwrap();
        let response = proxy::handle(g.clone(), principal("alice"), request)
            .await
            .unwrap();
        assert_eq!(
            response.status(),
            200,
            "browser headers must not cause a provider CORS rejection"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("browser reply"));
    }
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn admission_waits_for_native_stop_confirmation_before_sending_new_inference() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = attempts.clone();
    let controller = Router::new().route(
        "/api/v1/permits",
        post(move || {
            let attempt = seen.fetch_add(1, Ordering::SeqCst);
            async move {
                if attempt < 2 {
                    (
                        StatusCode::TOO_MANY_REQUESTS,
                        Json(json!({"code":"WAIT_STOP_CONFIRMATION"})),
                    )
                } else {
                    (
                        StatusCode::OK,
                        Json(json!({"code":"OK","data":{"permit_id":"confirmed-permit"}})),
                    )
                }
            }
        }),
    );
    let root = temp();
    let g = gateway(&root, controller);
    let approval = lease::Approval {
        config: json!({"token":"fixture-private-token"}),
        policy: json!({"mode":"permit","executor_id":"fixture"}),
    };
    let result = lease::acquire(
        g,
        &approval,
        &upstream("fixture"),
        "echo",
        &HeaderMap::new(),
        &principal("alice"),
        false,
    )
    .await;
    assert!(
        result.is_ok(),
        "a confirmed stop should recover within the same request"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 3);
    drop(result);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn admission_does_not_retry_capacity_denial_or_release_an_unknown_stop() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    for code in ["WAIT_RESOURCE", "WAIT_STOP_CONFIRMATION"] {
        let attempts = Arc::new(AtomicUsize::new(0));
        let seen = attempts.clone();
        let controller = Router::new().route(
            "/api/v1/permits",
            post(move || {
                seen.fetch_add(1, Ordering::SeqCst);
                async move { (StatusCode::TOO_MANY_REQUESTS, Json(json!({"code":code}))) }
            }),
        );
        let root = temp();
        let g = gateway(&root, controller);
        let error = lease::admit(
            &g,
            "fixture",
            "/permits",
            json!({}),
            Duration::from_millis(30),
        )
        .await
        .unwrap_err();
        assert_eq!(error.code.as_deref(), Some(code));
        if code == "WAIT_RESOURCE" {
            assert_eq!(attempts.load(Ordering::SeqCst), 1);
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[tokio::test]
async fn synchronous_response_waits_for_controller_release_before_next_request() {
    use std::sync::atomic::{AtomicBool, Ordering};
    let held = Arc::new(AtomicBool::new(false));
    let acquiring = held.clone();
    let releasing = held.clone();
    let controller = Router::new()
        .route("/api/v1/permits", post(move || {
            let busy = acquiring.swap(true, Ordering::SeqCst);
            async move {
                if busy {
                    (StatusCode::TOO_MANY_REQUESTS, Json(json!({"code":"WAIT_RESOURCE"})))
                } else {
                    (StatusCode::OK, Json(json!({"data":{"permit_id":"serial-permit"}})))
                }
            }
        }))
        .route("/api/v1/permits/{id}/release", post(move |Json(body): Json<Value>| {
            let releasing = releasing.clone();
            async move {
                assert_eq!(body["stopped"], true);
                tokio::time::sleep(Duration::from_millis(100)).await;
                releasing.store(false, Ordering::SeqCst);
                Json(json!({"data":{"state":"released"}}))
            }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        axum::serve(listener, Router::new().route("/v1/chat/completions", post(|Json(request):Json<Value>| async move {
            if request["stream"] == true {
                let mut response = Response::new(Body::from("data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n"));
                response.headers_mut().insert("content-type","text/event-stream".parse().unwrap());
                response
            } else {
                Json(json!({"model":"echo","choices":[{"message":{"role":"assistant","content":"hello"},"finish_reason":"stop"}]})).into_response()
            }
        }))).await.unwrap();
    });
    let root = temp();
    let g = gateway(&root, controller);
    let mut u = upstream("serial");
    u.base_url = format!("http://127.0.0.1:{port}/v1");
    let cfg = json!({"url":"http://v-scheduler:38120/api/v1","token":"private-scoped-submitter-credential-1234567890","require_model_policy":true,"models":{"serial/echo":{"mode":"permit","executor_id":"fake-approved-executor","completion":"synchronous","max_input_bytes":4096}}});
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("scheduler.local.json");
    std::fs::write(&path, serde_json::to_vec(&cfg).unwrap()).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    g.registry.lock().unwrap().put(u, vec![model("echo")]);
    for stream in [false,true,false] {
        let request = axum::http::Request::builder().method("POST")
            .uri("/v1/chat/completions").header("content-type","application/json")
            .body(Body::from(serde_json::to_vec(&json!({"model":"manual/echo","stream":stream,"messages":[{"role":"user","content":"hello"}]})).unwrap())).unwrap();
        let response = proxy::handle(g.clone(), principal("alice"), request).await.unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 1 << 20).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("hello"));
        assert!(!held.load(Ordering::SeqCst), "response escaped before controller released its permit");
    }
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn calibrated_ollama_defaults_are_enforced_before_native_execution() {
    let mut u = upstream("v-scheduler/ollama-qa");
    u.source = "v-scheduler".into();
    u.role = "generate".into();
    let approval = lease::Approval {
        config: json!({}),
        policy: json!({"executor_id":"builtin-ollama-calibrated-fixture","max_input_bytes":4096,"limits":{"max_tokens":128,"options.num_ctx":512,"options.num_thread":4,"options.num_gpu":0,"options.num_batch":128,"options.num_predict":128}}),
    };
    let mut request = json!({"messages":[{"role":"user","content":"hello"}]});
    lease::validate(&approval, &u, &mut request, 100).unwrap();
    assert_eq!(request["options"], json!({"num_ctx":512,"num_thread":4,"num_gpu":0,"num_batch":128,"num_predict":128}));
    assert_eq!(request["max_tokens"],128);
    let mut bounded = json!({"messages":[{"role":"user","content":"hello"}],"max_tokens":8});
    lease::validate(&approval, &u, &mut bounded, 100).unwrap();
    assert_eq!(bounded["options"]["num_predict"], 8, "the requested output must not expand to the calibrated maximum");
    for (key, value) in [("num_ctx",513),("num_thread",5),("num_gpu",1),("num_batch",129),("num_predict",129)] {
        let mut oversized = request.clone();
        oversized["options"][key]=json!(value);
        assert!(lease::validate(&approval,&u,&mut oversized,100).is_err(),"{key} bypassed the calibrated bound");
    }
    for key in ["num_ctx", "num_thread", "num_batch"] {
        let mut automatic = request.clone();
        automatic["options"][key] = json!(0);
        assert!(lease::validate(&approval,&u,&mut automatic,100).is_err(),"{key}=0 enabled unmeasured native defaults");
    }
    assert!(lease::validate(&approval,&u,&mut request,4097).is_err());
}

#[tokio::test]
async fn startup_catalog_is_retryable_instead_of_empty_or_missing() {
    let root = temp();
    let g = gateway(&root, Router::new());
    g.initial_discovery.lock().unwrap().insert("qa".into());
    let request = Request::builder().uri("/v1/models").body(Body::empty()).unwrap();
    let error = proxy::handle(g.clone(), principal("alice"), request).await.err();
    assert_eq!(error.map(|e| e.status), Some(StatusCode::SERVICE_UNAVAILABLE),
        "an initial discovery window must not look like a ready empty catalog");
    let request = Request::builder().method("POST").uri("/v1/chat/completions")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"model":"v-scheduler/ollama-qa/starting","messages":[{"role":"user","content":"hello"}]}"#)).unwrap();
    let error = proxy::handle(g.clone(), principal("alice"), request).await.err().unwrap();
    assert_eq!(error.status, StatusCode::SERVICE_UNAVAILABLE,
        "a model not yet discovered during startup must not be reported as missing");
    assert_eq!(error.code.as_deref(), Some("MODEL_CATALOG_INITIALIZING"));
    let response = error.into_response();
    assert_eq!(response.headers()["retry-after"], "1");
    g.initial_discovery.lock().unwrap().clear();
    let request = Request::builder().uri("/v1/models").body(Body::empty()).unwrap();
    assert_eq!(proxy::handle(g, principal("alice"), request).await.unwrap().status(), 200);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn local_startup_does_not_hide_discovered_cloud_models() {
    let root = temp();
    let g = gateway(&root, Router::new());
    g.initial_discovery.lock().unwrap().insert("qa".into());
    g.registry.lock().unwrap().put(upstream("cloud"), vec![model("echo")]);
    let request = Request::builder().uri("/v1/models").body(Body::empty()).unwrap();
    let response = proxy::handle(g.clone(), principal("alice"), request).await.unwrap();
    assert_eq!(response.status(), 200);
    let value: Value = serde_json::from_slice(&axum::body::to_bytes(response.into_body(), 65536).await.unwrap()).unwrap();
    assert_eq!(value["data"].as_array().unwrap().len(), 1);
    assert_eq!(readiness::response(&g).status(), 503);
    assert!(g.local_model_readiness("manual/echo").is_ok());
    g.initial_discovery.lock().unwrap().clear();
    assert_eq!(readiness::response(&g).status(), 200);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn failed_builtin_discovery_is_unavailable_but_unknown_healthy_model_is_not_found() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let mut u = upstream("v-scheduler/ollama-qa");
    u.healthy = false;
    g.registry.lock().unwrap().put(u.clone(), vec![]);
    assert_eq!(g.local_model_readiness("v-scheduler/ollama-qa/missing").unwrap_err().status, 503);
    u.healthy = true;
    g.registry.lock().unwrap().put(u, vec![]);
    assert!(g.local_model_readiness("v-scheduler/ollama-qa/missing").is_ok());
    assert_eq!(g.registry.lock().unwrap().acquire("v-scheduler/ollama-qa/missing", "/chat/completions", "", &principal("alice")).err().unwrap().status, 404);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn admission_deadline_covers_stalled_response_body() {
    let root = temp();
    let controller = Router::new().route("/api/v1/permits", post(|| async {
        Body::from_stream(futures_util::stream::pending::<Result<Bytes, std::io::Error>>())
    }));
    let g = gateway(&root, controller);
    let error = tokio::time::timeout(Duration::from_secs(4),
        lease::call(&g, "fixture", "/permits", json!({}), true)).await
        .expect("admission must not hang after response headers").unwrap_err();
    assert_eq!(error.code.as_deref(), Some("SCHEDULER_ADMISSION_TIMEOUT"));
    assert_eq!(error.status, 503);
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn queued_admission_prioritizes_interaction_and_cancels_without_execution() {
    use std::sync::atomic::AtomicBool;
    let busy = Arc::new(AtomicBool::new(true));
    let order = Arc::new(Mutex::new(Vec::<String>::new()));
    let acquiring = busy.clone();
    let acquired = order.clone();
    let releasing = busy.clone();
    let controller = Router::new()
        .route(
            "/api/v1/inference-waiters",
            post(|| async { Json(json!({"data":{"updated":true}})) }),
        )
        .route(
            "/api/v1/permits",
            post(move |Json(body): Json<Value>| {
                let busy = acquiring.clone();
                let order = acquired.clone();
                async move {
                    if busy.swap(true, Ordering::SeqCst) {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            Json(json!({"code":"WAIT_RESOURCE"})),
                        )
                    } else {
                        order.lock().unwrap().push(text(&body, "model_id").into());
                        (
                            StatusCode::OK,
                            Json(json!({"data":{"permit_id":uuid::Uuid::new_v4().to_string()}})),
                        )
                    }
                }
            }),
        )
        .route(
            "/api/v1/permits/{id}/release",
            post(move || {
                let busy = releasing.clone();
                async move {
                    busy.store(false, Ordering::SeqCst);
                    Json(json!({"data":{"state":"released"}}))
                }
            }),
        );
    let root = temp();
    let g = gateway(&root, controller);
    let approval = lease::Approval {
        config: json!({"token":"private-admission-test-credential-123456789"}),
        policy: json!({"mode":"permit","executor_id":"test"}),
    };
    let spawn = |name: &'static str, priority: &'static str, wait: &'static str| {
        let g = g.clone();
        let approval = approval.clone();
        tokio::spawn(async move {
            let mut headers = HeaderMap::new();
            headers.insert("x-vscheduler-queue-wait-seconds", wait.parse().unwrap());
            headers.insert("x-vscheduler-priority", priority.parse().unwrap());
            headers.insert("x-vscheduler-request-id", name.parse().unwrap());
            lease::acquire(
                g,
                &approval,
                &upstream("test"),
                name,
                &headers,
                &principal("alice"),
                false,
            )
            .await
        })
    };
    let background = spawn("background", "background", "5");
    tokio::time::sleep(Duration::from_millis(40)).await;
    let interactive = spawn("interactive", "interactive", "5");
    let cancelled = spawn("cancelled", "background", "5");
    tokio::time::sleep(Duration::from_millis(40)).await;
    let snapshot = g.queue.lock().unwrap().snapshot(&principal("alice"));
    assert_eq!(snapshot["items"][0]["request_id"], "interactive");
    assert!(
        g.queue.lock().unwrap().snapshot(&principal("bob"))["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    g.queue
        .lock()
        .unwrap()
        .cancel(&principal("alice"), "cancelled")
        .unwrap();
    assert_eq!(
        cancelled.await.unwrap().err().unwrap().code.as_deref(),
        Some("QUEUE_CANCELLED")
    );
    assert!(order.lock().unwrap().is_empty());
    busy.store(false, Ordering::SeqCst);
    let mut first = interactive.await.unwrap().unwrap();
    assert_eq!(*order.lock().unwrap(), vec!["interactive"]);
    first
        .release_confirmed("inference_request_not_started")
        .await;
    let mut second = background.await.unwrap().unwrap();
    second
        .release_confirmed("inference_request_not_started")
        .await;
    assert_eq!(*order.lock().unwrap(), vec!["interactive", "background"]);
    assert!(
        g.queue.lock().unwrap().snapshot(&principal("alice"))["items"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    busy.store(true, Ordering::SeqCst);
    let expired = spawn("timeout", "interactive", "1")
        .await
        .unwrap()
        .err()
        .unwrap();
    assert_eq!(expired.code.as_deref(), Some("QUEUE_TIMEOUT"));
    assert_eq!(
        order.lock().unwrap().len(),
        2,
        "timeout or cancellation started execution"
    );
    // A stop click may arrive before the inference POST has been authenticated.
    g.queue
        .lock()
        .unwrap()
        .cancel(&principal("alice"), "early-stop")
        .unwrap();
    let late = spawn("early-stop", "interactive", "5")
        .await
        .unwrap()
        .err()
        .unwrap();
    assert_eq!(late.code.as_deref(), Some("QUEUE_CANCELLED"));
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn api_model_metadata_is_editable_by_owner_and_persisted() {
    let root = temp();
    let g = gateway(&root, Router::new());
    {
        let mut registry = g.registry.lock().unwrap();
        let mut u = upstream("vendor");
        u.owner = "alice".into();
        registry.put(u, vec![model("echo")]);
    }
    let patch = |body: Value| {
        axum::http::Request::builder()
            .method("PATCH")
            .uri("/api/v1/models")
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    };
    let response = manage::handle(
        &g,
        &principal("alice"),
        patch(json!({
            "model": "manual/echo",
            "upstream_id": "vendor",
            "kind": "embedding",
            "capabilities": ["embedding"],
            "endpoints": ["embeddings"]
        })),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 1 << 20)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value["model"]["kind"], "embedding");
    assert_eq!(value["model"]["endpoints"], json!(["embeddings"]));
    assert_eq!(value["model"]["capabilities"], json!(["embedding"]));
    let disk: Value =
        serde_json::from_slice(&std::fs::read(root.join("state.json")).unwrap()).unwrap();
    assert_eq!(disk["models"]["vendor"][0]["kind"], "embedding");

    let denied = manage::handle(
        &g,
        &principal("bob"),
        patch(json!({"model": "manual/echo", "upstream_id": "vendor", "kind": "chat"})),
    )
    .await
    .err()
    .unwrap();
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn direct_upstream_bypasses_admission_for_all_models() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let u = {
        let mut registry = g.registry.lock().unwrap();
        let u = upstream("vendor");
        registry.put(u.clone(), vec![model("echo"), model("embed")]);
        registry.routing.insert(
            "vendor".into(),
            registry::Routing {
                direct: true,
                weight: 1,
                max_inflight: 0,
                disabled: false,
            },
        );
        u
    };
    for model in ["echo", "embed"] {
        assert!(lease::approval(&g, &u, model, "POST", "/chat/completions", false)
            .unwrap()
            .is_none());
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn editing_upstream_probe_reuses_saved_key_without_exposing_it() {
    let root = temp();
    let server = Router::new().route("/v1/models", axum::routing::get(|headers: HeaderMap| async move {
        if headers.get("authorization").and_then(|v| v.to_str().ok()) != Some("Bearer fixture-saved-key") {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        Json(json!({"data":[{"id":"echo"}]})).into_response()
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, server).await.unwrap(); });
    let g = gateway(&root, Router::new());
    let base = format!("http://{address}/v1");
    let mut u = upstream("saved");
    u.owner = "alice".into(); u.base_url = base.clone(); u.api_key = "fixture-saved-key".into();
    g.registry.lock().unwrap().put(u, vec![model("echo")]);
    let payload = json!({"upstream_id":"saved","name":"Edited","base_url":base,"api_key":"","protocol":"chat_completions"});
    let req = |path: &str, method: &str, value: Value| axum::http::Request::builder()
        .method(method).uri(path).header("content-type","application/json")
        .body(Body::from(value.to_string())).unwrap();
    let result = manage::handle(&g, &principal("alice"), req("/api/v1/upstreams/probe","POST",payload.clone())).await.unwrap();
    let body = axum::body::to_bytes(result.into_body(), 65536).await.unwrap();
    let report: Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(report["ok"], true, "editing probe must use the saved key");
    assert!(!String::from_utf8_lossy(&body).contains("fixture-saved-key"));
    let forbidden = manage::handle(&g, &principal("bob"), req("/api/v1/upstreams/probe","POST",payload.clone())).await.err().unwrap();
    assert_eq!(forbidden.status, StatusCode::FORBIDDEN);
    let mut changed = payload.clone(); changed["base_url"] = json!("http://other.invalid/v1");
    assert_eq!(manage::handle(&g, &principal("alice"), req("/api/v1/upstreams/probe","POST",changed)).await.err().unwrap().status, StatusCode::BAD_REQUEST);
    let saved = manage::handle(&g, &principal("alice"), req("/api/v1/upstreams/saved","PUT",payload)).await.unwrap();
    let view: Value = serde_json::from_slice(&axum::body::to_bytes(saved.into_body(),65536).await.unwrap()).unwrap();
    assert_eq!(view["api_key"], ""); assert_eq!(view["has_key"], true);
    assert_eq!(Registry::open(&root.join("state.json")).unwrap().upstreams["saved"].api_key, "fixture-saved-key");
    task.abort(); std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn per_model_direct_routing_accepts_slashes_in_upstream_and_native_ids() {
    let root = temp();
    let g = gateway(&root, Router::new());
    {
        let mut registry = g.registry.lock().unwrap();
        registry.put(upstream("v-scheduler/ollama-qa"), vec![model("qwen3.5:2b"), model("vendor/model:latest")]);
        registry.put(upstream("v-scheduler"), vec![model("unrelated")]);
    }
    let mut admin = principal("root");
    admin.admin = true;
    for native in ["qwen3.5:2b", "vendor/model:latest"] {
        let model_id = format!("v-scheduler/ollama-qa/{native}");
        for direct in [true, false] {
            let request = axum::http::Request::builder()
                .method("PUT").uri("/api/v1/model-routing")
                .header("content-type", "application/json")
                .body(Body::from(json!({"model_id":model_id,"direct":direct}).to_string())).unwrap();
            let response = manage::handle(&g, &admin, request).await;
            assert!(response.is_ok(), "qualified model ID must resolve: {model_id}");
            assert_eq!(g.registry.lock().unwrap().model_routing.contains_key(&model_id), direct);
        }
    }
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn per_model_direct_routing_bypasses_only_that_model() {
    let root = temp();
    let config = json!({
        "url": "http://v-scheduler:38120/api/v1",
        "token": "k".repeat(32),
        "models": {
            "vendor/echo": {"mode":"permit","executor_id":"manual-vendor","upstream_url":"http://fixture/v1"},
            "vendor/embed": {"mode":"permit","executor_id":"manual-vendor","upstream_url":"http://fixture/v1"}
        }
    });
    std::fs::write(root.join("scheduler.local.json"), config.to_string()).unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            root.join("scheduler.local.json"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();
    }
    let g = gateway(&root, Router::new());
    let u = {
        let mut registry = g.registry.lock().unwrap();
        registry.put(upstream("vendor"), vec![model("echo"), model("embed")]);
        registry.upstreams["vendor"].clone()
    };
    assert!(lease::approval(&g, &u, "echo", "POST", "/chat/completions", false)
        .unwrap()
        .is_some());
    let put = |model_id: &str, direct: bool| {
        axum::http::Request::builder()
            .method("PUT")
            .uri("/api/v1/model-routing")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"model_id": model_id, "direct": direct}).to_string(),
            ))
            .unwrap()
    };
    let denied = manage::handle(&g, &principal("alice"), put("vendor/echo", true))
        .await
        .err()
        .unwrap();
    assert_eq!(denied.status, StatusCode::FORBIDDEN);
    let mut root_user = principal("root");
    root_user.admin = true;
    assert_eq!(
        manage::handle(&g, &root_user, put("vendor/missing", true))
            .await
            .err()
            .unwrap()
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        manage::handle(&g, &root_user, put("vendor/echo", true))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert!(lease::approval(&g, &u, "echo", "POST", "/chat/completions", false)
        .unwrap()
        .is_none());
    assert!(lease::approval(&g, &u, "embed", "POST", "/chat/completions", false)
        .unwrap()
        .is_some());
    let disk: Value =
        serde_json::from_slice(&std::fs::read(root.join("state.json")).unwrap()).unwrap();
    assert_eq!(disk["model_routing"]["vendor/echo"]["direct"], true);
    let models = manage::handle(
        &g,
        &root_user,
        axum::http::Request::builder()
            .uri("/api/v1/models")
            .body(Body::empty())
            .unwrap(),
    )
    .await
    .unwrap();
    let bytes = axum::body::to_bytes(models.into_body(), 1 << 20)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    let echo = value["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["qualified"] == "manual/echo")
        .unwrap();
    assert_eq!(echo["direct"], true);
    assert_eq!(echo["upstream_url"], "http://fixture/v1");
    manage::handle(&g, &root_user, put("vendor/echo", false))
        .await
        .unwrap();
    assert!(lease::approval(&g, &u, "echo", "POST", "/chat/completions", false)
        .unwrap()
        .is_some());
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn managed_upstream_direct_routing_requires_an_administrator() {
    let root = temp();
    let g = gateway(&root, Router::new());
    {
        let mut registry = g.registry.lock().unwrap();
        let mut u = upstream("vendor");
        u.source = "v-scheduler".into();
        registry.put(u, vec![model("echo")]);
    }
    let put = |direct: bool| {
        axum::http::Request::builder()
            .method("PUT")
            .uri("/api/v1/upstreams/vendor/routing")
            .header("content-type", "application/json")
            .body(Body::from(
                json!({"weight":1,"max_inflight":0,"disabled":false,"direct":direct}).to_string(),
            ))
            .unwrap()
    };
    let denied = manage::handle(&g, &principal("alice"), put(true))
        .await
        .err()
        .unwrap();
    assert_eq!(denied.status, StatusCode::FORBIDDEN);

    let mut root_user = principal("root");
    root_user.admin = true;
    let response = manage::handle(&g, &root_user, put(true)).await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    // Once an administrator enabled it, owners may still save other fields.
    let response = manage::handle(&g, &principal("alice"), put(true))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    std::fs::remove_dir_all(root).unwrap();
}
