use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Resources {
    #[serde(default)]
    pub cpu_millicores: i64,
    #[serde(default)]
    pub memory_mib: i64,
    #[serde(default)]
    pub gpu_slots: i64,
    #[serde(default)]
    pub gpu_memory_mib: i64,
}
impl Resources {
    pub fn valid(&self) -> bool {
        [
            self.cpu_millicores,
            self.memory_mib,
            self.gpu_slots,
            self.gpu_memory_mib,
        ]
        .iter()
        .all(|x| *x >= 0)
    }
    pub fn fits(&self, used: &Self, capacity: &Self) -> bool {
        self.valid()
            && self.cpu_millicores <= capacity.cpu_millicores.saturating_sub(used.cpu_millicores)
            && self.memory_mib <= capacity.memory_mib.saturating_sub(used.memory_mib)
            && self.gpu_slots <= capacity.gpu_slots.saturating_sub(used.gpu_slots)
            && self.gpu_memory_mib <= capacity.gpu_memory_mib.saturating_sub(used.gpu_memory_mib)
    }
    pub fn add(&mut self, other: &Self) {
        self.cpu_millicores = self.cpu_millicores.saturating_add(other.cpu_millicores);
        self.memory_mib = self.memory_mib.saturating_add(other.memory_mib);
        self.gpu_slots = self.gpu_slots.saturating_add(other.gpu_slots);
        self.gpu_memory_mib = self.gpu_memory_mib.saturating_add(other.gpu_memory_mib);
    }
    pub fn fits_observed(&self, observed: &Value) -> bool {
        let available = observed["memory_available_mib"].as_i64().unwrap_or(0);
        if available < self.memory_mib {
            return false;
        }
        if self.gpu_slots == 0 {
            return true;
        }
        let Some(gpu) = observed["gpus"].as_array().and_then(|items| items.first()) else {
            return false;
        };
        // On Tegra the CUDA pool shares system RAM. CUDA free excludes
        // reclaimable page cache; admission budgets host and GPU memory together.
        // Keep the raw CUDA free value in telemetry instead of relabeling it.
        if gpu["memory_type"] == "unified" && gpu["total_mib"].as_i64().is_some_and(|n| n > 0) {
            return self
                .memory_mib
                .checked_add(self.gpu_memory_mib)
                .is_some_and(|required| available >= required);
        }
        gpu["free_mib"]
            .as_i64()
            .is_some_and(|free| free >= self.gpu_memory_mib)
    }

    // Keep VOS responsive even when approved reservations have not materialized
    // in the latest sample yet. Count all commitments conservatively against
    // available memory; observed utilization is never a free capacity promise.
    pub fn fits_guarded(&self, used: &Self, observed: &Value, reserve: &Self) -> bool {
        if !self.valid() || !used.valid() || !reserve.valid() {
            return false;
        }
        if self.cpu_millicores > 0
            && observed["cpu_used_percent"]
                .as_f64()
                .is_some_and(|n| n >= 95.0)
        {
            return false;
        }
        let mut committed = used.clone();
        committed.add(self);
        let headroom = reserve.safety_floor(observed);
        if let Some(cores) = observed["cpu_logical_count"].as_i64().filter(|n| *n > 0) {
            if committed
                .cpu_millicores
                .saturating_add(headroom.cpu_millicores)
                > cores.saturating_mul(1000)
            {
                return false;
            }
        }
        let unified = observed["gpus"]
            .as_array()
            .is_some_and(|gpus| gpus.iter().any(|gpu| gpu["memory_type"] == "unified"));
        let host_need = committed
            .memory_mib
            .saturating_add(headroom.memory_mib)
            .saturating_add(if unified { committed.gpu_memory_mib } else { 0 });
        if observed["memory_available_mib"].as_i64().unwrap_or(0) < host_need {
            return false;
        }
        if committed.gpu_slots == 0 && committed.gpu_memory_mib == 0 {
            return true;
        }
        let Some(gpu) = observed["gpus"].as_array().and_then(|gpus| gpus.first()) else {
            return false;
        };
        if unified {
            return gpu["total_mib"].as_i64().is_some_and(|n| n > 0);
        }
        gpu["free_mib"].as_i64().is_some_and(|free| {
            free >= committed
                .gpu_memory_mib
                .saturating_add(headroom.gpu_memory_mib)
        })
    }

    pub fn safety_floor(&self, observed: &Value) -> Self {
        let total = observed["memory_total_mib"].as_i64().unwrap_or(0).max(0);
        let gpu_total = observed["gpus"]
            .as_array()
            .and_then(|gpus| gpus.first())
            .and_then(|gpu| gpu["total_mib"].as_i64())
            .unwrap_or(0)
            .max(0);
        Self {
            cpu_millicores: self
                .cpu_millicores
                .max(if observed["cpu_logical_count"] == 1 {
                    500
                } else {
                    1000
                }),
            memory_mib: self.memory_mib.max(2048).max(total / 10),
            gpu_memory_mib: self.gpu_memory_mib.max(256).max(gpu_total / 20),
            ..self.clone()
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn admission_keeps_host_headroom_and_counts_unmaterialized_reservations() {
        let request = Resources {
            memory_mib: 1024,
            cpu_millicores: 1000,
            ..Resources::default()
        };
        let report = serde_json::json!({"memory_total_mib":8192,"memory_available_mib":4096,"cpu_used_percent":20});
        assert!(request.fits_guarded(
            &Resources {
                memory_mib: 1024,
                ..Resources::default()
            },
            &report,
            &Resources::default()
        ));
        assert!(!request.fits_guarded(
            &Resources {
                memory_mib: 1025,
                ..Resources::default()
            },
            &report,
            &Resources::default()
        ));
        assert!(!request.fits_guarded(
            &Resources::default(),
            &serde_json::json!({"memory_available_mib":4096,"cpu_used_percent":95}),
            &Resources::default()
        ));
        assert!(!request.fits_guarded(
            &Resources::default(),
            &report,
            &Resources {
                memory_mib: 4096,
                ..Resources::default()
            }
        ));
        assert!(!request.fits_guarded(
            &Resources {
                cpu_millicores: 1000,
                ..Resources::default()
            },
            &serde_json::json!({"cpu_logical_count":2,"memory_available_mib":4096}),
            &Resources::default()
        ));
    }
    #[test]
    fn unified_memory_has_one_host_safety_reserve_and_dedicated_gpu_keeps_headroom() {
        let request = Resources {
            memory_mib: 512,
            gpu_slots: 1,
            gpu_memory_mib: 1024,
            ..Resources::default()
        };
        let mut report = serde_json::json!({"memory_total_mib":8192,"memory_available_mib":3584,"gpus":[{"memory_type":"unified","total_mib":8192,"free_mib":100}]});
        assert!(request.fits_guarded(&Resources::default(), &report, &Resources::default()));
        report["memory_available_mib"] = serde_json::json!(3583);
        assert!(!request.fits_guarded(&Resources::default(), &report, &Resources::default()));
        report["memory_available_mib"] = serde_json::json!(4096);
        report["gpus"][0]["memory_type"] = serde_json::json!("dedicated");
        report["gpus"][0]["free_mib"] = serde_json::json!(1432);
        assert!(!request.fits_guarded(&Resources::default(), &report, &Resources::default()));
        report["gpus"][0]["free_mib"] = serde_json::json!(1433);
        assert!(request.fits_guarded(&Resources::default(), &report, &Resources::default()));
    }
    #[test]
    fn unified_memory_shares_the_observed_host_budget() {
        let resources = Resources {
            memory_mib: 1024,
            gpu_slots: 1,
            gpu_memory_mib: 2048,
            ..Resources::default()
        };
        let mut report = serde_json::json!({"memory_available_mib":4096,"gpus":[{"total_mib":8192,"free_mib":128,"memory_type":"unified"}]});
        assert!(resources.fits_observed(&report));
        report["memory_available_mib"] = serde_json::json!(3000);
        assert!(!resources.fits_observed(&report));
        report["memory_available_mib"] = serde_json::json!(4096);
        report["gpus"][0]["memory_type"] = serde_json::json!("dedicated");
        assert!(!resources.fits_observed(&report));
    }
    #[test]
    fn gpu_slots_require_actual_telemetry_even_with_zero_memory_request() {
        let gpu = Resources {
            gpu_slots: 1,
            ..Resources::default()
        };
        assert!(!gpu.fits_observed(&serde_json::json!({"memory_available_mib":1024,"gpus":[]})));
        assert!(gpu.fits_observed(
            &serde_json::json!({"memory_available_mib":1024,"gpus":[{"free_mib":512}]})
        ));
        let cpu = Resources {
            memory_mib: 128,
            ..Resources::default()
        };
        assert!(!cpu.fits_observed(&serde_json::json!({})));
        assert!(cpu.fits_observed(&serde_json::json!({"memory_available_mib":512})));
    }
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Principal {
    pub token: String,
    pub role: String,
    pub app_id: String,
    #[serde(default)]
    pub node_id: String,
    #[serde(default)]
    pub delegated_executors: Vec<String>,
}
fn one() -> i64 {
    1
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Executor {
    pub id: String,
    pub contract_version: String,
    pub task_type: String,
    pub node_id: String,
    pub app_id: String,
    pub resources: Resources,
    #[serde(default)]
    pub upstream_urls: Vec<String>,
    #[serde(default)]
    pub retry_mode: String,
    #[serde(default = "one")]
    pub max_attempts: i64,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Node {
    pub id: String,
    pub capacities: Resources,
    #[serde(default)]
    pub resident_baseline: Resources,
    #[serde(default)]
    pub safety_reserve: Resources,
    #[serde(default)]
    pub vos_member_id: String,
    #[serde(default)]
    pub vos: Option<VosEndpoint>,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VosEndpoint {
    pub url: String,
    #[serde(default)]
    pub token: String,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub principals: Vec<Principal>,
    pub nodes: Vec<Node>,
    pub executors: Vec<Executor>,
    #[serde(default)]
    pub vos_cluster: Option<VosEndpoint>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ExecutorRef {
    pub id: String,
    pub contract_version: String,
    pub task_type: String,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TaskSubmit {
    pub protocol_version: u32,
    pub app_task_id: String,
    pub executor: ExecutorRef,
    pub parameters: Value,
    #[serde(default)]
    pub inputs: Vec<Value>,
    #[serde(default)]
    pub idempotency_key: Option<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub command_id: String,
    pub task_id: String,
    pub attempt_id: String,
    pub fence: i64,
    pub executor_id: String,
    pub parameters: Value,
    pub lease_seconds: u64,
    pub cancel: bool,
}
