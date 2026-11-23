//! consul 客户端抽象：注册 / 注销 / 列举（供监控注册与 reconcile）
//!
//! 生产实现走 consul agent HTTP API；测试用内存替身。consul 不可达 → `20735`。

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::{ModelinferError, codes};
use crate::monitor::{ConsulService, RegisteredService};

/// consul 连接配置
#[derive(Debug, Clone)]
pub struct ConsulConfig {
    pub url: String,
}

impl ConsulConfig {
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into().trim_end_matches('/').to_string(),
        }
    }
}

impl Default for ConsulConfig {
    fn default() -> Self {
        Self::new("http://172.17.0.1:8500")
    }
}

/// consul 客户端
#[async_trait]
pub trait ConsulClient: Send + Sync {
    /// 注册（幂等；覆盖既有定义）
    async fn register(&self, spec: &ConsulService) -> Result<(), ModelinferError>;
    /// 注销（不存在视为成功）
    async fn deregister(&self, service_id: &str) -> Result<(), ModelinferError>;
    /// 列举本 agent 上的服务（ID → 摘要）
    async fn list_services(&self) -> Result<BTreeMap<String, RegisteredService>, ModelinferError>;
}

/// 生产实现（reqwest）
pub struct RealConsulClient {
    cfg: ConsulConfig,
    client: reqwest::Client,
}

impl RealConsulClient {
    pub fn new(cfg: ConsulConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .unwrap_or_default();
        Self { cfg, client }
    }
}

/// 构造 consul 注册 body（纯函数，便于单测）
pub fn register_body(spec: &ConsulService) -> Value {
    let check_url = format!("http://{}:{}/metrics", spec.address, spec.port);
    serde_json::json!({
        "ID": spec.id,
        "Name": spec.name,
        "Address": spec.address,
        "Port": spec.port,
        "Tags": spec.tags,
        "Meta": spec.meta,
        "Check": {
            "Name": spec.name,
            "HTTP": check_url,
            "Interval": "30s",
            "Timeout": "5s",
            "DeregisterCriticalServiceAfter": "72h"
        }
    })
}

/// 解析 `GET /v1/agent/services` 响应（纯函数）
pub fn parse_services(v: &Value) -> BTreeMap<String, RegisteredService> {
    let mut out = BTreeMap::new();
    let Some(obj) = v.as_object() else {
        return out;
    };
    for (id, item) in obj {
        let name = item.get("Service").and_then(Value::as_str).unwrap_or("").to_string();
        let address = item.get("Address").and_then(Value::as_str).unwrap_or("").to_string();
        let port = item.get("Port").and_then(Value::as_u64).unwrap_or(0) as u16;
        out.insert(
            id.clone(),
            RegisteredService { id: id.clone(), name, address, port },
        );
    }
    out
}

#[async_trait]
impl ConsulClient for RealConsulClient {
    async fn register(&self, spec: &ConsulService) -> Result<(), ModelinferError> {
        let url = format!("{}/v1/agent/service/register?replace-existing-checks=true", self.cfg.url);
        let resp = self
            .client
            .put(&url)
            .json(&register_body(spec))
            .send()
            .await
            .map_err(|e| {
                ModelinferError::new(codes::CONSUL_UNAVAILABLE, format!("consul 不可达 {url}: {e}"))
            })?;
        if !resp.status().is_success() {
            return Err(ModelinferError::new(
                codes::CONSUL_UNAVAILABLE,
                format!("consul 注册失败（HTTP {}）: {}", resp.status(), spec.id),
            ));
        }
        Ok(())
    }

    async fn deregister(&self, service_id: &str) -> Result<(), ModelinferError> {
        let url = format!("{}/v1/agent/service/deregister/{}", self.cfg.url, service_id);
        let resp = self.client.put(&url).send().await.map_err(|e| {
            ModelinferError::new(codes::CONSUL_UNAVAILABLE, format!("consul 不可达 {url}: {e}"))
        })?;
        if !resp.status().is_success() {
            return Err(ModelinferError::new(
                codes::CONSUL_UNAVAILABLE,
                format!("consul 注销失败（HTTP {}）: {service_id}", resp.status()),
            ));
        }
        Ok(())
    }

    async fn list_services(&self) -> Result<BTreeMap<String, RegisteredService>, ModelinferError> {
        let url = format!("{}/v1/agent/services", self.cfg.url);
        let resp = self.client.get(&url).send().await.map_err(|e| {
            ModelinferError::new(codes::CONSUL_UNAVAILABLE, format!("consul 不可达 {url}: {e}"))
        })?;
        let body: Value = resp.json().await.map_err(|e| {
            ModelinferError::new(codes::CONSUL_UNAVAILABLE, format!("解析 consul 服务列表失败: {e}"))
        })?;
        Ok(parse_services(&body))
    }
}

/// 测试替身（内存服务表）
#[derive(Default)]
pub struct MockConsulClient {
    services: Mutex<BTreeMap<String, RegisteredService>>,
    pub fail: bool,
    pub calls: Mutex<Vec<String>>,
}

impl MockConsulClient {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn failing() -> Self {
        Self {
            fail: true,
            ..Self::default()
        }
    }

    /// 预置一个已注册服务
    pub fn with_service(self, reg: RegisteredService) -> Self {
        self.services.lock().unwrap().insert(reg.id.clone(), reg);
        self
    }

    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn snapshot(&self) -> BTreeMap<String, RegisteredService> {
        self.services.lock().unwrap().clone()
    }

    fn guard(&self, name: &str) -> Result<(), ModelinferError> {
        self.calls.lock().unwrap().push(name.to_string());
        if self.fail {
            return Err(ModelinferError::new(codes::CONSUL_UNAVAILABLE, "mock consul 不可达"));
        }
        Ok(())
    }
}

#[async_trait]
impl ConsulClient for MockConsulClient {
    async fn register(&self, spec: &ConsulService) -> Result<(), ModelinferError> {
        self.guard(&format!("register:{}", spec.id))?;
        self.services.lock().unwrap().insert(
            spec.id.clone(),
            RegisteredService {
                id: spec.id.clone(),
                name: spec.name.clone(),
                address: spec.address.clone(),
                port: spec.port,
            },
        );
        Ok(())
    }

    async fn deregister(&self, service_id: &str) -> Result<(), ModelinferError> {
        self.guard(&format!("deregister:{service_id}"))?;
        self.services.lock().unwrap().remove(service_id);
        Ok(())
    }

    async fn list_services(&self) -> Result<BTreeMap<String, RegisteredService>, ModelinferError> {
        self.guard("list_services")?;
        Ok(self.services.lock().unwrap().clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ConsulService {
        let mut meta = BTreeMap::new();
        meta.insert("node_id".to_string(), "n1".to_string());
        ConsulService {
            id: "modelinfer-node-exporter-n1".into(),
            name: crate::monitor::ComponentKind::NodeExporter.service_name().into(),
            address: "10.0.0.1".into(),
            port: 9100,
            tags: vec!["modelinfer".into(), "node-exporter".into()],
            meta,
        }
    }

    #[test]
    fn register_body_shape() {
        let b = register_body(&spec());
        assert_eq!(b["ID"], "modelinfer-node-exporter-n1");
        assert_eq!(b["Address"], "10.0.0.1");
        assert_eq!(b["Port"], 9100);
        assert_eq!(b["Check"]["HTTP"], "http://10.0.0.1:9100/metrics");
        assert_eq!(b["Check"]["Interval"], "30s");
    }

    #[test]
    fn parse_services_variants() {
        let v = serde_json::json!({
            "modelinfer-node-exporter-n1": {
                "ID": "modelinfer-node-exporter-n1",
                "Service": "modelinfer-node-exporter",
                "Address": "10.0.0.1",
                "Port": 9100
            },
            "consul": {"ID": "consul", "Service": "consul", "Address": "", "Port": 8300}
        });
        let m = parse_services(&v);
        assert_eq!(m.len(), 2);
        assert_eq!(m["modelinfer-node-exporter-n1"].port, 9100);
        assert!(parse_services(&serde_json::json!([])).is_empty());
    }

    #[tokio::test]
    async fn mock_register_list_deregister() {
        let c = MockConsulClient::new();
        c.register(&spec()).await.unwrap();
        let list = c.list_services().await.unwrap();
        assert_eq!(list.len(), 1);
        c.deregister("modelinfer-node-exporter-n1").await.unwrap();
        assert!(c.list_services().await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn mock_fail_is_20735() {
        let c = MockConsulClient::failing();
        assert_eq!(c.register(&spec()).await.unwrap_err().code, codes::CONSUL_UNAVAILABLE);
        assert_eq!(c.list_services().await.unwrap_err().code, codes::CONSUL_UNAVAILABLE);
    }
}