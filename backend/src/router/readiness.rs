use super::*;
use std::collections::BTreeSet;

pub(super) fn initial_roles() -> BTreeSet<String> {
    ["qa", "embedding", "rerank"]
        .into_iter()
        .filter(|role| {
            !env(
                &format!("V_SCHEDULER_OLLAMA_{}_URL", role.to_ascii_uppercase()),
                &format!("http://v-scheduler-ollama-{role}:11535"),
            )
            .trim()
            .is_empty()
        })
        .map(str::to_owned)
        .collect()
}

pub(super) fn initializing() -> Failure {
    let mut error = Failure::new(
        503,
        "Model catalog is initializing; retry after discovery completes",
    );
    error.code = Some("MODEL_CATALOG_INITIALIZING".into());
    error
}

impl Gateway {
    pub(super) fn catalog_initializing(&self) -> bool {
        !self.initial_discovery.lock().unwrap().is_empty()
    }

    pub(super) fn local_model_readiness(&self, requested: &str) -> Api<()> {
        let Some(role) = requested
            .strip_prefix("v-scheduler/ollama-")
            .and_then(|rest| rest.split_once('/').map(|(role, _)| role))
        else {
            return Ok(());
        };
        if self.initial_discovery.lock().unwrap().contains(role) {
            return Err(initializing());
        }
        let registry = self.registry.lock().unwrap();
        if registry
            .upstreams
            .get(&format!("v-scheduler/ollama-{role}"))
            .is_some_and(|u| !u.live())
        {
            let mut error = Failure::new(
                503,
                "Built-in inference backend is unavailable; check its health and model discovery",
            );
            error.code = Some("UPSTREAM_UNAVAILABLE".into());
            return Err(error);
        }
        Ok(())
    }
}

pub(super) fn snapshot(g: &Gateway) -> Value {
    let pending = g.initial_discovery.lock().unwrap().clone();
    let registry = g.registry.lock().unwrap();
    let roles = ["qa", "embedding", "rerank"]
        .into_iter()
        .map(|role| {
            let upstream = registry
                .upstreams
                .get(&format!("v-scheduler/ollama-{role}"));
            let state = if pending.contains(role) {
                "initializing"
            } else if upstream.is_some_and(|u| u.live()) {
                "ready"
            } else if upstream.is_some() {
                "unavailable"
            } else {
                "disabled"
            };
            (role, state)
        })
        .collect::<std::collections::BTreeMap<_, _>>();
    let ready = roles
        .values()
        .all(|state| matches!(*state, "ready" | "disabled"));
    json!({"ready":ready,"scope":"gateway-catalog","version":version(),"backends":roles})
}

pub(super) fn response(g: &Gateway) -> Response {
    let status = snapshot(g);
    let ready = status["ready"] == true;
    let mut response = (
        if ready {
            StatusCode::OK
        } else {
            StatusCode::SERVICE_UNAVAILABLE
        },
        axum::Json(status),
    )
        .into_response();
    if !ready {
        response
            .headers_mut()
            .insert("retry-after", "1".parse().unwrap());
    }
    response
}
