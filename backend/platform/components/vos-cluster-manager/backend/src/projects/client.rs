//! 平台 projects API 客户端（ProjectsClient trait + Real / Mock）
//!
//! 封装平台 `/v1000/app/projects/*`（app-projects-design.md §7.1 七端点）。
//! - `RealProjectsClient`：reqwest + Bearer access-token（token 不出本机）
//! - `MockProjectsClient`：内存状态机（deploy→deployed / remove→removed，故障注入）

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use async_trait::async_trait;

use crate::error::codes;
use crate::error::VcmError;

use super::access_token::read_access_token_from;
use super::dto::{ProjectDeployBo, ProjectDetailVo, ProjectInstanceVo};

/// 平台 projects API 基路径（本应用的 app_id）
pub const APP_ID: &str = match option_env!("VOS_CLUSTER_MANAGER_OWNER_APP") {
    Some(app_id) => app_id,
    None => "com.ictrek.vos-cluster-manager",
};

/// 平台 projects API 客户端 trait
#[async_trait]
pub trait ProjectsClient: Send + Sync {
    /// 应用 projects 声明（GET manifest?app_id=）
    async fn manifest(&self) -> Result<Vec<super::dto::ProjectDefVo>, VcmError>;
    /// 本应用 project 实例列表（GET list?app_id=）
    async fn instances(&self) -> Result<Vec<ProjectInstanceVo>, VcmError>;
    /// 实例详情（GET status?compose_project_name=）
    async fn status(&self, compose_project_name: &str) -> Result<Option<ProjectDetailVo>, VcmError>;
    /// 同步部署（POST deploy；内部轮询平台 workflow 至终态）
    async fn deploy_sync(&self, bo: &ProjectDeployBo) -> Result<ProjectInstanceVo, VcmError>;
    /// 异步部署（POST deploy_async；返回平台 task_id）
    async fn deploy_async(&self, bo: &ProjectDeployBo) -> Result<String, VcmError>;
    /// 同步移除（POST remove）
    async fn remove_sync(&self, compose_project_name: &str) -> Result<(), VcmError>;
    /// 异步移除（POST remove_async；返回平台 task_id）
    async fn remove_async(&self, compose_project_name: &str) -> Result<String, VcmError>;
}

/// 平台错误映射（body.code != 0 / HTTP 非 2xx）
fn map_projects_err(context: &str, detail: impl std::fmt::Display) -> VcmError {
    VcmError::new(codes::PROJECTS_CALL_FAILED, format!("{context}: {detail}"))
}

// ============================================================================
// Real 实现：reqwest + Bearer token（本节点平台 API）
// ============================================================================

/// 真实平台 projects 客户端（base_url 形如 `http://172.17.0.1:1182/v1000/app/projects/`）
pub struct RealProjectsClient {
    http: reqwest::Client,
    base_url: String,
    token_path: std::path::PathBuf,
}

impl RealProjectsClient {
    /// 构造：需提供平台网关 base_url（`<gw>:1182/v1000/app/projects/`）+ token 文件路径
    pub fn new(base_url: String, token_path: &std::path::Path) -> Result<Self, VcmError> {
        if !base_url.ends_with('/') {
            return Err(VcmError::new(
                codes::PROJECTS_CALL_FAILED,
                format!("base_url 应含尾斜杠: {base_url}"),
            ));
        }
        Ok(Self {
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .map_err(|e| map_projects_err("构建 http client", e))?,
            base_url,
            token_path: token_path.to_owned(),
        })
    }

    /// GET Json 并解析 ResultBody.data（T 序列化）
    async fn get_json<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
    ) -> Result<T, VcmError> {
        let resp = self
            .http
            .get(format!("{}{endpoint}", self.base_url))
            .bearer_auth(read_access_token_from(&self.token_path)?)
            .send()
            .await
            .map_err(|e| map_projects_err("GET 请求失败", e))?;
        parse_result(resp).await
    }

    /// POST Json 并解析 ResultBody.data（T 序列化或 unit）
    async fn post_json<T: serde::de::DeserializeOwned>(
        &self,
        endpoint: &str,
        body: &impl serde::Serialize,
    ) -> Result<T, VcmError> {
        let resp = self
            .http
            .post(format!("{}{endpoint}", self.base_url))
            .bearer_auth(read_access_token_from(&self.token_path)?)
            .json(body)
            .send()
            .await
            .map_err(|e| map_projects_err("POST 请求失败", e))?;
        parse_result(resp).await
    }

    /// 轮询平台 async-task 至终态（deploy/remove 同步路径；task 前缀 vos_platform-*）
    ///
    /// 超时窗口 = max(120s, health_timeout_secs + 90s)：平台 deploy workflow 的
    /// wait-project-healthy activity 最长 health_timeout_secs（本应用 300s），
    /// 同步调用必须等过健康窗口，不能拍死 120s。
    async fn wait_platform_task(&self, task_id: &str, health_timeout_secs: &Option<u64>) -> Result<(), VcmError> {
        // 平台 async-task/get 是 GET `/v1000/async-task/get?task_id=`（平台域，非 projects 子路径）
        // 本实现复用平台 API 网关：GET <gw>/v1000/async-task/get?task_id=
        let gw = self
            .base_url
            .replace("/v1000/app/projects/", "/v1000/async-task/get");
        // 轮询（最多 N 次 × 3s）；等待窗口随 health_timeout 放宽
        let poll_deadline_secs = health_timeout_secs.map_or(120, |h| h + 90);
        let max_iters = (poll_deadline_secs / 3) + 1;
        for _ in 0..max_iters {
            let resp = self
                .http
                .get(&gw)
                .query(&[("task_id", task_id)])
                .bearer_auth(read_access_token_from(&self.token_path)?)
                .send()
                .await
                .map_err(|e| map_projects_err("async-task 轮询失败", e))?;
            let body: serde_json::Value = resp
                .json()
                .await
                .map_err(|e| map_projects_err("async-task 响应解析失败", e))?;
            if body["code"].as_i64() != Some(0) {
                return Err(map_projects_err(
                    "async-task 响应异常",
                    body["msg"].as_str().unwrap_or("unknown"),
                ));
            }
            let status = body["data"]["status"].as_str().unwrap_or("Running");
            match status {
                "Completed" => return Ok(()),
                "Failed" | "Cancelled" | "Timeout" => {
                    return Err(map_projects_err(
                        "平台任务失败",
                        status,
                    ));
                }
                _ => tokio::time::sleep(std::time::Duration::from_secs(3)).await,
            }
        }
        Err(map_projects_err("平台任务等待超时（120s）", ""))
    }
}

/// 解析平台 ResultBody 响应（code==0 → serde 出 data；否则 20508）
async fn parse_result<T: serde::de::DeserializeOwned>(
    resp: reqwest::Response,
) -> Result<T, VcmError> {
    let status = resp.status();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| map_projects_err("读取响应失败", e))?;
    let body: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| map_projects_err("响应非 ResultBody JSON", format!("{e} ({status})")))?;
    if body["code"].as_i64() != Some(0) {
        return Err(map_projects_err(
            "平台业务错误",
            body["msg"].as_str().unwrap_or("unknown"),
        ));
    }
    let data_raw = body.get("data").cloned().unwrap_or(serde_json::Value::Null);
    if data_raw.is_null() {
        // 无 data 场景：调用方期待 Result<()> 时用空 JSON 反序列化兼容
        // 这里返回 Ok(()) 需要 T=();为方便统一，返回默认构造
        // —— 用 serde 对 Null 直接转 T 会失败；故对 unit 场景特判由调用方用
        //    `post_json::<serde_json::Value>` 包裹。此处保留 null 透传。
        serde_json::from_value(data_raw)
            .map_err(|e| map_projects_err("解析 data 失败", e))
    } else {
        serde_json::from_value(data_raw)
            .map_err(|e| map_projects_err("解析 data 失败", e))
    }
}

#[async_trait]
impl ProjectsClient for RealProjectsClient {
    async fn manifest(&self) -> Result<Vec<super::dto::ProjectDefVo>, VcmError> {
        self.get_json(&format!("manifest?app_id={APP_ID}")).await
    }

    async fn instances(&self) -> Result<Vec<ProjectInstanceVo>, VcmError> {
        self.get_json(&format!("list?app_id={APP_ID}")).await
    }

    async fn status(&self, compose_project_name: &str) -> Result<Option<ProjectDetailVo>, VcmError> {
        // 平台 status 返回单实例详情；找不到 → None
        let vo: ProjectDetailVo = self
            .get_json(&format!("status?compose_project_name={compose_project_name}"))
            .await?;
        Ok(if vo.instance.is_some() { Some(vo) } else { None })
    }

    async fn deploy_sync(&self, bo: &ProjectDeployBo) -> Result<ProjectInstanceVo, VcmError> {
        // 平台 deploy 同步端点已废弃；统一 async + 轮询终态
        let task_id = self.deploy_async(bo).await?;
        self.wait_platform_task(&task_id, &bo.health_timeout_secs).await?;
        let detail = self
            .status(&bo.compose_project_name)
            .await?
            .ok_or_else(|| map_projects_err("部署完成后实例不存在", bo.compose_project_name.clone()))?;
        detail
            .instance
            .ok_or_else(|| map_projects_err("实例详情缺少 instance", ""))
    }

    async fn deploy_async(&self, bo: &ProjectDeployBo) -> Result<String, VcmError> {
        let mut request = bo.clone();
        if APP_ID == "com.ictrek.v-scheduler" {
            let arch = match std::env::consts::ARCH {
                "aarch64" => "arm64",
                "x86_64" => "amd64",
                other => return Err(map_projects_err("unsupported cluster image architecture", other)),
            };
            request.env_vars.insert("VOS_CLUSTER_PROJECT_ARCH".into(), arch.into());
            if request.project_name == "keepalived" {
                let image = std::env::var("V_SCHEDULER_IMAGE")
                    .map_err(|_| map_projects_err("V-Scheduler image is not configured", "keepalived"))?;
                request.env_vars.insert("VOS_CLUSTER_KEEPALIVED_IMAGE".into(), image);
            }
        }
        let vo: serde_json::Value = self.post_json("deploy_async", &request).await?;
        vo["task_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| map_projects_err("deploy_async 响应缺 task_id", ""))
    }

    async fn remove_sync(&self, compose_project_name: &str) -> Result<(), VcmError> {
        let task_id = self.remove_async(compose_project_name).await?;
        self.wait_platform_task(&task_id, &None).await
    }

    async fn remove_async(&self, compose_project_name: &str) -> Result<String, VcmError> {
        let bo = super::dto::ProjectRemoveBo {
            compose_project_name: compose_project_name.to_string(),
            remove_volumes: Some(false),
        };
        let vo: serde_json::Value = self.post_json("remove_async", &bo).await?;
        vo["task_id"]
            .as_str()
            .map(|s| s.to_string())
            .ok_or_else(|| map_projects_err("remove_async 响应缺 task_id", ""))
    }
}

// ============================================================================
// Mock 实现：内存状态机
// ============================================================================

/// 内存 Mock projects 客户端（deploy→deployed / remove→removed；故障注入）
pub struct MockProjectsClient {
    instances: Arc<StdMutex<HashMap<String, ProjectInstanceVo>>>,
    fail_next: AtomicBool,
}

impl MockProjectsClient {
    pub fn new() -> Self {
        Self {
            instances: Arc::new(StdMutex::new(HashMap::new())),
            fail_next: AtomicBool::new(false),
        }
    }

    /// 注入故障：下一次调用返回 20508
    pub fn set_fail_next(&self) {
        self.fail_next.store(true, Ordering::SeqCst);
    }

    fn check_fail(&self) -> Result<(), VcmError> {
        if self.fail_next.swap(false, Ordering::SeqCst) {
            return Err(map_projects_err("Mock 故障注入", ""));
        }
        Ok(())
    }

    /// 实例快照（测试辅助）
    pub fn snapshot(&self) -> HashMap<String, ProjectInstanceVo> {
        self.instances.lock().unwrap().clone()
    }
}

impl Default for MockProjectsClient {
    fn default() -> Self {
        Self::new()
    }
}

fn mock_instance(compose_project_name: &str) -> ProjectInstanceVo {
    let now = chrono::Utc::now().to_rfc3339();
    ProjectInstanceVo {
        id: "mock".into(),
        app_id: APP_ID.into(),
        project_name: "etcd".into(),
        compose_project_name: compose_project_name.into(),
        state: "deployed".into(),
        deployed_at: Some(now.clone()),
        removed_at: None,
        create_time: now.clone(),
        updated_at: now,
    }
}

#[async_trait]
impl ProjectsClient for MockProjectsClient {
    async fn manifest(&self) -> Result<Vec<super::dto::ProjectDefVo>, VcmError> {
        self.check_fail()?;
        Ok(vec![super::dto::ProjectDefVo {
            name: "etcd".into(),
        }])
    }

    async fn instances(&self) -> Result<Vec<ProjectInstanceVo>, VcmError> {
        self.check_fail()?;
        Ok(self.instances.lock().unwrap().values().cloned().collect())
    }

    async fn status(&self, compose_project_name: &str) -> Result<Option<ProjectDetailVo>, VcmError> {
        self.check_fail()?;
        Ok(self
            .instances
            .lock()
            .unwrap()
            .get(compose_project_name)
            .cloned()
            .map(|i| ProjectDetailVo {
                instance: Some(i),
                runtime: Some(super::dto::ProjectRuntimeVo {
                    aggregate_state: Some("running".into()),
                    ..Default::default()
                }),
                env_keys: vec![],
            }))
    }

    async fn deploy_sync(&self, bo: &ProjectDeployBo) -> Result<ProjectInstanceVo, VcmError> {
        self.deploy_async(bo).await?;
        self.instances
            .lock()
            .unwrap()
            .get(&bo.compose_project_name)
            .cloned()
            .ok_or_else(|| map_projects_err("Mock deploy 失败", ""))
    }

    async fn deploy_async(&self, bo: &ProjectDeployBo) -> Result<String, VcmError> {
        self.check_fail()?;
        let mut map = self.instances.lock().unwrap();
        match map.get(&bo.compose_project_name) {
            Some(_) => Err(map_projects_err("实例已存在", bo.compose_project_name.clone())),
            None => {
                let inst = mock_instance(&bo.compose_project_name);
                map.insert(bo.compose_project_name.clone(), inst);
                Ok(format!("mock-task-{}", bo.compose_project_name))
            }
        }
    }

    async fn remove_sync(&self, compose_project_name: &str) -> Result<(), VcmError> {
        self.remove_async(compose_project_name).await?;
        Ok(())
    }

    async fn remove_async(&self, compose_project_name: &str) -> Result<String, VcmError> {
        self.check_fail()?;
        let mut map = self.instances.lock().unwrap();
        match map.remove(compose_project_name) {
            Some(_) => Ok(format!("mock-remove-{compose_project_name}")),
            None => Err(map_projects_err("实例不存在", compose_project_name)),
        }
    }
}

// ============================================================================
// Unavailable：access-token 缺失 / 平台不可达时的降级实现（明确失败，不假成功）
// ============================================================================

/// access-token / 平台未配置时的降级客户端：所有操作返回 20508
pub struct UnavailableProjectsClient {
    reason: String,
}

impl UnavailableProjectsClient {
    pub fn new(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
        }
    }
}

#[async_trait]
impl ProjectsClient for UnavailableProjectsClient {
    async fn manifest(&self) -> Result<Vec<super::dto::ProjectDefVo>, VcmError> {
        Err(map_projects_err("projects 不可用", &self.reason))
    }
    async fn instances(&self) -> Result<Vec<ProjectInstanceVo>, VcmError> {
        Err(map_projects_err("projects 不可用", &self.reason))
    }
    async fn status(&self, _: &str) -> Result<Option<ProjectDetailVo>, VcmError> {
        Err(map_projects_err("projects 不可用", &self.reason))
    }
    async fn deploy_sync(&self, _: &ProjectDeployBo) -> Result<ProjectInstanceVo, VcmError> {
        Err(map_projects_err("projects 不可用", &self.reason))
    }
    async fn deploy_async(&self, _: &ProjectDeployBo) -> Result<String, VcmError> {
        Err(map_projects_err("projects 不可用", &self.reason))
    }
    async fn remove_sync(&self, _: &str) -> Result<(), VcmError> {
        Err(map_projects_err("projects 不可用", &self.reason))
    }
    async fn remove_async(&self, _: &str) -> Result<String, VcmError> {
        Err(map_projects_err("projects 不可用", &self.reason))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_deploy_status_remove_flow() {
        let c = MockProjectsClient::new();
        // 未部署
        assert!(c.status("cluster-etcd-node-0").await.unwrap().is_none());
        assert!(c.instances().await.unwrap().is_empty());

        // deploy
        let bo = ProjectDeployBo {
            app_id: APP_ID.into(),
            project_name: "etcd".into(),
            compose_project_name: "cluster-etcd-node-0".into(),
            env_vars: HashMap::new(),
            health_timeout_secs: Some(300),
        };
        let inst = c.deploy_sync(&bo).await.unwrap();
        assert_eq!(inst.state, "deployed");

        // status + list
        let d = c.status("cluster-etcd-node-0").await.unwrap().unwrap();
        assert_eq!(d.instance.unwrap().compose_project_name, "cluster-etcd-node-0");
        assert_eq!(c.instances().await.unwrap().len(), 1);

        // remove
        c.remove_sync("cluster-etcd-node-0").await.unwrap();
        assert!(c.status("cluster-etcd-node-0").await.unwrap().is_none());

        // remove 不存在 → Err(20508 语义由上层映射 20505)
        assert!(c.remove_sync("cluster-etcd-node-0").await.is_err());
    }

    #[tokio::test]
    async fn mock_fault_injection() {
        let c = MockProjectsClient::new();
        c.set_fail_next();
        assert!(c.instances().await.is_err());
        // 故障消费后恢复
        assert!(c.instances().await.is_ok());
    }
}
