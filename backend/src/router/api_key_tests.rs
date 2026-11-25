use super::*;
use axum::{Json, routing::get};
use sha2::{Digest, Sha256};
use std::os::unix::fs::PermissionsExt;

async fn fixture() -> (Arc<Gateway>, Identity, Arc<Mutex<Value>>, tokio::task::JoinHandle<()>) {
    let root=tests::temp();let g=tests::gateway(&root,Router::new());
    let user=Arc::new(Mutex::new(json!({"id":"alice","username":"alice","role_code":"user","enable":1,"delete":0})));
    let data=user.clone();
    let server=Router::new().route("/users",get(move |headers:HeaderMap,axum::extract::Query(query):axum::extract::Query<std::collections::HashMap<String,String>>| {
        let data=data.clone();async move {
            assert_eq!(headers["authorization"],"Bearer context-fixture");assert_eq!(query["user_id"],"alice");
            Json(json!({"code":0,"data":{"data":[data.lock().unwrap().clone()]}}))
        }
    }));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let task=tokio::spawn(async move{axum::serve(listener,server).await.unwrap();});
    let context=root.join("context-token");std::fs::write(&context,"context-fixture").unwrap();std::fs::set_permissions(&context,std::fs::Permissions::from_mode(0o600)).unwrap();
    {let mut store=g.api_keys.lock().unwrap();store.authority=format!("http://{address}/users");store.context=context;}
    let mut alice=tests::principal("alice");alice.vos_user=true;
    (g,alice,user,task)
}
fn request(method:&str,path:&str,body:Value)->Request {
    Request::builder().method(method).uri(path).header("content-type","application/json").body(Body::from(serde_json::to_vec(&body).unwrap())).unwrap()
}
async fn value(response:Response)->Value {serde_json::from_slice(&axum::body::to_bytes(response.into_body(),1<<20).await.unwrap()).unwrap()}
async fn create(g:&Gateway,alice:&Identity)->Value {
    value(api_keys::handle(g,alice,request("POST","/api/v1/api-keys",json!({"name":"External Agent"}))).await.unwrap()).await
}
#[tokio::test]
async fn personal_keys_persist_hashes_inherit_identity_and_revoke() {
    let (g,alice,user,task)=fixture().await;
    let created=create(&g,&alice).await;let secret=created["api_key"].as_str().unwrap();
    assert_eq!(secret.len(),71);assert_eq!(created["key"]["active"],true);
    let path=g.state_path.with_file_name("api-keys.json");let disk=std::fs::read_to_string(&path).unwrap();
    assert!(!disk.contains(secret));assert!(!disk.contains("context-fixture"));
    assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode()&0o777,0o600);
    assert!(api_keys::Store::open(&g.state_path).is_ok());
    let identity=api_keys::authenticate(&g,secret).await.unwrap();assert_eq!(identity.owner,"alice");assert!(!identity.admin);assert!(!identity.vos_user);
    let list=value(api_keys::handle(&g,&alice,request("GET","/api/v1/api-keys",json!({}))).await.unwrap()).await;
    assert!(!list.to_string().contains(secret));assert!(!list.to_string().contains("hash"));
    let mut bob=tests::principal("bob");bob.vos_user=true;
    let path=format!("/api/v1/api-keys/{}",created["key"]["id"].as_str().unwrap());
    assert_eq!(api_keys::handle(&g,&bob,request("DELETE",&path,json!({}))).await.err().unwrap().status,404);
    assert_eq!(api_keys::handle(&g,&identity,request("POST","/api/v1/api-keys",json!({"name":"Escalate"}))).await.err().unwrap().status,403);
    user.lock().unwrap()["enable"]=json!(0);assert_eq!(api_keys::authenticate(&g,secret).await.err().unwrap().status,401);
    user.lock().unwrap()["enable"]=json!(1);user.lock().unwrap()["delete"]=json!(1);assert_eq!(api_keys::authenticate(&g,secret).await.err().unwrap().status,401);
    user.lock().unwrap()["delete"]=json!(0);user.lock().unwrap()["role_code"]=json!("system_admin");assert!(api_keys::authenticate(&g,secret).await.unwrap().admin);
    api_keys::handle(&g,&alice,request("DELETE",&path,json!({}))).await.unwrap();assert_eq!(api_keys::authenticate(&g,secret).await.err().unwrap().status,401);
    assert!(api_keys::Store::open(&g.state_path).is_ok());task.abort();std::fs::remove_dir_all(g.state_path.parent().unwrap()).unwrap();
}
#[tokio::test]
async fn model_key_rotation_never_grants_other_private_models() {
    let (g,alice,_,task)=fixture().await;
    let mut u=tests::upstream("source");u.owner="alice".into();u.visibility="private".into();
    {let mut r=g.registry.lock().unwrap();r.put(u,vec![tests::model("one"),tests::model("two")]);}
    let response=api_keys::model_access(&g,&alice,request("PUT","/api/v1/models/access-key",json!({"upstream_id":"source","model":"manual/one","access_key":true}))).await.unwrap();
    let result=value(response).await;let first=result["model_access_keys"][0]["api_key"].as_str().unwrap();
    let id=api_keys::authenticate(&g,first).await.unwrap();
    {let mut r=g.registry.lock().unwrap();assert_eq!(r.list_models(&id).len(),1);assert!(r.acquire("manual/two","/chat/completions","",&id).is_err());}
    let custom=value(api_keys::model_access(&g,&alice,request("PUT","/api/v1/models/access-key",json!({"upstream_id":"source","model":"manual/one","access_key":"custom-model-secret-123"}))).await.unwrap()).await;
    assert!(custom["model_access_keys"][0].get("api_key").is_none(), "custom keys must not be echoed");
    assert!(api_keys::authenticate(&g,first).await.is_err());assert!(api_keys::authenticate(&g,"custom-model-secret-123").await.is_ok());
    let revoked=value(api_keys::model_access(&g,&alice,request("PUT","/api/v1/models/access-key",json!({"upstream_id":"source","model":"manual/one","access_key":null}))).await.unwrap()).await;
    assert_eq!(revoked,json!({"model_access_keys":[{"model":"source/one","key":null}]}));
    assert!(api_keys::authenticate(&g,"custom-model-secret-123").await.is_err());task.abort();
}
#[tokio::test]
async fn public_catalog_ignores_invalid_keys_and_private_stays_hidden() {
    let (g,alice,_,task)=fixture().await;let created=create(&g,&alice).await;let secret=created["api_key"].as_str().unwrap();
    let mut u=tests::upstream("source");u.owner="alice".into();u.visibility="private".into();let mut public=tests::model("public");public.visibility=Some("public".into());
    g.registry.lock().unwrap().put(u,vec![tests::model("private"),public]);
    for key in [None,Some("vs-key-invalid"),Some(secret)] {
        let mut headers=HeaderMap::new();if let Some(key)=key {headers.insert("authorization",format!("Bearer {key}").parse().unwrap());}
        let id=auth::identity(&g,&headers,false).await.unwrap();
        assert_eq!(g.registry.lock().unwrap().list_models(&id).len(),if key==Some(secret){2}else{1});
    }
    let identity=api_keys::authenticate(&g,secret).await.unwrap();
    assert_eq!(manage::handle(&g,&identity,request("GET","/api/v1/upstreams",json!({}))).await.err().unwrap().status,403);
    assert!(manage::handle(&g,&identity,request("GET","/api/v1/models",json!({}))).await.is_ok());
    // Existing VOS identities continue through their verified token cache.
    let mut headers=HeaderMap::new();headers.insert("authorization","Bearer vos-fixture".parse().unwrap());
    g.identities.lock().unwrap().insert(format!("{:x}",Sha256::digest("vos-fixture")),(alice,std::time::Instant::now()));
    assert!(auth::identity(&g,&headers,true).await.unwrap().vos_user);task.abort();
}

#[tokio::test]
async fn keys_work_through_dispatch_for_openai_anthropic_streaming_and_pinned_models() {
    let (g,alice,_,task)=fixture().await;
    let upstream=Router::new().route("/v1/chat/completions",axum::routing::post(|headers:HeaderMap,Json(body):Json<Value>|async move {
        assert_eq!(headers["authorization"],"Bearer upstream-secret-fixture");
        assert!(headers.get("x-api-key").is_none());
        if body["stream"]==true {
            ([("content-type","text/event-stream")],"data: {\"id\":\"key-fixture\",\"choices\":[{\"delta\":{\"content\":\"API_OK\"},\"finish_reason\":null}]}\n\ndata: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\ndata: [DONE]\n\n").into_response()
        }else{Json(json!({"id":"key-fixture","model":body["model"],"choices":[{"message":{"role":"assistant","content":"API_OK"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1}})).into_response()}
    }));
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();let address=listener.local_addr().unwrap();
    let upstream_task=tokio::spawn(async move{axum::serve(listener,upstream).await.unwrap();});
    let mut u=tests::upstream("source");u.owner="alice".into();u.visibility="private".into();u.base_url=format!("http://{address}/v1");u.api_key="upstream-secret-fixture".into();
    let mut public=tests::model("public");public.visibility=Some("public".into());
    g.registry.lock().unwrap().put(u,vec![public,tests::model("private"),tests::model("sibling")]);
    let created=create(&g,&alice).await;let personal=created["api_key"].as_str().unwrap();
    api_keys::configure_models(&g,"alice",vec![("source/private".into(),json!("model-access-secret-123"))]).unwrap();
    g.identities.lock().unwrap().insert(format!("{:x}",Sha256::digest("vos-fixture")),(alice,std::time::Instant::now()));
    for (native,key,expected) in [
        ("public",None,200),("public",Some("vs-key-invalid"),200),("public",Some(personal),200),
        ("private",Some(personal),200),("private",Some("model-access-secret-123"),200),("private",Some("vos-fixture"),200),
        ("private",None,404),("private",Some("vs-key-invalid"),404),("sibling",Some("model-access-secret-123"),404),
    ] {
        for endpoint in ["/v1/chat/completions","/v1/messages"] {
            for stream in [false,true] {
                for pinned in [false,true] {
                    let mut req=request("POST",endpoint,json!({"model":if pinned {native.to_owned()}else{format!("manual/{native}")},"messages":[{"role":"user","content":"Hello"}],"max_tokens":16,"stream":stream}));
                    if let Some(key)=key {let (header,value)=if endpoint.ends_with("messages"){("x-api-key",key.to_owned())}else{("authorization",format!("Bearer {key}"))};req.headers_mut().insert(header,value.parse().unwrap());}
                    if pinned {req.headers_mut().insert("x-vrouter-upstream","source".parse().unwrap());}
                    let response=handle(State(g.clone()),req).await;assert_eq!(response.status(),if expected==404 && (key==Some("vs-key-invalid") || pinned && key.is_none()){401}else{expected},"{native} {endpoint} stream={stream} pinned={pinned}");
                    if expected==200 {
                        let bytes=axum::body::to_bytes(response.into_body(),1<<20).await.unwrap();let body=String::from_utf8_lossy(&bytes);
                        assert!(body.contains("API_OK"));if stream && endpoint.ends_with("messages"){assert!(body.contains("message_stop"));}
                    }
                }
            }
        }
    }
    upstream_task.abort();task.abort();
}

#[tokio::test]
async fn registration_sets_preserves_and_rotates_model_credentials() {
    let (g,alice,_,task)=fixture().await;
    g.identities.lock().unwrap().insert(format!("{:x}",Sha256::digest("vos-fixture")),(alice,std::time::Instant::now()));
    let payload=json!({"service_id":"my-infer","base_url":"http://fixture/v1","protocol":"chat_completions","visibility":"private","models":[{"id":"chat-a","endpoints":["chat_completions"]}],"access_key":true});
    let mut req=request("POST","/api/v1/registrations",payload.clone());req.headers_mut().insert("authorization","Bearer vos-fixture".parse().unwrap());
    let response=dispatch(g.clone(),req).await.unwrap();assert_eq!(response.headers()["cache-control"],"no-store");
    let result=value(response).await;let key=result["model_access_keys"][0]["api_key"].as_str().unwrap();
    let identity=api_keys::authenticate(&g,key).await.unwrap();assert_eq!(g.registry.lock().unwrap().list_models(&identity).len(),1);
    *g.registry.lock().unwrap()=Registry::open(&g.state_path).unwrap();
    let mut recovery=request("POST","/api/v1/registrations/registered-my-infer/heartbeat",payload.clone());recovery.headers_mut().insert("authorization","Bearer vos-fixture".parse().unwrap());
    let recovered=value(dispatch(g.clone(),recovery).await.unwrap()).await;
    assert!(recovered["model_access_keys"].is_null(), "recovery must not generate or return a replacement key");
    assert!(api_keys::authenticate(&g,key).await.is_ok(), "a saved key must survive a full recovery heartbeat");
    let mut again=payload;again.as_object_mut().unwrap().remove("access_key");
    let mut req=request("POST","/api/v1/registrations",again);req.headers_mut().insert("authorization","Bearer vos-fixture".parse().unwrap());
    assert!(dispatch(g.clone(),req).await.is_ok());assert!(api_keys::authenticate(&g,key).await.is_ok());
    let mut req=request("PUT","/api/v1/models/access-key",json!({"upstream_id":"registered-my-infer","model":"registered/chat-a","access_key":"replacement-model-key-123"}));req.headers_mut().insert("authorization","Bearer vos-fixture".parse().unwrap());
    dispatch(g.clone(),req).await.unwrap();assert!(api_keys::authenticate(&g,key).await.is_err());assert!(api_keys::authenticate(&g,"replacement-model-key-123").await.is_ok());
    task.abort();
}

#[tokio::test]
async fn embedded_registered_model_uses_verified_administrator_as_key_owner() {
    let (g, mut admin, user, task) = fixture().await;
    let mut upstream = tests::upstream("registered-modelinfer");
    upstream.owner = "com.ictrek.v-scheduler".into();
    upstream.source = "registered".into();
    upstream.visibility = "private".into();
    upstream.model_prefix = "registered".into();
    g.registry.lock().unwrap().put(upstream, vec![tests::model("target"), tests::model("other")]);

    let request_key = || request("PUT", "/api/v1/models/access-key",
        json!({"upstream_id":"registered-modelinfer","model":"registered/target","access_key":true}));
    assert_eq!(api_keys::model_access(&g, &admin, request_key()).await.unwrap_err().status, 403);
    admin.admin = true;
    assert_eq!(api_keys::model_access(&g, &admin, request_key()).await.unwrap_err().status, 403);
    user.lock().unwrap()["role_code"] = json!("system_admin");

    let response = api_keys::model_access(&g, &admin, request_key()).await.unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let result = value(response).await;
    let key = result["model_access_keys"][0]["api_key"].as_str().unwrap();
    let identity = api_keys::authenticate(&g, key).await.unwrap();
    assert_eq!(identity.owner, admin.owner);
    assert_eq!(g.registry.lock().unwrap().list_models(&identity).len(), 1);
    task.abort();
}

#[tokio::test]
async fn expired_keys_and_insecure_stores_do_not_authorize_private_access() {
    let (g,alice,_,task)=fixture().await;let result=create(&g,&alice).await;let key=result["api_key"].as_str().unwrap();
    let mut snapshot=serde_json::to_value(&*g.api_keys.lock().unwrap()).unwrap();
    snapshot["keys"][0]["expires_at"]=json!(Utc::now()-chrono::Duration::seconds(1));
    let mut expired:api_keys::Store=serde_json::from_value(snapshot).unwrap();
    {let mut store=g.api_keys.lock().unwrap();expired.authority=store.authority.clone();expired.context=store.context.clone();*store=expired;}
    assert_eq!(api_keys::authenticate(&g,key).await.err().unwrap().status,401);
    std::fs::set_permissions(g.state_path.with_file_name("api-keys.json"),std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(api_keys::Store::open(&g.state_path).is_err());task.abort();
}

#[tokio::test]
async fn administrator_model_key_is_scoped_even_for_unowned_models() {
    let (g,mut alice,user,task)=fixture().await;
    alice.admin=true;user.lock().unwrap()["role_code"]=json!("system_admin");
    let mut u=tests::upstream("builtin");u.visibility="private".into();
    g.registry.lock().unwrap().put(u,vec![tests::model("one"),tests::model("two")]);
    let result=value(api_keys::model_access(&g,&alice,request("PUT","/api/v1/models/access-key",json!({"upstream_id":"builtin","model":"manual/one","access_key":true}))).await.unwrap()).await;
    let secret=result["model_access_keys"][0]["api_key"].as_str().unwrap();
    let identity=api_keys::authenticate(&g,secret).await.unwrap();assert!(identity.admin);
    assert_eq!(g.registry.lock().unwrap().list_models(&identity).len(),1);
    assert_eq!(manage::handle(&g,&identity,request("GET","/api/v1/upstreams",json!({}))).await.err().unwrap().status,403);
    user.lock().unwrap()["role_code"]=json!("user");
    let identity=api_keys::authenticate(&g,secret).await.unwrap();assert!(g.registry.lock().unwrap().list_models(&identity).is_empty());task.abort();
}

#[tokio::test]
async fn auto_key_is_admin_managed_and_does_not_delegate_private_model_access() {
    let (g, mut alice, user, task) = fixture().await;
    let auto_key = |access_key: Value| request("PUT", "/api/v1/models/access-key",
        json!({"upstream_id":"auto","model":"auto","access_key":access_key}));
    let anon = Identity::default();
    let body = json!({"model":"auto","messages":[{"role":"user","content":"hello"}]});
    assert!(auto::advertised(&g, &anon));
    assert_eq!(api_keys::model_access(&g, &alice, auto_key(json!(true))).await.unwrap_err().status, 403);

    alice.admin = true;
    assert_eq!(api_keys::model_access(&g, &alice, auto_key(json!(true))).await.unwrap_err().status, 403);
    user.lock().unwrap()["role_code"] = json!("system_admin");
    let generated = value(api_keys::model_access(&g, &alice, auto_key(json!(true))).await.unwrap()).await;
    let secret = generated["model_access_keys"][0]["api_key"].as_str().unwrap();
    assert_eq!(generated["model_access_keys"][0]["model"], "auto/auto");
    let identity = api_keys::authenticate(&g, secret).await.unwrap();
    assert!(!identity.admin);
    assert!(auto::advertised(&g, &identity));
    assert!(auto::advertised(&g, &alice));
    assert!(!auto::advertised(&g, &anon));
    let denial = proxy::handle(g.clone(), anon.clone(), request("POST", "/v1/chat/completions", body.clone())).await.unwrap_err();
    assert_eq!(denial.status, 401);

    let personal = create(&g, &alice).await;
    let personal = api_keys::authenticate(&g, personal["api_key"].as_str().unwrap()).await.unwrap();
    assert!(auto::advertised(&g, &personal));
    let mut private = tests::upstream("hidden");
    private.owner = alice.owner.clone();
    private.visibility = "private".into();
    let mut public = tests::upstream("published");
    public.visibility = "public".into();
    public.base_url = "http://public-fixture/v1".into();
    {
        let mut registry = g.registry.lock().unwrap();
        registry.put(private, vec![tests::model("hidden")]);
        registry.put(public, vec![tests::model("published")]);
        assert!(registry.acquire("manual/hidden", "/chat/completions", "", &identity).is_err());
    }
    let decision = auto::resolve_for(&g, &identity, &body, &HeaderMap::new(), "/chat/completions").await.unwrap();
    assert_eq!(decision.resolved_model, "manual/published");
    assert!(!serde_json::to_string(&decision.candidates).unwrap().contains("hidden"));
    assert_eq!(api_keys::model_access(&g, &alice, request("PUT", "/api/v1/models/access-key",
        json!({"upstream_id":"hidden","model":"manual/hidden","access_key":secret}))).await.unwrap_err().status, 409);

    let scoped = value(api_keys::model_access(&g, &alice, request("PUT", "/api/v1/models/access-key",
        json!({"upstream_id":"hidden","model":"manual/hidden","access_key":true}))).await.unwrap()).await;
    let scoped = api_keys::authenticate(&g, scoped["model_access_keys"][0]["api_key"].as_str().unwrap()).await.unwrap();
    assert_eq!(api_keys::auto_access(&g, &scoped).unwrap_err().status, 403);
    let updated = value(api_keys::model_access(&g, &alice, auto_key(json!("fixture-auto-custom-secret"))).await.unwrap()).await;
    assert!(updated["model_access_keys"][0].get("api_key").is_none());
    assert!(api_keys::authenticate(&g, secret).await.is_err());
    assert!(api_keys::authenticate(&g, "fixture-auto-custom-secret").await.is_ok());
    user.lock().unwrap()["role_code"] = json!("user");
    assert!(api_keys::authenticate(&g, "fixture-auto-custom-secret").await.is_err());
    user.lock().unwrap()["role_code"] = json!("system_admin");
    let revoked = value(api_keys::model_access(&g, &alice, auto_key(Value::Null)).await.unwrap()).await;
    assert_eq!(revoked["model_access_keys"][0], json!({"model":"auto/auto","key":null}));
    assert!(auto::advertised(&g, &anon));
    task.abort();
}
