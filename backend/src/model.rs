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
    pub fn host_memory_need(&self, used: &Self, observed: &Value, reserve: &Self) -> i64 {
        self.host_memory_need_with_policy(used, observed, reserve, false)
    }
    pub fn host_memory_need_with_policy(&self, used: &Self, observed: &Value, reserve: &Self, force: bool) -> i64 {
        let mut committed = used.clone();
        committed.add(self);
        let unified = observed["gpus"]
            .as_array()
            .is_some_and(|gpus| gpus.iter().any(|gpu| gpu["memory_type"] == "unified"));
        committed
            .memory_mib
            .saturating_add(reserve.admission_floor(observed, force).memory_mib)
            .saturating_add(if unified { committed.gpu_memory_mib } else { 0 })
    }

    pub fn fits_guarded(&self, used: &Self, observed: &Value, reserve: &Self) -> bool {
        self.fits_guarded_with_policy(used, observed, reserve, false)
    }
    pub fn fits_guarded_with_policy(&self, used: &Self, observed: &Value, reserve: &Self, force: bool) -> bool {
        if !self.valid() || !used.valid() || !reserve.valid() {
            return false;
        }
        if !force && self.cpu_millicores > 0
            && observed["cpu_used_percent"]
                .as_f64()
                .is_some_and(|n| n >= 95.0)
        {
            return false;
        }
        let mut committed = used.clone();
        committed.add(self);
        let headroom = reserve.admission_floor(observed, force);
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
        let host_need = self.host_memory_need_with_policy(used, observed, reserve, force);
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
            return observed["memory_total_mib"]
                .as_i64()
                .or_else(|| gpu["total_mib"].as_i64())
                .is_some_and(|n| n > 0);
        }
        if gpu["memory_type"] != "dedicated" {
            return false;
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
            // An explicit unified-memory reserve is one host pool, not a
            // second CUDA reserve. Unconfigured nodes retain the auto floor.
            memory_mib: if self.memory_mib > 0 && observed["gpus"].as_array()
                .is_some_and(|g| g.iter().any(|v| v["memory_type"] == "unified")) {
                self.memory_mib.max(1024)
            } else { self.memory_mib.max(2048).max(total / 10) },
            gpu_memory_mib: self.gpu_memory_mib.max(256).max(gpu_total / 20),
            ..self.clone()
        }
    }
    fn admission_floor(&self, observed: &Value, force: bool) -> Self {
        if !force { return self.safety_floor(observed); }
        // Operator override relaxes soft headroom, never the actual model
        // budget, hardware capacity, liveness or native-stop barrier.
        Self { cpu_millicores: 500, memory_mib: 1024, gpu_memory_mib: 256, gpu_slots: 0 }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_unified_one_gib_reserve_and_force_keep_a_hard_floor() {
        let mut report = serde_json::json!({"memory_total_mib":16384,"memory_available_mib":4096,"cpu_logical_count":8,"cpu_used_percent":10,"gpus":[{"memory_type":"unified","total_mib":16384}]});
        let cost = Resources {memory_mib:3072, ..Default::default()};
        let reserve = Resources {memory_mib:1024, ..Default::default()};
        assert!(!cost.fits_guarded(&Resources::default(),&report,&Resources::default()));
        assert!(cost.fits_guarded(&Resources::default(),&report,&reserve));
        assert!(cost.fits_guarded_with_policy(&Resources::default(),&report,&Resources::default(),true));
        report["memory_available_mib"]=serde_json::json!(4095);
        assert!(!cost.fits_guarded_with_policy(&Resources::default(),&report,&Resources::default(),true));
        report["gpus"][0]["memory_type"]=serde_json::json!("dedicated");
        assert!(reserve.safety_floor(&report).memory_mib>=2048);
    }
    #[test]
    fn scheduling_is_strict_and_default_preserves_legacy_envelope_hash() {
        let body=serde_json::json!({"protocol_version":1,"app_task_id":"old","executor":{"id":"test","contract_version":"1","task_type":"test"},"parameters":{},"inputs":[],"idempotency_key":null});
        let parsed:TaskSubmit=serde_json::from_value(body.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(),body);
        let command=serde_json::json!({"command_id":"old","task_id":"old","attempt_id":"old","fence":1,"executor_id":"test","parameters":{},"lease_seconds":30,"cancel":false});
        let parsed:Command=serde_json::from_value(command.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(),command,"default commands must remain readable by strict legacy workers");
        assert!(serde_json::from_value::<Scheduling>(serde_json::json!({"priority":"arbitrary"})).is_err());
        assert!(serde_json::from_value::<Scheduling>(serde_json::json!({"force_admission":"true"})).is_err());
        assert!(serde_json::from_value::<Scheduling>(serde_json::json!({"resource_budget":1})).is_err());
    }
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
    fn thor_uses_host_pool_without_cuda_memory_counters() {
        let resources = Resources {
            cpu_millicores: 1000,
            memory_mib: 1024,
            gpu_slots: 1,
            gpu_memory_mib: 2048,
        };
        let mut report = serde_json::json!({"memory_total_mib":16384,"memory_available_mib":8192,"cpu_logical_count":14,"gpus":[{"memory_type":"unified","total_mib":null,"free_mib":null}]});
        assert!(resources.fits_guarded(&Resources::default(), &report, &Resources::default()));
        report["memory_available_mib"] = serde_json::json!(5000);
        assert!(!resources.fits_guarded(&Resources::default(), &report, &Resources::default()));
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
    /// 可核验的同步许可执行器；不授予创建、续租或释放其他应用许可的权限。
    #[serde(default)]
    pub permit_executors: Vec<String>,
}
fn one() -> i64 {
    1
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Placement {
    pub node_id: String,
    pub resources: Resources,
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
    pub placements: Vec<Placement>,
    #[serde(default)]
    pub retry_mode: String,
    #[serde(default = "one")]
    pub max_attempts: i64,
}
impl Executor {
    pub fn allocations(&self) -> Vec<Placement> {
        let mut result = vec![Placement {
            node_id: self.node_id.clone(),
            resources: self.resources.clone(),
        }];
        result.extend(self.placements.clone());
        result.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        result
    }
    pub fn valid_placements(&self) -> bool {
        let mut seen = std::collections::HashSet::new();
        seen.insert(self.node_id.as_str());
        self.placements.len() <= 31
            && (self.placements.is_empty() || self.task_type == "model.infer")
            && self.placements.iter().all(|p| {
                seen.insert(&p.node_id)
                    && p.resources.valid()
                    && p.resources.cpu_millicores > 0
                    && p.resources.memory_mib > 0
                    && (p.resources.gpu_memory_mib == 0 || p.resources.gpu_slots > 0)
            })
    }
}
#[derive(Clone, Deserialize, Serialize)]
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
    pub discovery_source: String,
    #[serde(default)]
    pub discovery_id: String,
    #[serde(default)]
    pub vos: Option<VosEndpoint>,
}
#[derive(Clone, Deserialize, Serialize)]
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
    #[serde(default)]
    pub local_agent: Option<Value>,
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
    #[serde(default, skip_serializing_if = "Scheduling::is_default")]
    pub scheduling: Scheduling,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Scheduling {
    #[serde(default)]
    pub priority: Priority,
    #[serde(default)]
    pub preemptible: bool,
    #[serde(default)]
    pub allow_preemption: bool,
    #[serde(default)]
    pub force_admission: bool,
}
impl Scheduling {
    pub fn is_default(&self) -> bool { self == &Self::default() }
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Priority { #[default] Background, Interactive, High }
impl Priority {
    pub fn score(self) -> i64 { match self { Self::Background => 0, Self::Interactive => 1, Self::High => 2 } }
}
fn is_false(value: &bool) -> bool { !*value }
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
    #[serde(default, skip_serializing_if = "is_false")]
    pub preempted: bool,
}

// Operator-approved persistent overlays; credentials remain in their own stores.
#[derive(Clone, Default)]
pub struct ManagedConfig {
    pub nodes: Vec<Node>,
    pub executors: Vec<Executor>,
    pub disabled_executors: Vec<String>,
}
