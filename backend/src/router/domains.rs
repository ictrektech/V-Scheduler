//! Shared model-domain vocabulary and durable per-source model annotations.
use super::*;
use axum::Json;
use std::collections::BTreeMap;

const BUILTINS: &[(&str, &str)] = &[
    ("general", "通用"), ("law", "法律"), ("mathematics", "数学"),
    ("physics", "物理"), ("medicine", "医药"), ("chemistry", "化学"),
    ("biology", "生物"), ("engineering", "工程"), ("computing", "计算机"),
    ("finance", "金融"), ("education", "教育"), ("humanities", "人文"),
];

pub(super) fn catalog(r: &Registry) -> Vec<Value> {
    BUILTINS.iter().map(|(id, name)| json!({"id":id,"name":name,"builtin":true}))
        .chain(r.domain_catalog.iter().map(|(id,name)| json!({"id":id,"name":name,"builtin":false}))).collect()
}
pub(super) fn effective(r: &Registry, m: &Model) -> Vec<String> {
    r.model_domains.get(&format!("{}/{}",m.upstream_id,m.id)).cloned().unwrap_or_else(||vec!["general".into()])
}
pub(super) fn validate(r: &Registry, value: &Value) -> Result<Vec<String>, Failure> {
    let rows=value.as_array().ok_or_else(||Failure::new(400,"domains must be an array of domain IDs"))?;
    if rows.len()>32 {return Err(Failure::new(400,"at most 32 domains may be selected"));}
    let mut ids=vec![];
    for row in rows {
        let key=row.as_str().ok_or_else(||Failure::new(400,"domain IDs must be strings"))?;
        if !BUILTINS.iter().any(|(id,_)| *id==key) && !r.domain_catalog.contains_key(key) {
            return Err(Failure::new(400,"unknown domain ID; create the category first"));
        }
        if !ids.iter().any(|id| id==key) {ids.push(key.to_owned());}
    }
    if ids.is_empty(){ids.push("general".into());}
    Ok(ids)
}
pub(super) fn writer(id: &Identity) -> Result<(), Failure> {
    if !id.admin && (!id.vos_user || id.owner.is_empty()) {
        return Err(Failure::new(403,"domain management requires a VOS user identity"));
    }
    Ok(())
}
pub(super) fn create(g: &Gateway, id: &Identity, p: &Value) -> Api<Response> {
    writer(id)?;
    let name=p.get("name").and_then(Value::as_str).unwrap_or("").trim();
    if name.is_empty() || name.chars().count()>80 || name.chars().any(char::is_control) {
        return Err(Failure::new(400,"domain name must contain 1 to 80 characters without control characters"));
    }
    let mut current=g.registry.lock().unwrap();
    if let Some(row)=catalog(&current).into_iter().find(|d|text(d,"name").to_lowercase()==name.to_lowercase() || text(d,"id").eq_ignore_ascii_case(name)) {
        return Ok(Json(json!({"domain":row,"created":false})).into_response());
    }
    if current.domain_catalog.len()>=1000 {return Err(Failure::new(400,"custom domain catalog is full"));}
    let mut next=current.clone();
    let key=format!("custom-{}",uuid::Uuid::new_v4());
    next.domain_catalog.insert(key.clone(),name.to_owned());
    next.persist(&g.state_path).map_err(|_|Failure::new(500,"cannot persist domain catalog"))?;
    *current=next;
    Ok((StatusCode::CREATED,Json(json!({"domain":{"id":key,"name":name,"builtin":false},"created":true}))).into_response())
}
pub(super) fn single(g: &Gateway, id: &Identity, p: &Value) -> Api<Response> {
    let body=json!({"models":[{"model":p["model"],"upstream_id":p["upstream_id"]}],"domains":p["domains"]});
    assign(g,id,&body,true)
}
pub(super) fn batch(g: &Gateway, id: &Identity, p: &Value) -> Api<Response> { assign(g,id,p,false) }
fn assign(g: &Gateway, id: &Identity, p: &Value, single: bool) -> Api<Response> {
    writer(id)?;
    let mut current=g.registry.lock().unwrap();
    let domains=validate(&current,&p["domains"])?;
    let targets=p["models"].as_array().filter(|v| !v.is_empty() && v.len()<=200)
        .ok_or_else(||Failure::new(400,"models must contain 1 to 200 model/upstream_id objects"))?;
    let mut selected=BTreeMap::new();
    let mut auto_target=false;
    for target in targets {
        let upstream=text(target,"upstream_id");let requested=text(target,"model");
        if upstream.is_empty() || requested.is_empty(){return Err(Failure::new(400,"each model requires upstream_id and model"));}
        if upstream=="auto" && requested=="auto" {
            if !id.admin{return Err(Failure::new(403,"Auto configuration requires an administrator"));}
            auto_target=true;continue;
        }
        let u=current.upstreams.get(upstream).ok_or_else(||Failure::new(404,"model not found"))?;
        let m=current.model_by_id(u,requested,id).or_else(||current.models.get(upstream)?.iter().find(|m|m.id==requested && m.visible(u,id)))
            .ok_or_else(||Failure::new(404,"model not found"))?;
        if !u.manageable(id){return Err(Failure::new(403,"model domains are managed by the owner or an administrator"));}
        selected.insert(format!("{}/{}",u.id,m.id),m.clone());
    }
    let mut next=current.clone();
    for key in selected.keys() {next.model_domains.insert(key.clone(),domains.clone());}
    let auto_changed=auto_target && next.auto.domains!=domains;
    if auto_changed {next.auto.domains=domains.clone();next.auto.revision+=1;}
    next.persist(&g.state_path).map_err(|_|Failure::new(500,"cannot persist model domains"))?;
    *current=next;
    if auto_changed {
        auto::invalidate(g);
    }
    if single && auto_target {
        let mut model=auto::model_view();model["domains"]=json!(domains);
        return Ok(Json(json!({"model":model})).into_response());
    }
    if single {
        return Ok(Json(json!({"model":manage::model_view(&current,selected.values().next().expect("one target"),id)})).into_response());
    }
    Ok(Json(json!({"updated":selected.len()+usize::from(auto_target),"domains":domains})).into_response())
}
// Validate declarations before any registration/network side effects. Missing
// fields retain existing annotations, including after heartbeat recovery.
pub(super) fn declarations(r: &Registry, p: &Value) -> Result<BTreeMap<String,Vec<String>>,Failure> {
    let mut result=BTreeMap::new();
    if let Some(value)=p.get("domains"){result.insert(String::new(),validate(r,value)?);}
    if let Some(rows)=p["models"].as_array(){for row in rows {
        if let Some(value)=row.get("domains") {
            let key=if text(row,"id").is_empty(){text(row,"model")}else{text(row,"id")};
            if key.is_empty(){return Err(Failure::new(400,"domain declaration requires a model ID"));}
            result.insert(key.into(),validate(r,value)?);
        }
    }}
    Ok(result)
}
pub(super) fn apply(r: &mut Registry, upstream: &str, declarations: &BTreeMap<String,Vec<String>>) {
    for model in r.models.get(upstream).into_iter().flatten() {
        if let Some(value)=declarations.get(&model.id).or_else(||declarations.get("")) {
            r.model_domains.insert(format!("{upstream}/{}",model.id),value.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use super::super::tests::{gateway,model,principal,temp,upstream};
    async fn call(g: &Arc<Gateway>, id: &Identity, method: &str, path: &str, body: Value) -> Result<Value,Failure> {
        let response=manage::handle(g,id,Request::builder().method(method).uri(path).header("content-type","application/json").body(Body::from(body.to_string())).unwrap()).await?;
        Ok(serde_json::from_slice(&axum::body::to_bytes(response.into_body(),1<<20).await.unwrap()).unwrap())
    }
    fn owned(g: &Gateway, source: &str, owner: &str) {
        let mut u=upstream(source);u.owner=owner.into();
        g.registry.lock().unwrap().put(u,vec![model("chat")]);
    }
    fn user(name: &str) -> Identity {Identity{vos_user:true,..principal(name)}}
    #[tokio::test]
    async fn shared_catalog_defaults_and_assignment_survive_restart_and_refresh() {
        let root=temp();let g=gateway(&root,Router::new());let alice=user("alice");
        owned(&g,"a","alice");owned(&g,"b","alice");
        let before=call(&g,&alice,"GET","/api/v1/models",json!({})).await.unwrap();
        assert_eq!(before["models"][0]["domains"],json!(["general"]));
        let created=call(&g,&alice,"POST","/api/v1/model-domains",json!({"name":" Patents "})).await.unwrap();
        let key=created["domain"]["id"].as_str().unwrap();
        let duplicate=call(&g,&user("bob"),"POST","/api/v1/model-domains",json!({"name":"patents"})).await.unwrap();
        assert_eq!(duplicate["domain"]["id"],key);
        assert_eq!(duplicate["created"],false);
        let body=json!({"domains":["law",key,"law"],"models":[{"upstream_id":"a","model":"chat"},{"upstream_id":"b","model":"chat"}]});
        assert_eq!(call(&g,&alice,"PATCH","/api/v1/models/domains",body).await.unwrap()["updated"],2);
        // Rediscovery replaces native metadata, but not user annotations.
        owned(&g,"a","alice");
        let r=Registry::open(&g.state_path).unwrap();
        assert_eq!(r.domain_catalog[key],"Patents");
        assert_eq!(r.model_domains["a/chat"],vec!["law",key]);
        assert_eq!(r.model_domains["b/chat"],vec!["law",key]);
        let response=proxy::handle(g.clone(),alice.clone(),Request::builder().uri("/v1/models").body(Body::empty()).unwrap()).await.unwrap();
        let discovery:Value=serde_json::from_slice(&axum::body::to_bytes(response.into_body(),1<<20).await.unwrap()).unwrap();
        assert!(discovery["data"].as_array().unwrap().iter().filter(|m|m["id"]!="auto").all(|m|m["domains"]==json!(["law",key])));
        let catalog=call(&g,&user("bob"),"GET","/api/v1/model-domains",json!({})).await.unwrap();
        assert!(catalog["domains"].as_array().unwrap().iter().any(|d|d["id"]==key));
    }
    #[tokio::test]
    async fn batch_is_atomic_and_cannot_modify_another_owner_or_use_unknown_categories() {
        let root=temp();let g=gateway(&root,Router::new());let alice=user("alice");
        owned(&g,"a","alice");owned(&g,"b","bob");
        let batch=json!({"domains":["law"],"models":[{"upstream_id":"a","model":"chat"},{"upstream_id":"b","model":"chat"}]});
        assert!(call(&g,&alice,"PATCH","/api/v1/models/domains",batch).await.is_err());
        assert!(g.registry.lock().unwrap().model_domains.is_empty());
        for invalid in [json!(["missing"]),json!([42]),Value::Null,json!("law")] {
            assert!(call(&g,&alice,"PATCH","/api/v1/models",json!({"upstream_id":"a","model":"chat","domains":invalid})).await.is_err());
        }
        call(&g,&alice,"PATCH","/api/v1/models",json!({"upstream_id":"a","model":"chat","domains":["physics"]})).await.unwrap();
        call(&g,&alice,"PATCH","/api/v1/models",json!({"upstream_id":"a","model":"chat","domains":[]})).await.unwrap();
        assert_eq!(g.registry.lock().unwrap().model_domains["a/chat"],vec!["general"]);
        assert!(!g.registry.lock().unwrap().model_domains.contains_key("b/chat"));
        let unverified=principal("alice");
        assert!(call(&g,&unverified,"PATCH","/api/v1/models",json!({"upstream_id":"a","model":"chat","domains":["law"],"kind":"chat"})).await.is_err());
        let mut key=alice.clone();key.api_key_id=Some("client-key".into());
        assert!(call(&g,&key,"POST","/api/v1/model-domains",json!({"name":"forbidden"})).await.is_err());
        assert!(call(&g,&Identity::default(),"POST","/api/v1/model-domains",json!({"name":"forbidden"})).await.is_err());
    }
    #[tokio::test]
    async fn failed_persistence_does_not_publish_a_category_or_partial_batch() {
        let root=temp();let g=gateway(&root,Router::new());let alice=user("alice");
        owned(&g,"a","alice");std::fs::create_dir(&g.state_path).unwrap();
        assert!(call(&g,&alice,"POST","/api/v1/model-domains",json!({"name":"unsaved"})).await.is_err());
        assert!(g.registry.lock().unwrap().domain_catalog.is_empty());
        assert!(call(&g,&alice,"PATCH","/api/v1/models",json!({"model":"chat","upstream_id":"a","domains":["law"]})).await.is_err());
        assert!(g.registry.lock().unwrap().model_domains.is_empty());
    }
    #[tokio::test]
    async fn registration_uses_shared_vocabulary_and_omission_preserves_user_edit() {
        let root=temp();let g=gateway(&root,Router::new());let alice=user("alice");
        let mut body=json!({"service_id":"domains-fixture","base_url":"http://inference.fixture:8000/v1","protocol":"openai","domains":["physics"],"models":[{"id":"chat","endpoints":["chat_completions"],"domains":["law"]},{"id":"other","endpoints":["chat_completions"]}]});
        let registered=call(&g,&alice,"POST","/api/v1/registrations",body.clone()).await.unwrap();
        let key=registered["upstream_id"].as_str().unwrap();
        assert_eq!(g.registry.lock().unwrap().model_domains[&format!("{key}/chat")],vec!["law"]);
        assert_eq!(g.registry.lock().unwrap().model_domains[&format!("{key}/other")],vec!["physics"]);
        call(&g,&alice,"PATCH","/api/v1/models",json!({"upstream_id":key,"model":"chat","domains":["medicine"]})).await.unwrap();
        body.as_object_mut().unwrap().remove("domains");body["models"][0].as_object_mut().unwrap().remove("domains");
        call(&g,&alice,"POST","/api/v1/registrations",body).await.unwrap();
        assert_eq!(g.registry.lock().unwrap().model_domains[&format!("{key}/chat")],vec!["medicine"]);
    }
    #[tokio::test]
    async fn domain_routes_are_dispatched_with_vos_or_discovery_key_auth_and_reject_anonymous_writes() {
        use sha2::{Digest,Sha256};
        let root=temp();let g=gateway(&root,Router::new());let alice=user("alice");owned(&g,"a","alice");
        let mut key_identity=alice.clone();key_identity.api_key_id=Some("scoped-key".into());key_identity.vos_user=false;
        for (token,identity) in [("domain-user",alice),("domain-reader",key_identity)] {
            g.identities.lock().unwrap().insert(format!("{:x}",Sha256::digest(token)),(identity,std::time::Instant::now()));
        }
        let request=|method:&str,path:&str,token:&str,body:Value|Request::builder().method(method).uri(path).header("authorization",format!("Bearer {token}")).header("content-type","application/json").body(Body::from(body.to_string())).unwrap();
        assert_eq!(dispatch(g.clone(),Request::builder().uri("/api/v1/model-domains").body(Body::empty()).unwrap()).await.err().unwrap().status,401);
        assert_eq!(dispatch(g.clone(),request("GET","/api/v1/model-domains","domain-reader",json!({}))).await.unwrap().status(),200);
        assert_eq!(dispatch(g.clone(),request("POST","/api/v1/model-domains","domain-reader",json!({"name":"denied"}))).await.err().unwrap().status,403);
        assert_eq!(dispatch(g.clone(),request("POST","/api/v1/model-domains","domain-user",json!({"name":"Patents"}))).await.unwrap().status(),201);
        assert_eq!(dispatch(g.clone(),request("PATCH","/api/v1/models/domains","domain-user",json!({"models":[{"model":"chat","upstream_id":"a"}],"domains":["law"]}))).await.unwrap().status(),200);
    }

}

/// A bounded view of the current step. New user turns supersede older topic
/// words; tool continuations retain a small tail as classification context.
pub(super) fn step_text(body: &Value) -> String {
    let rows=body["messages"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let chosen=if rows.last().is_some_and(|m|m["role"]=="user") {&rows[rows.len()-1..]} else {&rows[rows.len().saturating_sub(3)..]};
    let mut text=String::new();
    for row in chosen {
        let value=match &row["content"] {
            Value::String(s)=>s.clone(),
            Value::Array(parts)=>parts.iter().filter_map(|p|p["text"].as_str().or_else(||p["content"].as_str())).collect::<Vec<_>>().join("\n"),
            _=>String::new(),
        };
        text.extend(value.chars().take(8000));text.push('\n');
        if let Some(calls)=row["tool_calls"].as_array(){for call in calls {text.extend(format!("{} {}",call["function"]["name"].as_str().unwrap_or(""),call["function"]["arguments"].as_str().unwrap_or("")).chars().take(2000));}}
    }
    text.to_lowercase()
}
pub(super) fn classify_step(r: &Registry, body: &Value) -> Vec<String> {
    let text=step_text(body);
    let keywords:&[(&str,&[&str])]=&[
        ("law",&["法律","合同","诉讼","法条","专利","legal","lawsuit","contract","patent law"]),
        ("mathematics",&["数学","方程","微积分","概率","math","mathematics","algebra","equation","calculus"]),
        ("physics",&["物理","量子","力学","physics","quantum","mechanics"]),
        ("medicine",&["医学","医药","药物","诊断","临床","medical","medicine","clinical","pharmac"]),
        ("chemistry",&["化学","分子","chemistry","chemical","molecule"]),
        ("biology",&["生物","基因","biology","genetic","genome"]),
        ("engineering",&["工程设计","engineering"]),
        ("computing",&["编程","代码","debug","programming","python","rust","javascript","typescript"]),
        ("finance",&["金融","财务","投资","finance","financial","investment"]),
        ("education",&["教学","教育","课程","education","curriculum"]),
        ("humanities",&["人文","历史","哲学","humanities","history","philosophy"]),
    ];
    let words=format!(" {} ",text.split(|c:char|!c.is_alphanumeric()).filter(|s|!s.is_empty()).collect::<Vec<_>>().join(" "));
    let mut ids=keywords.iter().filter(|(_,terms)|terms.iter().any(|term| {
        if !term.is_ascii(){text.contains(term)}else if *term=="pharmac"{words.split_whitespace().any(|w|w.starts_with(term))}else{words.contains(&format!(" {term} "))}
    })).map(|(id,_)|(*id).to_owned()).collect::<Vec<_>>();
    for (id,name) in &r.domain_catalog {
        let name=name.to_lowercase();
        let matched=if name.chars().all(|c|c.is_ascii_alphanumeric()||c.is_ascii_whitespace()){words.contains(&format!(" {} ",name.split_whitespace().collect::<Vec<_>>().join(" ")))}else{text.contains(&name)};
        if matched {ids.push(id.clone());}
    }
    ids.truncate(32);if ids.is_empty(){ids.push("general".into());}ids
}

#[cfg(test)]
mod auto_domain_tests {
    use super::*;
    use super::super::tests::{gateway,principal,temp,upstream,model};
    #[test]
    fn auto_and_real_models_assign_atomically_with_admin_only_auto_and_custom_rules() {
        let root=temp();let g=gateway(&root,Router::new());
        let mut u=upstream("a");u.owner="alice".into();g.registry.lock().unwrap().put(u,vec![model("chat")]);
        let user=Identity{vos_user:true,..principal("alice")};
        let p=json!({"models":[{"upstream_id":"a","model":"chat"},{"upstream_id":"auto","model":"auto"}],"domains":["law"]});
        assert_eq!(batch(&g,&user,&p).unwrap_err().status,403);
        assert!(g.registry.lock().unwrap().model_domains.is_empty());
        let admin=Identity{admin:true,..user};batch(&g,&admin,&p).unwrap();
        assert_eq!(g.registry.lock().unwrap().auto.domains,vec!["law"]);
        assert_eq!(g.registry.lock().unwrap().auto.revision,1);
        single(&g,&admin,&json!({"upstream_id":"auto","model":"auto","domains":["law"]})).unwrap();
        assert_eq!(g.registry.lock().unwrap().auto.revision,1);
        let r=Registry::open(&g.state_path).unwrap();assert_eq!(r.auto.domains,vec!["law"]);
        let mut r=r;r.domain_catalog.insert("custom-test".into(),"Astronomy".into());
        assert_eq!(classify_step(&r,&json!({"messages":[{"role":"user","content":"Astronomy"}]})),vec!["custom-test"]);
        assert_eq!(classify_step(&r,&json!({"messages":[{"role":"user","content":"astronomydummy"}]})),vec!["general"]);
        std::fs::remove_dir_all(root).unwrap();
    }
}
