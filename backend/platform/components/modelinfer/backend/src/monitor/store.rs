//! 节点监控组件状态与步骤落库（`modelinfer_node_monitors`，N:1，按 `(node_id, component)`）

use sea_orm::{ActiveModelTrait, ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, Set};
use serde::{Deserialize, Serialize};

use crate::entities::node_monitor;
use crate::error::ModelinferError;
use crate::nodes::db_err;
use crate::time;

/// 监控状态字面值
pub mod state {
    pub const PENDING: &str = "pending";
    pub const DEPLOYING: &str = "deploying";
    pub const DEPLOYED: &str = "deployed";
    pub const FAILED: &str = "failed";
}

/// 单个部署步骤
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MonitorStep {
    /// 机器标识（prepare / ensure-image / deploy / register-consul / verify）
    pub name: String,
    /// 中文可读名
    pub title: String,
    /// running / succeeded / failed
    pub state: String,
    /// 结果 / 错误明细
    pub detail: String,
    pub started_at: String,
    pub finished_at: Option<String>,
}

impl MonitorStep {
    pub fn new(name: &str, title: &str) -> Self {
        Self {
            name: name.to_string(),
            title: title.to_string(),
            state: "running".to_string(),
            detail: String::new(),
            started_at: time::now_iso(),
            finished_at: None,
        }
    }
    pub fn done(mut self, ok: bool, detail: impl Into<String>) -> Self {
        self.state = if ok { "succeeded" } else { "failed" }.to_string();
        self.detail = detail.into();
        self.finished_at = Some(time::now_iso());
        self
    }
}

/// 监控视图（供 API）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorView {
    pub node_id: String,
    /// 组件标识（`node-exporter` / `nvidia-gpu-exporter`）
    pub component: String,
    /// 指标分类（`system` / `gpu`）
    pub category: String,
    pub state: String,
    pub service_id: String,
    pub service_name: String,
    pub image: String,
    pub workflow_id: String,
    pub last_error: String,
    pub steps: Vec<MonitorStep>,
    pub deployed_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl From<node_monitor::Model> for MonitorView {
    fn from(m: node_monitor::Model) -> Self {
        Self {
            node_id: m.node_id,
            component: m.component,
            category: m.category,
            state: m.state,
            service_id: m.service_id,
            service_name: m.service_name,
            image: m.image,
            workflow_id: m.workflow_id,
            last_error: m.last_error,
            steps: serde_json::from_str(&m.steps_json).unwrap_or_default(),
            deployed_at: m.deployed_at,
            created_at: m.created_at,
            updated_at: m.updated_at,
        }
    }
}

/// 聚合多组件状态（纯函数）：任一失败 → failed；任一进行中 → deploying；全部成功 → deployed；
/// 空 / 其它 → pending。
pub fn aggregate_state(states: &[String]) -> String {
    if states.is_empty() {
        return state::PENDING.to_string();
    }
    if states.iter().any(|s| s == state::FAILED) {
        return state::FAILED.to_string();
    }
    if states.iter().any(|s| s == state::DEPLOYING) {
        return state::DEPLOYING.to_string();
    }
    if states.iter().all(|s| s == state::DEPLOYED) {
        return state::DEPLOYED.to_string();
    }
    state::PENDING.to_string()
}

/// 读取节点某组件监控记录（无则 None）
pub async fn get(
    db: &DatabaseConnection,
    node_id: &str,
    component: &str,
) -> Result<Option<node_monitor::Model>, ModelinferError> {
    node_monitor::Entity::find()
        .filter(node_monitor::Column::NodeId.eq(node_id.to_string()))
        .filter(node_monitor::Column::Component.eq(component.to_string()))
        .one(db)
        .await
        .map_err(db_err)
}

/// 读取节点全部组件监控记录
pub async fn list_of(
    db: &DatabaseConnection,
    node_id: &str,
) -> Result<Vec<node_monitor::Model>, ModelinferError> {
    node_monitor::Entity::find()
        .filter(node_monitor::Column::NodeId.eq(node_id.to_string()))
        .all(db)
        .await
        .map_err(db_err)
}

/// 读取节点全部组件监控视图（供详情页；无记录时为空列表）
pub async fn views_of(
    db: &DatabaseConnection,
    node_id: &str,
) -> Result<Vec<MonitorView>, ModelinferError> {
    Ok(list_of(db, node_id).await?.into_iter().map(MonitorView::from).collect())
}

/// 读取或构造单组件空视图（未部署返回 pending 空态）
pub async fn view_of(
    db: &DatabaseConnection,
    node_id: &str,
    component: &str,
) -> Result<MonitorView, ModelinferError> {
    match get(db, node_id, component).await? {
        Some(m) => Ok(MonitorView::from(m)),
        None => Ok(MonitorView {
            node_id: node_id.to_string(),
            component: component.to_string(),
            category: crate::monitor::ComponentKind::parse(component)
                .map(|k| k.category().to_string())
                .unwrap_or_default(),
            state: state::PENDING.to_string(),
            service_id: String::new(),
            service_name: String::new(),
            image: String::new(),
            workflow_id: String::new(),
            last_error: String::new(),
            steps: Vec::new(),
            deployed_at: None,
            created_at: String::new(),
            updated_at: String::new(),
        }),
    }
}

/// 开始一次部署：置 `deploying`、清空步骤与错误（按 `(node_id, component)` upsert）
#[allow(clippy::too_many_arguments)]
pub async fn begin(
    db: &DatabaseConnection,
    node_id: &str,
    component: &str,
    workflow_id: &str,
    image: &str,
    category: &str,
    service_id: &str,
    service_name: &str,
) -> Result<(), ModelinferError> {
    let now = time::now_iso();
    match get(db, node_id, component).await? {
        Some(existing) => {
            let mut am: node_monitor::ActiveModel = existing.into();
            am.state = Set(state::DEPLOYING.to_string());
            am.workflow_id = Set(workflow_id.to_string());
            am.image = Set(image.to_string());
            am.category = Set(category.to_string());
            am.service_id = Set(service_id.to_string());
            am.service_name = Set(service_name.to_string());
            am.last_error = Set(String::new());
            am.steps_json = Set("[]".to_string());
            am.updated_at = Set(now);
            am.update(db).await.map_err(db_err)?;
        }
        None => {
            let am = node_monitor::ActiveModel {
                id: Set(ulid::Ulid::new().to_string()),
                node_id: Set(node_id.to_string()),
                component: Set(component.to_string()),
                state: Set(state::DEPLOYING.to_string()),
                service_id: Set(service_id.to_string()),
                service_name: Set(service_name.to_string()),
                image: Set(image.to_string()),
                category: Set(category.to_string()),
                workflow_id: Set(workflow_id.to_string()),
                last_error: Set(String::new()),
                steps_json: Set("[]".to_string()),
                deployed_at: Set(None),
                created_at: Set(now.clone()),
                updated_at: Set(now),
            };
            am.insert(db).await.map_err(db_err)?;
        }
    }
    Ok(())
}

/// 追加一个步骤（读取现有步骤 → 追加 → 写回）
pub async fn append_step(
    db: &DatabaseConnection,
    node_id: &str,
    component: &str,
    step: MonitorStep,
) -> Result<(), ModelinferError> {
    let Some(existing) = get(db, node_id, component).await? else {
        return Ok(());
    };
    let mut steps: Vec<MonitorStep> = serde_json::from_str(&existing.steps_json).unwrap_or_default();
    steps.push(step);
    let mut am: node_monitor::ActiveModel = existing.into();
    am.steps_json = Set(serde_json::to_string(&steps).unwrap_or_else(|_| "[]".into()));
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(db_err)?;
    Ok(())
}

/// 收尾：置终态（deployed / failed）与错误；deployed 时记录部署时间
pub async fn finish(
    db: &DatabaseConnection,
    node_id: &str,
    component: &str,
    state: &str,
    last_error: &str,
) -> Result<(), ModelinferError> {
    let Some(existing) = get(db, node_id, component).await? else {
        return Ok(());
    };
    let mut am: node_monitor::ActiveModel = existing.into();
    am.state = Set(state.to_string());
    am.last_error = Set(last_error.to_string());
    if state == state::DEPLOYED {
        am.deployed_at = Set(Some(time::now_iso()));
    }
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(db_err)?;
    Ok(())
}

/// 列表（供 reconcile / 概览）
pub async fn list(db: &DatabaseConnection) -> Result<Vec<node_monitor::Model>, ModelinferError> {
    node_monitor::Entity::find()
        .all(db)
        .await
        .map_err(db_err)
}

/// 删除节点的全部监控记录（节点删除时）
pub async fn remove(db: &DatabaseConnection, node_id: &str) -> Result<(), ModelinferError> {
    node_monitor::Entity::delete_many()
        .filter(node_monitor::Column::NodeId.eq(node_id.to_string()))
        .exec(db)
        .await
        .map_err(db_err)?;
    Ok(())
}

/// 便捷：直接置失败（Temporal 不可用 / 手工触发失败；按组件 upsert）
pub async fn mark_failed(
    db: &DatabaseConnection,
    node_id: &str,
    component: &str,
    image: &str,
    category: &str,
    error: &str,
) -> Result<(), ModelinferError> {
    let now = time::now_iso();
    match get(db, node_id, component).await? {
        Some(existing) => {
            let mut am: node_monitor::ActiveModel = existing.into();
            am.state = Set(state::FAILED.to_string());
            am.last_error = Set(error.to_string());
            am.updated_at = Set(now);
            am.update(db).await.map_err(db_err)?;
        }
        None => {
            let am = node_monitor::ActiveModel {
                id: Set(ulid::Ulid::new().to_string()),
                node_id: Set(node_id.to_string()),
                component: Set(component.to_string()),
                state: Set(state::FAILED.to_string()),
                service_id: Set(String::new()),
                service_name: Set(String::new()),
                image: Set(image.to_string()),
                category: Set(category.to_string()),
                workflow_id: Set(String::new()),
                last_error: Set(error.to_string()),
                steps_json: Set("[]".to_string()),
                deployed_at: Set(None),
                created_at: Set(now.clone()),
                updated_at: Set(now),
            };
            am.insert(db).await.map_err(db_err)?;
        }
    }
    Ok(())
}

/// 节点视图的监控摘要（聚合多组件）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MonitorSummary {
    pub monitor_state: String,
    pub monitor_last_error: String,
    pub monitor_deployed_at: Option<String>,
}

/// 加载监控摘要映射（node_id → 聚合摘要），用于批量富化节点视图
pub async fn summaries(
    db: &DatabaseConnection,
) -> Result<std::collections::HashMap<String, MonitorSummary>, ModelinferError> {
    let rows = list(db).await?;
    let mut grouped: std::collections::HashMap<String, Vec<node_monitor::Model>> =
        std::collections::HashMap::new();
    for r in rows {
        grouped.entry(r.node_id.clone()).or_default().push(r);
    }
    Ok(grouped
        .into_iter()
        .map(|(node_id, rows)| {
            let states: Vec<String> = rows.iter().map(|r| r.state.clone()).collect();
            let last_error = rows
                .iter()
                .find(|r| r.state == state::FAILED)
                .map(|r| r.last_error.clone())
                .unwrap_or_default();
            let deployed_at = rows.iter().filter_map(|r| r.deployed_at.clone()).max();
            (
                node_id,
                MonitorSummary {
                    monitor_state: aggregate_state(&states),
                    monitor_last_error: last_error,
                    monitor_deployed_at: deployed_at,
                },
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    #[tokio::test]
    async fn begin_append_finish_roundtrip_multi_component() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        view_of(&db, "n1", "node-exporter").await.unwrap(); // 空态
        begin(&db, "n1", "node-exporter", "wf-1", "img:1", "system", "svc-n1", "modelinfer-node-exporter")
            .await
            .unwrap();
        append_step(&db, "n1", "node-exporter", MonitorStep::new("prepare", "准备").done(true, "ok"))
            .await
            .unwrap();
        finish(&db, "n1", "node-exporter", state::DEPLOYED, "").await.unwrap();

        // 第二个组件（GPU）独立记录
        begin(
            &db,
            "n1",
            "nvidia-gpu-exporter",
            "wf-2",
            "img:2",
            "gpu",
            "svc-n1-gpu",
            "modelinfer-nvidia-gpu-exporter",
        )
        .await
        .unwrap();
        finish(&db, "n1", "nvidia-gpu-exporter", state::FAILED, "boom").await.unwrap();

        let views = views_of(&db, "n1").await.unwrap();
        assert_eq!(views.len(), 2);
        let sys = views.iter().find(|v| v.component == "node-exporter").unwrap();
        assert_eq!(sys.state, "deployed");
        assert!(sys.deployed_at.is_some());
        let gpu = views.iter().find(|v| v.component == "nvidia-gpu-exporter").unwrap();
        assert_eq!(gpu.state, "failed");
        assert_eq!(gpu.category, "gpu");

        // 聚合摘要：任一失败 → failed
        let s = summaries(&db).await.unwrap();
        assert_eq!(s.get("n1").unwrap().monitor_state, "failed");
        assert_eq!(s.get("n1").unwrap().monitor_last_error, "boom");

        // 再次 begin 清空步骤（仅该组件）
        begin(&db, "n1", "node-exporter", "wf-3", "img:1", "system", "svc-n1", "modelinfer-node-exporter")
            .await
            .unwrap();
        let v = view_of(&db, "n1", "node-exporter").await.unwrap();
        assert_eq!(v.state, "deploying");
        assert!(v.steps.is_empty());

        remove(&db, "n1").await.unwrap();
        assert!(views_of(&db, "n1").await.unwrap().is_empty());
    }

    #[test]
    fn aggregate_state_rules() {
        assert_eq!(aggregate_state(&[]), "pending");
        assert_eq!(aggregate_state(&["deployed".into(), "failed".into()]), "failed");
        assert_eq!(aggregate_state(&["deployed".into(), "deploying".into()]), "deploying");
        assert_eq!(aggregate_state(&["deployed".into(), "deployed".into()]), "deployed");
        assert_eq!(aggregate_state(&["pending".into(), "deployed".into()]), "pending");
    }

    #[tokio::test]
    async fn mark_failed_upserts() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        mark_failed(&db, "n2", "nvidia-gpu-exporter", "img", "gpu", "boom").await.unwrap();
        let v = view_of(&db, "n2", "nvidia-gpu-exporter").await.unwrap();
        assert_eq!(v.state, "failed");
        assert_eq!(v.last_error, "boom");
        assert_eq!(v.category, "gpu");
    }
}