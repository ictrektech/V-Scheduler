use super::*;
fn userinfo() -> Option<String> {
    let explicit = env("VOS_OIDC_USERINFO_URL", "");
    if !explicit.is_empty() {
        return Some(explicit);
    }
    crate::identity::base()
        .map(|base| format!("{base}/v1000/oauth2/userinfo"))
        .or_else(|| {
            std::env::var("SCHEDULER_VOS_URL")
                .is_err()
                .then(|| "http://172.17.0.1:8105/v1000/oauth2/userinfo".into())
        })
}
pub(super) async fn probe(g: &Gateway) -> Api<Response> {
    let (reachable, error) = match userinfo() {
        Some(url) => match g
            .client
            .get(url)
            .timeout(Duration::from_secs(5))
            .send()
            .await
        {
            Ok(_) => (true, ""),
            Err(_) => (false, "VOS userinfo endpoint unavailable"),
        },
        None => (false, "VOS origin configuration invalid"),
    };
    Ok(axum::Json(json!({"version":version(),"oidc_enabled":userinfo().is_some() && env("V_ROUTER_AUTH_DISABLED","")!="true","oidc_reachable":reachable,"oidc_error":error})).into_response())
}
pub(super) async fn identity(g: &Gateway, h: &HeaderMap, management: bool) -> Api<Identity> {
    if env("V_ROUTER_AUTH_DISABLED", "") == "true" && management {
        return Ok(Identity {
            owner: "auth-disabled".into(),
            username: "auth-disabled".into(),
            admin: true,
            ..Default::default()
        });
    }
    let bearer = crate::identity::bearer(h);
    let api_key = h.get("x-api-key").and_then(|v| v.to_str().ok());
    if let Some(key) = api_key {
        if h.contains_key("authorization") && bearer != Some(key) {
            return Err(Failure::new(401, "conflicting credentials"));
        }
    }
    let token = bearer.or(api_key);
    let gateway_key = if env("V_ROUTER_REQUIRE_KEY", "") == "true" {
        env("V_ROUTER_API_KEY", "")
    } else {
        String::new()
    };
    let Some(token) = token else {
        if management
            || h.contains_key("authorization")
            || env("V_ROUTER_REQUIRE_KEY", "") == "true"
        {
            return Err(Failure::new(
                401,
                "invalid or missing gateway Key / VOS access token",
            ));
        }
        return Ok(Identity {
            username: "internal".into(),
            ..Default::default()
        });
    };
    if !gateway_key.is_empty() && safe_secret_eq(token, &gateway_key) {
        return Ok(Identity {
            owner: "gateway-admin".into(),
            username: "gateway-admin".into(),
            admin: true,
            ..Default::default()
        });
    }
    if token.starts_with("vs-app-") {
        if management {
            return Err(Failure::new(
                403,
                "application credential is inference only",
            ));
        }
        let snapshot = private_json(
            &g.state_path
                .parent()
                .unwrap()
                .join("app-identities/hybrag.json"),
            4096,
        )
        .map_err(|_| Failure::new(401, "invalid application credential"))?;
        use sha2::{Digest, Sha256};
        let digest = format!("{:x}", Sha256::digest(token));
        if token.len() != 71
            || snapshot["protocol_version"] != 1
            || snapshot["binding_revision"].as_u64().unwrap_or(0) < 1
            || snapshot["app_id"] != "com.ictrek.hybrag"
            || snapshot["scope"] != "gateway-inference-v1"
            || snapshot["enabled"] != true
            || !safe_secret_eq(&digest, text(&snapshot, "token_hash"))
        {
            return Err(Failure::new(401, "invalid application credential"));
        }
        return Ok(Identity {
            owner: "app:com.ictrek.hybrag".into(),
            username: "hybrag-service".into(),
            service_app: "com.ictrek.hybrag".into(),
            ..Default::default()
        });
    }
    use sha2::{Digest, Sha256};
    let key = format!("{:x}", Sha256::digest(token));
    if let Some((id, at)) = g
        .identities
        .lock()
        .unwrap()
        .get(&key)
        .filter(|(_, at)| at.elapsed() < Duration::from_secs(60))
        .cloned()
    {
        let _ = at;
        return Ok(id);
    }
    let url = userinfo().ok_or_else(|| Failure::new(401, "VOS userinfo is not configured"))?;
    let response = g
        .client
        .get(url)
        .bearer_auth(token)
        .timeout(Duration::from_secs(10))
        .send()
        .await
        .map_err(|_| Failure::new(503, "VOS identity service is unavailable"))?;
    if !response.status().is_success() {
        return Err(Failure::new(401, "VOS access token is invalid or expired"));
    }
    let value: Value = read_json(response, 1 << 20)
        .await
        .map_err(|_| Failure::new(401, "invalid VOS identity response"))?;
    if value.get("code").is_some_and(|v| v != 0) {
        return Err(Failure::new(401, "VOS access token is invalid or expired"));
    }
    let value = value.get("data").unwrap_or(&value);
    let owner = text(value, "sub");
    let username = text(value, "preferred_username");
    if owner.is_empty() && username.is_empty() {
        return Err(Failure::new(401, "invalid VOS identity response"));
    }
    let id = Identity {
        owner: if owner.is_empty() { username } else { owner }.into(),
        username: if username.is_empty() { owner } else { username }.into(),
        email: text(value, "email").into(),
        ..Default::default()
    };
    {
        let mut cache = g.identities.lock().unwrap();
        if cache.len() > 512 {
            cache.clear();
        }
        cache.insert(key, (id.clone(), std::time::Instant::now()));
    }
    discovery::vsub(g, token, &id).await;
    Ok(id)
}
