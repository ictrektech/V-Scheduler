use super::tests::{gateway, model, principal, temp, upstream};
use super::*;
use axum::{Json, routing::post};

fn admin() -> Identity {
    Identity {
        admin: true,
        ..principal("admin")
    }
}
fn candidate(model: &str, upstream: &str, tier: &str) -> Value {
    json!({"model":format!("manual/{model}"),"upstream_id":upstream,"tier":tier,"context_tokens":32768,"max_output_tokens":4096})
}
fn body(text: &str) -> Value {
    json!({"model":"auto","messages":[{"role":"user","content":text}],"max_tokens":128})
}
fn setup(g: &Gateway) -> Value {
    let mut r = g.registry.lock().unwrap();
    for (name, key) in [("small", "s"), ("large", "l")] {
        let mut u = upstream(key);
        u.base_url = "http://127.0.0.2:12345/v1".into();
        r.put(u, vec![model(name)]);
    }
    json!({"enabled":true,"candidates":[candidate("small","s","simple"),candidate("large","l","complex")]})
}
#[test]
fn auto_configuration_is_admin_only_validated_atomic_and_persistent() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let config = setup(&g);
    assert_eq!(
        auto::configure(&g, &principal("alice"), config.clone())
            .unwrap_err()
            .status,
        403
    );
    let saved = auto::configure(&g, &admin(), config.clone()).unwrap();
    assert_eq!(saved["revision"], 1);
    assert_eq!(
        auto::configure(&g, &admin(), config).unwrap_err().status,
        409
    );
    assert!(auto::configuration(&g, &principal("alice")).is_err());
    let restored = Registry::open(&root.join("state.json")).unwrap();
    assert_eq!(restored.auto.candidates.len(), 2);
    assert!(restored.auto.enabled);
    for mut invalid in [
        json!({"revision":1,"enabled":true,"candidates":[]}),
        json!({"revision":1,"policy":"anything"}),
        json!({"revision":1,"enabled":true,"classifier":"jev"}),
        json!({"revision":1,"unknown":true}),
    ] {
        if invalid["policy"] == "anything" {
            invalid["candidates"] = saved["candidates"].clone();
        }
        assert!(auto::configure(&g, &admin(), invalid).is_err());
    }
    assert_eq!(g.registry.lock().unwrap().auto.revision, 1);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn auto_rules_route_chinese_and_enforce_candidate_capability_and_quality() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let config = setup(&g);
    auto::configure(&g, &admin(), config).unwrap();
    let user = principal("alice");
    let h = HeaderMap::new();
    assert_eq!(
        auto::resolve(&g, &user, &body("总结这段文字"), &h)
            .await
            .unwrap()
            .resolved_model,
        "manual/small"
    );
    assert_eq!(
        auto::resolve(&g, &user, &body("分析这段代码的并发死锁"), &h)
            .await
            .unwrap()
            .resolved_model,
        "manual/large"
    );
    let mut tool = body("总结");
    tool["tools"] = json!([{"type":"function","function":{"name":"lookup"}}]);
    let d = auto::resolve(&g, &user, &tool, &h).await.unwrap();
    assert!(d.resolved_model.is_empty());
    assert!(
        d.candidates
            .iter()
            .all(|v| v["reason"] == "tools_unsupported")
    );
    {
        let mut r = g.registry.lock().unwrap();
        r.models.get_mut("l").unwrap()[0]
            .capabilities
            .push("tools".into());
    }
    assert_eq!(
        auto::resolve(&g, &user, &tool, &h)
            .await
            .unwrap()
            .resolved_model,
        "manual/large"
    );
    let mut image = body("总结");
    image["messages"][0]["content"] =
        json!([{"type":"image_url","image_url":{"url":"https://example.test/a.png"}}]);
    assert!(
        auto::resolve(&g, &user, &image, &h)
            .await
            .unwrap()
            .resolved_model
            .is_empty()
    );
    {
        let mut r = g.registry.lock().unwrap();
        r.models.get_mut("l").unwrap()[0]
            .capabilities
            .push("vision".into());
        r.auto.candidates[1].image_tokens = Some(4096);
    }
    assert_eq!(
        auto::resolve(&g, &user, &image, &h)
            .await
            .unwrap()
            .resolved_model,
        "manual/large"
    );
    let mut format = body("总结");
    format["response_format"] =
        json!({"type":"json_schema","json_schema":{"name":"result","schema":{"type":"object"}}});
    assert!(
        auto::resolve(&g, &user, &format, &h)
            .await
            .unwrap()
            .resolved_model
            .is_empty()
    );
    {
        let mut r = g.registry.lock().unwrap();
        r.auto.candidates.retain(|c| c.upstream_id == "s");
    }
    assert!(
        auto::resolve(&g, &user, &body("证明这个公式"), &h)
            .await
            .unwrap()
            .resolved_model
            .is_empty()
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn auto_private_cloud_context_capacity_and_pinned_constraints_never_relax() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let config = setup(&g);
    auto::configure(&g, &admin(), config).unwrap();
    let user = principal("alice");
    let h = HeaderMap::new();
    {
        let mut r = g.registry.lock().unwrap();
        let u = r.upstreams.get_mut("l").unwrap();
        u.visibility = "private".into();
        u.owner = "bob".into();
    }
    let d = auto::resolve(&g, &user, &body("分析并发"), &h)
        .await
        .unwrap();
    assert!(d.resolved_model.is_empty());
    assert!(!json!(d).to_string().contains("manual/large"));
    {
        let mut r = g.registry.lock().unwrap();
        let u = r.upstreams.get_mut("l").unwrap();
        u.visibility = "public".into();
        u.kind = "cloud".into();
    }
    assert!(
        auto::resolve(&g, &user, &body("分析并发"), &h)
            .await
            .unwrap()
            .resolved_model
            .is_empty()
    );
    {
        let mut r = g.registry.lock().unwrap();
        r.auto.allow_cloud = true;
    }
    assert_eq!(
        auto::resolve(&g, &user, &body("分析并发"), &h)
            .await
            .unwrap()
            .resolved_model,
        "manual/large"
    );
    let mut local = h.clone();
    local.insert("x-vscheduler-auto-local-only", "true".parse().unwrap());
    assert!(
        auto::resolve(&g, &user, &body("分析并发"), &local)
            .await
            .unwrap()
            .resolved_model
            .is_empty()
    );
    let mut long = body("总结");
    long["max_tokens"] = json!(100000);
    assert!(
        auto::resolve(&g, &user, &long, &h)
            .await
            .unwrap()
            .resolved_model
            .is_empty()
    );
    let mut pinned = h.clone();
    pinned.insert("x-vrouter-upstream", "s".parse().unwrap());
    assert_eq!(
        auto::resolve(&g, &user, &body("总结"), &pinned)
            .await
            .err()
            .unwrap()
            .status,
        400
    );
    {
        let mut r = g.registry.lock().unwrap();
        r.routing.insert(
            "s".into(),
            registry::Routing {
                max_inflight: 1,
                ..Default::default()
            },
        );
        r.inflight.insert("s".into(), 1);
    }
    assert_eq!(
        auto::resolve(&g, &user, &body("总结"), &h)
            .await
            .unwrap()
            .upstream_id,
        "l"
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn auto_same_named_unselected_instance_cannot_receive_inference() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let config = setup(&g);
    auto::configure(&g, &admin(), config).unwrap();
    let mut r = g.registry.lock().unwrap();
    r.put(upstream("evil"), vec![model("small")]);
    for _ in 0..3 {
        assert_eq!(
            r.acquire_from(
                "manual/small",
                "/chat/completions",
                "",
                &principal("alice"),
                Some("s")
            )
            .unwrap()
            .0
            .id,
            "s"
        );
    }
    r.routing.insert(
        "s".into(),
        registry::Routing {
            disabled: true,
            ..Default::default()
        },
    );
    assert!(
        r.acquire_from(
            "manual/small",
            "/chat/completions",
            "",
            &principal("alice"),
            Some("s")
        )
        .is_err()
    );
    drop(r);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn auto_sessions_are_isolated_and_tool_turns_cannot_silently_switch() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let config = setup(&g);
    auto::configure(&g, &admin(), config).unwrap();
    let mut h = HeaderMap::new();
    h.insert("x-vscheduler-session-id", "conversation".parse().unwrap());
    let user = principal("alice");
    let d = auto::resolve(&g, &user, &body("分析并发代码"), &h)
        .await
        .unwrap();
    auto::remember(&g, &user, &h, &d);
    assert!(
        auto::resolve(&g, &user, &body("总结"), &h)
            .await
            .unwrap()
            .session_reused
    );
    assert!(
        !auto::resolve(&g, &principal("bob"), &body("总结"), &h)
            .await
            .unwrap()
            .session_reused
    );
    let mut tool = body("总结");
    tool["messages"]
        .as_array_mut()
        .unwrap()
        .push(json!({"role":"tool","tool_call_id":"a","content":"lookup result"}));
    assert!(
        auto::resolve(&g, &user, &tool, &h)
            .await
            .unwrap()
            .session_reused
    );
    assert_eq!(
        auto::resolve(&g, &user, &tool, &HeaderMap::new())
            .await
            .err()
            .unwrap()
            .status,
        422
    );
    {
        g.registry
            .lock()
            .unwrap()
            .upstreams
            .get_mut("l")
            .unwrap()
            .healthy = false;
    }
    assert_eq!(
        auto::resolve(&g, &user, &tool, &h)
            .await
            .err()
            .unwrap()
            .status,
        409
    );
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn auto_jev_native_classification_real_chat_sse_and_fallback() {
    use std::sync::atomic::AtomicUsize;
    let root = temp();
    let calls = Arc::new(AtomicUsize::new(0));
    let observed = calls.clone();
    let server=Router::new().route("/v1/systemone",post(move|headers:HeaderMap,Json(v):Json<Value>|{let calls=observed.clone();async move {
        calls.fetch_add(1,Ordering::SeqCst);assert_eq!(headers["authorization"],"Bearer classifier-secret");assert_eq!(v["model"],"jev");assert!(v.get("messages").is_none());
        Json(json!({"answers":{"task":{"choice":"code","confidence":0.99},"complexity":{"choice":"complex","confidence":0.99}},"usage":{"input_tokens":50,"output_tokens":0}}))
    }})).route("/v1/chat/completions",post(|headers:HeaderMap,Json(v):Json<Value>|async move {
        assert!(headers.get("x-vscheduler-auto-policy").is_none());assert!(headers.get("x-vscheduler-session-id").is_none());assert_eq!(v["model"],"large");
        if v["stream"]==true {(StatusCode::OK,[("content-type","text/event-stream")],"data: {\"model\":\"large\",\"choices\":[{\"delta\":{\"content\":\"auto fixture\"},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n".to_string()).into_response()}else{Json(json!({"model":"large","choices":[{"message":{"content":"auto fixture"}}]})).into_response()}
    }));
    let listener = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let task = tokio::spawn(async move { axum::serve(listener, server).await.unwrap() });
    let g = gateway(&root, Router::new());
    let mut config = setup(&g);
    {
        let mut r = g.registry.lock().unwrap();
        for u in r.upstreams.values_mut() {
            u.base_url = format!("http://{addr}/v1");
        }
        let mut u = upstream("j");
        u.base_url = format!("http://{addr}/v1");
        u.protocol = "typesafe".into();
        u.kind = "cloud".into();
        u.api_key = "classifier-secret".into();
        r.put(u, vec![model("jev")]);
    }
    config["classifier"] = json!("jev");
    config["allow_cloud_classifier"] = json!(true);
    config["jev_model"] = json!("manual/jev");
    config["jev_upstream_id"] = json!("j");
    auto::configure(&g, &admin(), config).unwrap();
    let user = principal("alice");
    let h = HeaderMap::new();
    let d = auto::resolve(&g, &user, &body("总结"), &h).await.unwrap();
    assert_eq!(d.resolved_model, "manual/large");
    assert_eq!(json!(d)["classification"]["source"], "jev");
    let before = calls.load(Ordering::SeqCst);
    let mut local = h.clone();
    local.insert("x-vscheduler-auto-local-only", "true".parse().unwrap());
    auto::resolve(&g, &user, &body("总结"), &local)
        .await
        .unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), before);
    for stream in [false, true] {
        let mut v = body("hello");
        v["stream"] = json!(stream);
        let request = axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap();
        let response = proxy::handle(g.clone(), user.clone(), request)
            .await
            .unwrap();
        assert_eq!(
            response.headers()["x-vscheduler-auto-model"],
            "manual/large"
        );
        let bytes = axum::body::to_bytes(response.into_body(), 65536)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("auto fixture"));
    }
    assert!(
        g.registry
            .lock()
            .unwrap()
            .inflight
            .values()
            .all(|v| *v == 0)
    );
    task.abort();
    let d = auto::resolve(&g, &user, &body("总结"), &h).await.unwrap();
    assert_eq!(json!(d)["classification"]["fallback"], true);
    assert_eq!(d.resolved_model, "manual/small");
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn auto_managed_inference_requires_permit_and_releases_before_return() {
    use std::sync::atomic::AtomicUsize;
    let attempts = Arc::new(AtomicUsize::new(0));
    let releases = Arc::new(AtomicUsize::new(0));
    let executions = Arc::new(AtomicUsize::new(0));
    let deny = Arc::new(AtomicBool::new(false));
    let a = attempts.clone();
    let d = deny.clone();
    let rel = releases.clone();
    let exe = executions.clone();
    let controller = Router::new()
        .route(
            "/api/v1/permits",
            post(move || {
                let deny = d.load(Ordering::SeqCst);
                a.fetch_add(1, Ordering::SeqCst);
                async move {
                    if deny {
                        (
                            StatusCode::TOO_MANY_REQUESTS,
                            Json(json!({"code":"WAIT_RESOURCE"})),
                        )
                    } else {
                        (
                            StatusCode::OK,
                            Json(json!({"data":{"permit_id":"auto-permit"}})),
                        )
                    }
                }
            }),
        )
        .route(
            "/api/v1/permits/{id}/release",
            post(move |Json(v): Json<Value>| {
                let rel = rel.clone();
                async move {
                    assert_eq!(v["stopped"], true);
                    rel.fetch_add(1, Ordering::SeqCst);
                    Json(json!({"data":{"state":"released"}}))
                }
            }),
        );
    let native = Router::new().route(
        "/v1/chat/completions",
        post(move |headers: HeaderMap, Json(v): Json<Value>| {
            let exe = exe.clone();
            async move {
                assert_eq!(headers["x-vscheduler-permit-id"], "auto-permit");
                assert_eq!(v["model"], "small");
                exe.fetch_add(1, Ordering::SeqCst);
                Json(json!({"choices":[{"message":{"content":"approved"},"finish_reason":"stop"}]}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, native).await.unwrap() });
    let root = temp();
    let g = gateway(&root, controller);
    let mut config = setup(&g);
    config["candidates"] = json!([candidate("small", "s", "complex")]);
    g.registry
        .lock()
        .unwrap()
        .upstreams
        .get_mut("s")
        .unwrap()
        .base_url = format!("http://{addr}/v1");
    let binding = json!({"url":"http://v-scheduler:38120/api/v1","token":"auto-fixture-scoped-submitter-token-1234567890","require_model_policy":true,"models":{"s/small":{"mode":"permit","executor_id":"auto-executor","completion":"synchronous","max_input_bytes":4096,"limits":{"max_tokens":1024}}}});
    use std::os::unix::fs::PermissionsExt;
    let path = root.join("scheduler.local.json");
    std::fs::write(&path, binding.to_string()).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    auto::configure(&g, &admin(), config).unwrap();
    let request = || {
        axum::http::Request::builder()
            .method("POST")
            .uri("/v1/chat/completions")
            .header("content-type", "application/json")
            .body(Body::from(body("hello").to_string()))
            .unwrap()
    };
    // Dry run checks the binding but does not reserve or execute.
    assert_eq!(
        auto::resolve(&g, &principal("alice"), &body("hello"), &HeaderMap::new())
            .await
            .unwrap()
            .upstream_id,
        "s"
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    let response = proxy::handle(g.clone(), principal("alice"), request())
        .await
        .unwrap();
    let _ = axum::body::to_bytes(response.into_body(), 65536)
        .await
        .unwrap();
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    deny.store(true, Ordering::SeqCst);
    assert!(
        proxy::handle(g.clone(), principal("alice"), request())
            .await
            .is_err()
    );
    assert_eq!(attempts.load(Ordering::SeqCst), 2);
    assert_eq!(executions.load(Ordering::SeqCst), 1);
    assert_eq!(releases.load(Ordering::SeqCst), 1);
    assert!(
        g.registry
            .lock()
            .unwrap()
            .inflight
            .values()
            .all(|n| *n == 0)
    );
    server.abort();
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn auto_policies_use_profile_cost_and_load_only_after_quality_floor() {
    let root = temp();
    let g = gateway(&root, Router::new());
    let config = setup(&g);
    auto::configure(&g, &admin(), config).unwrap();
    {
        let mut r = g.registry.lock().unwrap();
        let small = &mut r.auto.candidates[0];
        small.cold_start_ms = 10;
        small.input_price = Some(10.0);
        small.output_price = Some(10.0);
        let large = &mut r.auto.candidates[1];
        large.cold_start_ms = 20000;
        large.input_price = Some(1.0);
        large.output_price = Some(1.0);
    }
    for (policy, model) in [
        ("balanced", "manual/small"),
        ("fast", "manual/small"),
        ("quality", "manual/large"),
        ("economy", "manual/large"),
    ] {
        let mut headers = HeaderMap::new();
        headers.insert("x-vscheduler-auto-policy", policy.parse().unwrap());
        assert_eq!(
            auto::resolve(&g, &principal("alice"), &body("总结"), &headers)
                .await
                .unwrap()
                .resolved_model,
            model
        );
        assert_eq!(
            auto::resolve(&g, &principal("alice"), &body("证明公式"), &headers)
                .await
                .unwrap()
                .resolved_model,
            "manual/large"
        );
    }
    {
        let mut r = g.registry.lock().unwrap();
        r.auto.candidates[1].cold_start_ms = 0;
    }
    assert_eq!(
        auto::resolve(&g, &principal("alice"), &body("总结"), &HeaderMap::new())
            .await
            .unwrap()
            .resolved_model,
        "manual/large",
        "balanced should avoid a slower cold load when a qualified faster candidate exists"
    );
    std::fs::remove_dir_all(root).unwrap();
}

#[tokio::test]
async fn local_systemone_classifier_requires_admission_unloads_and_works_local_only() {
    use std::sync::atomic::AtomicUsize;
    let admitted=Arc::new(AtomicUsize::new(0)); let released=Arc::new(AtomicUsize::new(0));
    let invoked=Arc::new(AtomicUsize::new(0)); let deny=Arc::new(AtomicBool::new(false));
    let (a,r,d)=(admitted.clone(),released.clone(),deny.clone());
    let controller=Router::new().route("/api/v1/permits",post(move||{let(a,d)=(a.clone(),d.clone());async move{a.fetch_add(1,Ordering::SeqCst);if d.load(Ordering::SeqCst){(StatusCode::TOO_MANY_REQUESTS,Json(json!({"code":"WAIT_RESOURCE"})))}else{(StatusCode::OK,Json(json!({"data":{"permit_id":"local-classifier"}})))}}}))
      .route("/api/v1/permits/{id}/release",post(move|Json(v):Json<Value>|{let r=r.clone();async move{assert_eq!(v["stopped"],true);r.fetch_add(1,Ordering::SeqCst);Json(json!({"data":{"state":"released"}}))}}));
    let n=invoked.clone();
    let native=Router::new().route("/v1/systemone",post(move|h:HeaderMap,Json(v):Json<Value>|{let n=n.clone();async move{
        assert_eq!(h["x-vscheduler-permit-id"],"local-classifier");assert_eq!(v["model"],"tev");assert_eq!(v["keep_alive"],0);
        n.fetch_add(1,Ordering::SeqCst);Json(json!({"answers":{"task":{"choice":"general"},"complexity":{"choice":"simple","confidence":0.99}}}))
    }}));
    let listener=tokio::net::TcpListener::bind("127.0.0.2:0").await.unwrap();let addr=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{axum::serve(listener,native).await.unwrap()});
    let root=temp();let g=gateway(&root,controller);let mut cfg=setup(&g);
    {let mut r=g.registry.lock().unwrap();let mut u=upstream("j");u.base_url=format!("http://{addr}/v1");u.protocol="ollama".into();r.put(u,vec![Model{id:"tev".into(),capabilities:vec!["decision".into()],..Default::default()}]);}
    let policy=json!({"mode":"permit","executor_id":"local-decision","completion":"synchronous","max_input_bytes":4096,"limits":{"max_tokens":1024}});
    let binding=json!({"url":"http://v-scheduler:38120/api/v1","token":"auto-fixture-scoped-submitter-token-1234567890","require_model_policy":true,"models":{"j/tev":policy,"s/small":policy,"l/large":policy}});
    use std::os::unix::fs::PermissionsExt;let path=root.join("scheduler.local.json");std::fs::write(&path,binding.to_string()).unwrap();std::fs::set_permissions(&path,std::fs::Permissions::from_mode(0o600)).unwrap();
    cfg["classifier"]=json!("systemone");cfg["jev_model"]=json!("manual/tev");cfg["jev_upstream_id"]=json!("j");cfg["classifier_timeout_ms"]=json!(60000);
    auto::configure(&g,&admin(),cfg).unwrap();let mut h=HeaderMap::new();h.insert("x-vscheduler-auto-local-only","true".parse().unwrap());
    let result=auto::resolve(&g,&principal("alice"),&body("hello"),&h).await.unwrap();assert_eq!(json!(result.classification)["source"],"local_systemone");assert_eq!(result.resolved_model,"manual/small");
    assert_eq!(admitted.load(Ordering::SeqCst),1);assert_eq!(released.load(Ordering::SeqCst),1);assert_eq!(invoked.load(Ordering::SeqCst),1);
    deny.store(true,Ordering::SeqCst);let result=auto::resolve(&g,&principal("alice"),&body("hello"),&h).await.unwrap();assert_eq!(json!(result.classification)["fallback"],true);assert_eq!(invoked.load(Ordering::SeqCst),1);
    task.abort();std::fs::remove_dir_all(root).unwrap();
}
