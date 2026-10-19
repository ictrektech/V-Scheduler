use super::download::env;
use serde_json::{Value, json};
pub fn integration() -> Value {
    let mut endpoints = vec![
        json!({"name":"Unified inference","internal":"http://v-scheduler:8080","host_port":env("V_ROUTER_HOST_PORT","22888"),"path":"/v1","authentication":"Gateway API key / VOS account policy"}),
        json!({"name":"Gateway management","internal":"http://v-scheduler:8080","host_port":env("V_ROUTER_HOST_PORT","22888"),"path":"/api/v1","authentication":"Gateway API key / VOS account policy"}),
        json!({"name":"Scheduler management","internal":"http://v-scheduler:38120","host_port":env("SCHEDULER_HOST_PORT","38120"),"path":"/api/v1","authentication":"Scoped service bearer or VOS administrator"}),
        json!({"name":"Model management","internal":"http://v-scheduler:6005","host_port":env("MODEL_HUB_BACKEND_HOST_PORT","36005"),"path":"/api/v1","authentication":"Same API contract as Model Hub; restrict management ports to trusted network"}),
    ];
    // These are declared network aliases of the unified container, not extra services.
    for endpoint in &mut endpoints {
        let (aliases, compatibility): (Vec<&str>, Vec<&str>) = match endpoint["name"].as_str().unwrap_or("") {
            "Unified inference" | "Gateway management" => {
                (vec![], vec!["v-scheduler-gateway:8080", "v-router:8080", "v-router-backend:8080"])
            }
            "Scheduler management" => (
                vec![],
                vec!["v-scheduler-backend:38120", "v-scheduler-frontend:38120"],
            ),
            "Model management" => (vec![], vec!["v-scheduler-model-hub:6005", "v-scheduler-model-hub-backend:6005"]),
            _ => (vec![], vec![]),
        };
        endpoint["aliases"] = json!(aliases);
        endpoint["compatibility_aliases"] = json!(compatibility);
    }
    for (role, api, gateway) in [
        ("qa", "12434", "12535"),
        ("embedding", "12435", "12536"),
        ("rerank", "12436", "12537"),
    ] {
        let upper = role.to_uppercase();
        endpoints.push(json!({"name":format!("Ollama {role} native management"),"internal":format!("http://v-scheduler-ollama-{role}:11434"),"host_port":env(&format!("OLLAMA_{upper}_HOST_PORT"),api),"path":"/api","authentication":"Ollama native API; trusted network"}));
        endpoints.push(json!({"name":format!("Ollama {role} metered gateway"),"internal":format!("http://v-scheduler-ollama-{role}:11535"),"host_port":env(&format!("OLLAMA_{upper}_GATEWAY_HOST_PORT"),gateway),"path":"/v1","authentication":"Ollama gateway policy"}));
    }
    json!({"endpoints":endpoints,"shared_path":env("MODEL_HUB_SHARED_MODELS_PATH","/data/vos_workspace/model_hub"),"internal_storage_root":"/data","api_prefix":"/app/com.ictrek.v-scheduler/model-hub","preloads_default":false})
}
pub fn openapi() -> Value {
    let mut specification: Value =
        serde_json::from_str(include_str!("../../../api/model-management.openapi.json"))
            .expect("valid model management OpenAPI");
    specification["info"]["version"] = json!(env("VOS_APP_VERSION", "development"));
    specification
}

pub fn version() -> Value {
    let scheduler = std::env::var("V_SCHEDULER_IMAGE").ok();
    let ollama = std::env::var("OLLAMA_SERVER_IMAGE").ok();
    let embedded = std::env::var("SCHEDULER_EMBEDDED_MODEL_HUB").as_deref() == Ok("true");
    let mut components = vec![("scheduler", scheduler.clone())];
    if !embedded {
        components.push(("model-management", scheduler.clone()));
        components.push((
            "router",
            std::env::var("V_ROUTER_BACKEND_IMAGE").ok().or(scheduler),
        ));
    }
    components.extend([
        ("ollama-qa", ollama.clone()),
        ("ollama-embedding", ollama.clone()),
        ("ollama-rerank", ollama),
    ]);
    let images = components
        .into_iter()
        .map(|(service, image)| {
            let name = image.as_ref().and_then(|value| value.rsplit('/').next());
            let tag = name.and_then(|value| value.rsplit_once(':').map(|(_, tag)| tag));
            let includes = if service == "scheduler" && embedded {
                vec!["scheduler", "router", "model-management"]
            } else {
                vec![service]
            };
            json!({"service":service,"image":name,"tag":tag,"components":includes})
        })
        .collect::<Vec<_>>();
    json!({"version":std::env::var("VOS_APP_VERSION").ok(),"profile":std::env::var("MODEL_HUB_PROFILE").ok(),"images":images})
}
