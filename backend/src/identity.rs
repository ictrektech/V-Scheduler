use crate::model::Principal;
use axum::http::{HeaderMap, StatusCode};
use serde_json::Value;
use std::time::Duration;

pub fn base() -> Option<String> {
    if let Ok(raw) = std::env::var("SCHEDULER_VOS_URL") {
        let raw = raw.trim();
        if !raw.is_empty() {
            let url = reqwest::Url::parse(raw).ok()?;
            return (matches!(url.scheme(), "http" | "https")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.path() == "/")
                .then(|| raw.trim_end_matches('/').to_string());
        }
    }
    std::env::var("VOS_HOST_GW_IP")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|host| format!("http://{host}:8105"))
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

pub fn bearer(h: &HeaderMap) -> Option<&str> {
    let mut fields = h.get("authorization")?.to_str().ok()?.split_whitespace();
    let scheme = fields.next()?;
    let token = fields.next()?;
    (scheme.eq_ignore_ascii_case("Bearer") && fields.next().is_none()).then_some(token)
}

pub async fn verify(h: &HeaderMap) -> Result<Principal, StatusCode> {
    let base = base().ok_or(StatusCode::UNAUTHORIZED)?;
    let token = bearer(h).ok_or(StatusCode::UNAUTHORIZED)?;
    let bearer = format!("Bearer {token}");
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
    validate_user_check(&v)?;
    if !administrator(&v) {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(Principal {
        token: String::new(),
        role: "admin".into(),
        app_id: "vos-administrator".into(),
        node_id: String::new(),
        delegated_executors: Vec::new(),
        permit_executors: Vec::new(),
    })
}

// VOS may return HTTP 200 with a failed business envelope for an expired session.
fn validate_user_check(value: &Value) -> Result<(), StatusCode> {
    match value.get("code").and_then(Value::as_i64) {
        Some(0) | None => Ok(()),
        Some(10002) => Err(StatusCode::FORBIDDEN),
        Some(10001 | 10099) => Err(StatusCode::UNAUTHORIZED),
        Some(_) => Err(StatusCode::SERVICE_UNAVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn bearer_syntax_matches_gateway() {
        for value in [
            "Bearer token",
            "bearer token",
            "BEARER token",
            "Bearer   token",
        ] {
            let mut h = HeaderMap::new();
            h.insert("authorization", value.parse().unwrap());
            assert_eq!(bearer(&h), Some("token"));
        }
        for value in ["Basic token", "Bearer", "Bearer token extra"] {
            let mut h = HeaderMap::new();
            h.insert("authorization", value.parse().unwrap());
            assert_eq!(bearer(&h), None);
        }
    }
    #[test]
    fn expired_sessions_are_not_permission_denials() {
        for code in [10001, 10099] {
            assert_eq!(
                validate_user_check(&json!({"code":code})),
                Err(StatusCode::UNAUTHORIZED)
            );
        }
        assert_eq!(
            validate_user_check(&json!({"code":10002})),
            Err(StatusCode::FORBIDDEN)
        );
        assert_eq!(
            validate_user_check(&json!({"code":10003})),
            Err(StatusCode::SERVICE_UNAVAILABLE)
        );
    }
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
