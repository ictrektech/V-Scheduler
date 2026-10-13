use crate::model::Principal;
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;
use std::time::Duration;

pub fn base() -> Option<String> {
    std::env::var("SCHEDULER_VOS_URL").ok().or_else(|| {
        std::env::var("VOS_HOST_GW_IP")
            .ok()
            .map(|host| format!("http://{host}:8105"))
    })
}

pub fn administrator(v: &Value) -> bool {
    let data = v.get("data").unwrap_or(v);
    v.get("code").is_none_or(|c| c == 0)
        && data["role_code"] == "system_admin"
        && (data["enable"] == 1 || data["enable"] == true)
        && data["delete"] != 1
        && data["delete"] != true
        && data["id"]
            .as_str()
            .or(data["sub"].as_str())
            .is_some_and(|s| !s.is_empty())
}

pub async fn verify(h: &HeaderMap) -> Result<Principal, StatusCode> {
    let base = base().ok_or(StatusCode::UNAUTHORIZED)?;
    let bearer = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .filter(|v| v.starts_with("Bearer ") && v.len() > 7)
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(5))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let response = client
        .get(format!("{}/v1000/user/check", base.trim_end_matches('/')))
        .header("Authorization", bearer)
        .send()
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    if !response.status().is_success() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let v: Value = response
        .json()
        .await
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    if !administrator(&v) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Principal {
        token: String::new(),
        role: "admin".into(),
        app_id: "vos-administrator".into(),
        node_id: String::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn only_enabled_verified_platform_administrators_can_manage() {
        let good =
            json!({"code":0,"data":{"id":"u1","role_code":"system_admin","enable":1,"delete":0}});
        assert!(administrator(&good));
        for (key, value) in [
            ("role_code", json!("user")),
            ("enable", json!(0)),
            ("delete", json!(1)),
            ("id", json!("")),
        ] {
            let mut bad = good.clone();
            bad["data"][key] = value;
            assert!(!administrator(&bad));
        }
        let mut rejected = good;
        rejected["code"] = json!(10001);
        assert!(!administrator(&rejected));
        assert!(!administrator(&json!({"username":"admin"})));
    }
}
