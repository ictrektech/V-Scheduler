use super::*;
use axum::body::Bytes;
use axum::{Json, routing::post};
use futures_util::StreamExt;
use std::collections::BTreeMap;
fn principal(name: &str) -> Identity {
    Identity {
        owner: name.into(),
        username: name.into(),
        ..Default::default()
    }
}
fn upstream(id: &str) -> Upstream {
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
fn model(id: &str) -> Model {
    Model {
        id: id.into(),
        kind: "chat".into(),
        ..Default::default()
    }
}
fn temp() -> PathBuf {
    let path = std::env::temp_dir().join(format!("v-scheduler-router-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&path).unwrap();
    path
}
fn gateway(root: &Path, controller: Router) -> Arc<Gateway> {
    Arc::new(Gateway {
        registry: Mutex::new(Registry::default()),
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
        },
    );
    r.routing.insert(
        "b".into(),
        registry::Routing {
            weight: 1,
            max_inflight: 1,
            disabled: false,
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
