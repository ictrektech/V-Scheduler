//! Personal inference credentials. Secrets are returned once; only hashes persist.
use super::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::os::unix::fs::OpenOptionsExt;

#[derive(Clone, Serialize, Deserialize)]
struct Key {
    id: String,
    owner: String,
    name: String,
    prefix: String,
    hash: String,
    #[serde(default)]
    models: Option<Vec<String>>,
    created_at: DateTime<Utc>,
    expires_at: Option<DateTime<Utc>>,
    revoked_at: Option<DateTime<Utc>>,
}
impl Key {
    fn active(&self) -> bool {
        self.revoked_at.is_none() && self.expires_at.is_none_or(|at| at > Utc::now())
    }
    fn view(&self) -> Value {
        json!({"id":self.id,"name":self.name,"prefix":self.prefix,"created_at":self.created_at,
            "models":self.models,"expires_at":self.expires_at,"revoked_at":self.revoked_at,"active":self.active()})
    }
}
#[derive(Clone, Default, Serialize, Deserialize)]
pub(super) struct Store {
    keys: Vec<Key>,
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    pub(super) authority: String,
    #[serde(skip)]
    pub(super) context: PathBuf,
}
impl Store {
    pub(super) fn open(state: &Path) -> anyhow::Result<Self> {
        let path = state.with_file_name("api-keys.json");
        let mut store: Self = if path.try_exists()? {
            serde_json::from_value(private_json(&path, 4 << 20)?)?
        } else { Self::default() };
        anyhow::ensure!(store.keys.len() <= 4096 && store.keys.iter().all(|k|
            !k.owner.is_empty() && k.hash.len() == 64 && k.hash.bytes().all(|c| c.is_ascii_hexdigit())),
            "invalid personal API key store");
        store.path = path;
        store.authority = auth::userinfo().and_then(|url| reqwest::Url::parse(&url).ok())
            .and_then(|url| url.join("/v1000/user/list").ok()).map(|url|url.to_string()).unwrap_or_default();
        store.context = PathBuf::from("/var/lib/vos-context/access-token");
        Ok(store)
    }
    fn save(&self) -> Api<()> {
        let persist = || -> anyhow::Result<()> {
            let parent = self.path.parent().ok_or_else(||anyhow::anyhow!("missing key directory"))?;
            std::fs::create_dir_all(parent)?;
            let temporary = parent.join(format!(".api-keys-{}.tmp", uuid::Uuid::new_v4()));
            let result = (|| -> anyhow::Result<()> {
                let mut file = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&temporary)?;
                file.write_all(&serde_json::to_vec(self)?)?;
                file.sync_all()?;
                std::fs::rename(&temporary, &self.path)?;
                Ok(())
            })();
            if result.is_err() { let _ = std::fs::remove_file(&temporary); }
            result
        };
        persist().map_err(|_|Failure::new(500,"cannot persist API keys"))
    }
}

/// Resolve the owner's current enabled/deleted state and roles, never an expiring
/// token saved at creation time. The platform app context is supplied by VOS.
pub(super) async fn owner(g: &Gateway, owner: &str) -> Api<Identity> {
    let (authority, context) = {let store=g.api_keys.lock().unwrap();(store.authority.clone(),store.context.clone())};
    let mut url = reqwest::Url::parse(&authority).map_err(|_|Failure::new(503,"VOS identity service is unavailable"))?;
    url.query_pairs_mut().append_pair("page_num","1").append_pair("page_size","2").append_pair("user_id",owner);
    let token = std::fs::read_to_string(context).map_err(|_|Failure::new(503,"VOS identity service is unavailable"))?;
    let response = g.client.get(url).bearer_auth(token.trim()).timeout(Duration::from_secs(5)).send().await
        .map_err(|_|Failure::new(503,"VOS identity service is unavailable"))?;
    if !response.status().is_success() { return Err(Failure::new(503,"VOS identity service is unavailable")); }
    let result = read_json(response,1 << 20).await.map_err(|_|Failure::new(503,"invalid VOS account response"))?;
    if result["code"] != 0 { return Err(Failure::new(503,"VOS account verification failed")); }
    let rows=result["data"]["data"].as_array().ok_or_else(||Failure::new(503,"invalid VOS account response"))?;
    let user=rows.iter().find(|u|text(u,"id")==owner).ok_or_else(||Failure::new(401,"API key owner is unavailable"))?;
    if !(user["enable"]==1 || user["enable"]==true) || !(user["delete"]==0 || user["delete"]==false) {
        return Err(Failure::new(401,"API key owner is disabled or deleted"));
    }
    Ok(Identity {owner:owner.into(),username:text(user,"username").into(),email:text(user,"email").into(),
        admin:crate::identity::administrator(&json!({"code":0,"data":user})),..Default::default()})
}

fn secret() -> Api<String> {
    let mut random=[0u8;32];
    std::fs::File::open("/dev/urandom").and_then(|mut f|f.read_exact(&mut random))
        .map_err(|_|Failure::new(500,"cannot generate API key"))?;
    Ok(format!("vs-key-{}",random.iter().map(|b|format!("{b:02x}")).collect::<String>()))
}

/// Update model access credentials independently of the upstream's provider key.
/// Null revokes, true generates, a string sets a caller-supplied secret.
pub(super) fn configure_models(g: &Gateway, owner: &str, settings: Vec<(String,Value)>) -> Api<Value> {
    let mut store=g.api_keys.lock().unwrap();
    let mut next=store.clone();
    let mut output=vec![];
    for (model,value) in settings {
        let generated=value==true;
        let secret=if generated {Some(secret()?)} else if value.is_null() {None}
            else {Some(value.as_str().filter(|s|s.len()>=16 && s.len()<=512 && s.bytes().all(|c|c.is_ascii_graphic()))
                .ok_or_else(||Failure::new(422,"access_key must be null, true, or a 16..512 character secret"))?.to_owned())};
        let hash=secret.as_ref().map(|s|format!("{:x}",Sha256::digest(s)));
        if hash.as_ref().is_some_and(|hash|next.keys.iter().any(|k|safe_secret_eq(&k.hash,hash) && (k.owner!=owner || k.models.is_none() || !k.active()))) {
            return Err(Failure::new(409,"access key is already used or revoked; choose a new secret"));
        }
        // A model has one active model credential. Rotating it leaves personal
        // credentials and other models sharing the previous key intact.
        for key in &mut next.keys {
            if let Some(models)=&mut key.models {
                models.retain(|target|target!=&model);
                if models.is_empty() && key.revoked_at.is_none() {key.revoked_at=Some(Utc::now());}
            }
        }
        if let (Some(hash),Some(secret))=(hash,secret) {
            let existing=next.keys.iter_mut().find(|k|k.owner==owner && safe_secret_eq(&k.hash,&hash));
            let view=if let Some(key)=existing {
                key.models.as_mut().unwrap().push(model.clone());key.revoked_at=None;key.view()
            } else {
                if next.keys.len()>=4096 {return Err(Failure::new(409,"API key store is full"));}
                let key=Key {id:uuid::Uuid::new_v4().to_string(),owner:owner.into(),name:format!("Model: {model}"),prefix:if secret.starts_with("vs-key-") && secret.len()==71 {secret[..15].into()} else {format!("custom-{}",&hash[..8])},hash,
                    models:Some(vec![model.clone()]),created_at:Utc::now(),expires_at:None,revoked_at:None};
                let view=key.view();next.keys.push(key);view
            };
            let mut row=json!({"model":model,"key":view});
            if generated {row["api_key"]=json!(secret);}
            output.push(row);
        } else {output.push(json!({"model":model,"key":null}));}
    }
    next.save()?;*store=next;
    Ok(json!({"model_access_keys":output}))
}

pub(super) async fn model_access(g: &Gateway, identity: &Identity, request: Request) -> Api<Response> {
    if !identity.vos_user || identity.api_key_id.is_some() {return Err(Failure::new(403,"model access key management requires a VOS account token"));}
    let bytes=axum::body::to_bytes(request.into_body(),8192).await.map_err(|_|Failure::new(413,"model key request too large"))?;
    let payload:Value=serde_json::from_slice(&bytes).map_err(|_|Failure::new(422,"invalid model key request"))?;
    let target={
        let r=g.registry.lock().unwrap();
        let u=r.upstreams.get(text(&payload,"upstream_id")).ok_or_else(||Failure::new(404,"upstream not found"))?;
        if !(identity.admin || !u.owner.is_empty() && u.owner==identity.owner) {return Err(Failure::new(403,"only the owner or an administrator can set model access keys"));}
        let m=r.model_by_id(u,text(&payload,"model"),identity).ok_or_else(||Failure::new(404,"model not found"))?;
        (if u.owner.is_empty(){identity.owner.clone()}else{u.owner.clone()},format!("{}/{}",u.id,m.id),u.owner.clone())
    };
    let value=payload.get("access_key").ok_or_else(||Failure::new(422,"access_key is required"))?.clone();
    let current_owner=owner(g,&target.0).await?;
    if target.2.is_empty() && !current_owner.admin {return Err(Failure::new(403,"unowned model keys require a current VOS administrator"));}
    let result={
        let r=g.registry.lock().unwrap();
        let u=r.upstreams.get(text(&payload,"upstream_id")).filter(|u|u.owner==target.2)
            .ok_or_else(||Failure::new(409,"model ownership changed; refresh and retry"))?;
        if r.model_by_id(u,text(&payload,"model"),identity).is_none(){return Err(Failure::new(404,"model not found"));}
        configure_models(g,&target.0,vec![(target.1,value)])?
    };
    let mut response=axum::Json(result).into_response();response.headers_mut().insert("cache-control","no-store".parse().unwrap());Ok(response)
}

pub(super) fn known(g: &Gateway, secret: &str) -> bool {
    let hash=format!("{:x}",Sha256::digest(secret));
    g.api_keys.lock().unwrap().keys.iter().any(|k|safe_secret_eq(&k.hash,&hash))
}
pub(super) async fn authenticate(g: &Gateway, secret: &str) -> Api<Identity> {
    if secret.len()>512 { return Err(Failure::new(401,"invalid API key")); }
    let hash=format!("{:x}",Sha256::digest(secret));
    let key={let store=g.api_keys.lock().unwrap();store.keys.iter().find(|k|k.active() && safe_secret_eq(&k.hash,&hash)).cloned()}
        .ok_or_else(||Failure::new(401,"invalid or revoked API key"))?;
    let mut identity=owner(g,&key.owner).await?;
    // Recheck after the platform call: revocation may race account verification.
    let current=g.api_keys.lock().unwrap().keys.iter().find(|k|k.id==key.id && k.active()).cloned()
        .ok_or_else(||Failure::new(401,"invalid or revoked API key"))?;
    identity.api_key_id=Some(current.id);
    identity.key_models=current.models;
    Ok(identity)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Create {
    name: String,
    #[serde(default)]
    expires_in_days: u32,
}
pub(super) async fn handle(g: &Gateway, identity: &Identity, request: Request) -> Api<Response> {
    if !identity.vos_user || identity.api_key_id.is_some() {
        return Err(Failure::new(403,"API key management requires a VOS account token"));
    }
    let path=request.uri().path().to_owned();
    let method=request.method().clone();
    let result = if path=="/api/v1/api-keys" && method=="GET" {
        json!({"keys":g.api_keys.lock().unwrap().keys.iter().filter(|k|k.owner==identity.owner).map(Key::view).collect::<Vec<_>>()})
    } else if path=="/api/v1/api-keys" && method=="POST" {
        let bytes=axum::body::to_bytes(request.into_body(),4096).await.map_err(|_|Failure::new(413,"API key request too large"))?;
        let value:Create=serde_json::from_slice(&bytes).map_err(|_|Failure::new(422,"name and optional expires_in_days are required"))?;
        let name=value.name.trim();
        if name.is_empty() || name.chars().count()>80 || name.chars().any(char::is_control) || value.expires_in_days>3650 {
            return Err(Failure::new(422,"invalid API key name or expiry"));
        }
        owner(g,&identity.owner).await?;
        let secret=secret()?;
        let key=Key {id:uuid::Uuid::new_v4().to_string(),owner:identity.owner.clone(),name:name.into(),prefix:secret[..15].into(),
            hash:format!("{:x}",Sha256::digest(&secret)),models:None,created_at:Utc::now(),revoked_at:None,
            expires_at:if value.expires_in_days==0 {None} else {Some(Utc::now()+chrono::Duration::days(value.expires_in_days.into()))}};
        let mut store=g.api_keys.lock().unwrap();
        if store.keys.len()>=4096 || store.keys.iter().filter(|k|k.owner==identity.owner && k.active()).count()>=32 {
            return Err(Failure::new(409,"API key limit reached; revoke unused keys"));
        }
        let mut next=store.clone();next.keys.push(key.clone());next.save()?;*store=next;
        json!({"api_key":secret,"key":key.view()})
    } else if method=="DELETE" {
        let id=path.strip_prefix("/api/v1/api-keys/").ok_or_else(||Failure::new(404,"API key not found"))?;
        let mut store=g.api_keys.lock().unwrap();let mut next=store.clone();
        let key=next.keys.iter_mut().find(|k|k.id==id && k.owner==identity.owner).ok_or_else(||Failure::new(404,"API key not found"))?;
        if key.revoked_at.is_none(){key.revoked_at=Some(Utc::now());}
        let value=key.view();next.save()?;*store=next;json!({"key":value})
    } else {return Err(Failure::new(405,"method not allowed"));};
    let mut response=axum::Json(result).into_response();
    response.headers_mut().insert("cache-control","no-store".parse().unwrap());
    Ok(response)
}
