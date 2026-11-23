//! 平台 projects API DTO（对齐 app-projects-design.md §7.2）
//!
//! 平台 JSON 字段为 snake_case（实测 projects/manifest 响应含 `compose_exists` /
//! `compose_path` 等 snake_case 字段，与平台 Rust 侧默认 serde 一致），
//! 本模块字段名直接与平台文档对齐，无需 rename。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// 应用 projects 声明项（`/v1000/app/projects/manifest` 返回）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectDefVo {
    pub name: String,
    // title / description 等元数据按需补充，当前编排不需
}

/// 平台 Project 实例管理态（`/v1000/app/projects/list` / `status.instance`）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectInstanceVo {
    pub id: String,
    pub app_id: String,
    pub project_name: String,
    pub compose_project_name: String,
    pub state: String, // deploying | deployed | removing | failed | removed
    #[serde(default)]
    pub deployed_at: Option<String>,
    #[serde(default)]
    pub removed_at: Option<String>,
    /// 平台字段名 create_time（实测 status/list 返回；非 created_at）
    pub create_time: String,
    pub updated_at: String,
}

/// 单容器实况（status.runtime.containers[]）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectContainerVo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub service: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub health: String,
}

/// 运行态容器实况（`status.runtime`）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectRuntimeVo {
    #[serde(default)]
    pub docker_reachable: Option<bool>,
    #[serde(default)]
    pub aggregate_state: Option<String>,
    #[serde(default)]
    pub containers: Vec<ProjectContainerVo>,
}

/// 实例详情（`/v1000/app/projects/status` 返回）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct ProjectDetailVo {
    #[serde(default)]
    pub instance: Option<ProjectInstanceVo>,
    #[serde(default)]
    pub runtime: Option<ProjectRuntimeVo>,
    #[serde(default)]
    pub env_keys: Vec<String>,
}

/// 部署请求体（`/v1000/app/projects/deploy(_async)`）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectDeployBo {
    pub app_id: String,
    pub project_name: String,
    pub compose_project_name: String,
    #[serde(default)]
    pub env_vars: HashMap<String, String>,
    #[serde(default)]
    pub health_timeout_secs: Option<u64>,
}

/// 移除请求体（`/v1000/app/projects/remove(_async)`）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProjectRemoveBo {
    pub compose_project_name: String,
    #[serde(default)]
    pub remove_volumes: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dto_snake_case_fields() {
        // 平台返回形态（snake_case；时间字段为 create_time，非 created_at——实测）
        let raw = r#"{"id":"01H","app_id":"com.ictrek.vos-cluster-manager",
            "project_name":"etcd","compose_project_name":"cluster-etcd-node-0",
            "state":"deployed","deployed_at":"2026-09-06T00:00:00Z",
            "create_time":"2026-09-06T00:00:00Z","updated_at":"2026-09-06T00:00:00Z"}"#;
        let vo: ProjectInstanceVo = serde_json::from_str(raw).unwrap();
        assert_eq!(vo.compose_project_name, "cluster-etcd-node-0");
        assert_eq!(vo.state, "deployed");
        assert!(vo.deployed_at.is_some());

        // 缺失可选字段兼容（removed_at 无）
        let raw2 = r#"{"id":"01H","app_id":"a","project_name":"etcd",
            "compose_project_name":"x","state":"failed","create_time":"t","updated_at":"t"}"#;
        let vo2: ProjectInstanceVo = serde_json::from_str(raw2).unwrap();
        assert!(vo2.deployed_at.is_none());
        assert!(vo2.removed_at.is_none());
    }

    #[test]
    fn deploy_bo_snake_case_fields() {
        let bo = ProjectDeployBo {
            app_id: "com.ictrek.vos-cluster-manager".into(),
            project_name: "etcd".into(),
            compose_project_name: "cluster-etcd-node-0".into(),
            env_vars: HashMap::from([("ETCD_NAME".into(), "etcd0".into())]),
            health_timeout_secs: Some(300),
        };
        let json = serde_json::to_string(&bo).unwrap();
        assert!(json.contains("\"compose_project_name\""), "json: {json}");
        assert!(json.contains("\"app_id\""), "json: {json}");
        assert!(json.contains("\"health_timeout_secs\""), "json: {json}");
    }
}