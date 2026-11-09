use super::*;
fn spec() -> Value { serde_json::from_str(include_str!("../../../api/router-openapi-en.json")).unwrap() }
fn keys(value: &Value) -> std::collections::BTreeSet<String> { value.as_object().unwrap().keys().cloned().collect() }
#[test]
fn documented_routing_and_auto_fields_match_serialized_contracts() {
    let spec = spec();
    let schema = |path: &str| spec["paths"][path]["put"]["requestBody"]["content"]["application/json"]["schema"].clone();
    assert_eq!(keys(&schema("/api/v1/upstreams/{id}/routing")["properties"]), keys(&serde_json::to_value(registry::Routing::default()).unwrap()));
    let auto = schema("/api/v1/auto-routing");
    assert_eq!(keys(&auto["properties"]), keys(&serde_json::to_value(auto::Config::default()).unwrap()));
    assert_eq!(keys(&auto["properties"]["candidates"]["items"]["properties"]), keys(&serde_json::to_value(auto::Candidate::default()).unwrap()));
    for (key, value) in serde_json::to_value(auto::Config::default()).unwrap().as_object().unwrap() {
        if key != "request_type_tiers" { assert_eq!(&auto["properties"][key]["default"], value, "default for {key}"); }
    }
    for path in ["/api/v1/registrations", "/api/v1/registrations/{id}/heartbeat"] {
        assert_eq!(spec["paths"][path]["post"]["requestBody"]["content"]["application/json"]["schema"]["properties"]["scheduler"]["properties"]["mode"]["enum"], json!(manage::REGISTRATION_SCHEDULER_MODES));
    }
    assert_eq!(spec["components"]["parameters"]["SchedulingPriority"]["schema"]["default"], json!("interactive"));
}
