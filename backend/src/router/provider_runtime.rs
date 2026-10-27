//! Opt-in pi-ai boundary. One isolated child per operation keeps provider
//! credentials out of process arguments and prevents cross-owner SDK env reuse.
use super::*;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use std::collections::HashMap;
use std::sync::{OnceLock, Weak};

pub(super) fn validate(provider: &str, credential: &Value) -> Api<()> {
    let ids: Vec<String> = serde_json::from_str(include_str!("../../../providers/catalog.json")).unwrap();
    if !ids.iter().any(|v| v == provider) { return Err(Failure::new(400,"unknown native provider")); }
    if !credential.is_null() && !credential.is_object() { return Err(Failure::new(400,"provider_credentials must be an object")); }
    if let Some(kind)=credential.get("type") {
        if !matches!(kind.as_str(),Some("api_key"|"oauth")) {return Err(Failure::new(400,"provider credential type must be api_key or oauth"));}
    }
    if credential["type"] == "oauth" && (!credential["access"].is_string() || !credential["refresh"].is_string() || !credential["expires"].is_number()) {
        return Err(Failure::new(400,"OAuth credentials require access, refresh and expires"));
    }
    if let Some(values)=credential.get("env") {
        if !values.is_object() || values.as_object().unwrap().values().any(|v|!v.is_string()) {return Err(Failure::new(400,"provider environment values must be strings"));}
    }
    Ok(())
}
fn scope(u: &Upstream) -> String {
    let raw=json!([u.owner,u.base_url,u.provider,u.api_key,u.provider_credentials]);
    format!("{:x}",Sha256::digest(serde_json::to_vec(&raw).unwrap()))
}
fn credential_path(g: &Gateway,u:&Upstream)->PathBuf {g.state_path.parent().unwrap_or(Path::new(".")).join("provider-auth").join(format!("{}.json",scope(u)))}
pub(super) fn forget(g: &Gateway,u:&Upstream) {
    let shared=g.registry.lock().unwrap().upstreams.values().any(|other|other.id!=u.id && scope(other)==scope(u));
    if !shared {let _=std::fs::remove_file(credential_path(g,u));}
}
fn auth_lock(key:String)->Arc<tokio::sync::Mutex<()>> {
    static LOCKS: OnceLock<Mutex<HashMap<String,Weak<tokio::sync::Mutex<()>>>>>=OnceLock::new();
    let mut locks=LOCKS.get_or_init(Default::default).lock().unwrap();
    locks.retain(|_,v|v.strong_count()>0);
    if let Some(lock)=locks.get(&key).and_then(Weak::upgrade){return lock;}
    let lock=Arc::new(tokio::sync::Mutex::new(()));locks.insert(key,Arc::downgrade(&lock));lock
}
pub(super) async fn response(g:&Gateway,u:&Upstream,operation:&str,body:Value)->Api<reqwest::Response> {
    validate(&u.provider,&u.provider_credentials)?;
    let lock=auth_lock(scope(u));let mut guard=Some(lock.lock().await);
    let worker=env("V_SCHEDULER_PROVIDER_RUNTIME","/opt/v-scheduler/providers/worker.mjs");
    let mut child=tokio::process::Command::new(env("V_SCHEDULER_PROVIDER_NODE","node"))
        .arg(worker).env_clear().env("PATH","/usr/local/bin:/usr/bin:/bin")
        .env("HOME","/nonexistent").env("NODE_ENV","production")
        .stdin(std::process::Stdio::piped()).stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null()).kill_on_drop(true).spawn()
        .map_err(|_|Failure::new(503,"native provider runtime unavailable"))?;
    let input=json!({"operation":operation,"provider":u.provider,"base_url":u.base_url,
        "api_key":u.api_key,"credential":u.provider_credentials,"credential_path":credential_path(g,u),"body":body});
    let mut stdin=child.stdin.take().unwrap();
    stdin.write_all(&serde_json::to_vec(&input).unwrap()).await.map_err(|_|Failure::new(502,"native provider input failed"))?;
    drop(stdin);
    let mut reader=BufReader::new(child.stdout.take().unwrap());
    let header=tokio::time::timeout(Duration::from_secs(120),async {
        loop {
            let mut line=String::new();
            if reader.read_line(&mut line).await.map_err(|_|Failure::new(502,"native provider response failed"))?==0 {return Err(Failure::new(502,"native provider exited before response"));}
            let frame:Value=serde_json::from_str(&line).map_err(|_|Failure::new(502,"invalid native provider response"))?;
            if frame["ready"]==true {guard.take();continue;}
            return Ok(frame);
        }
    }).await.map_err(|_|Failure::new(504,"native provider response timed out"))??;
    drop(guard);
    let status=header["status"].as_u64().filter(|v|(100..=599).contains(v)).unwrap_or(502) as u16;
    let content_type=if header["content_type"]=="text/event-stream" {"text/event-stream"}else{"application/json"};
    let stream=futures_util::stream::unfold((reader,child),|(mut reader,mut child)|async move {
        let mut buffer=vec![0u8;16*1024];
        match reader.read(&mut buffer).await {
            Ok(0)=>{let _=child.wait().await;None},
            Ok(n)=>{buffer.truncate(n);Some((Ok::<_,std::io::Error>(buffer),(reader,child)))},
            Err(e)=>Some((Err(e),(reader,child))),
        }
    });
    let response=axum::http::Response::builder().status(status).header("content-type",content_type)
        .body(reqwest::Body::wrap_stream(stream)).unwrap();
    Ok(reqwest::Response::from(response))
}
pub(super) fn upstream_failure(status:u16,message:&str)->Failure {
    let mut error=Failure::new(status,message);
    error.code=Some(match status {401|403=>"PROVIDER_AUTH_REQUIRED",429=>"PROVIDER_RATE_LIMITED",_=>"PROVIDER_ERROR"}.into());error
}
pub(super) async fn models(g:&Gateway,u:&Upstream)->Api<Vec<Model>> {
    let response=response(g,u,"models",json!({})).await?;
    if !response.status().is_success(){return Err(upstream_failure(response.status().as_u16(),"native provider discovery failed; verify credentials and configuration"));}
    let value=read_json(response,4<<20).await?;
    serde_json::from_value(value["models"].clone()).map_err(|_|Failure::new(502,"invalid native provider catalog"))
}
