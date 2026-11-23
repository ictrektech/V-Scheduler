//! 部署健康巡检：由 modelinfer 进程内的周期调度器执行（**不依赖 Temporal**）。
//!
//! 设计要点：健康巡检是幂等的周期作业，不需要 Temporal workflow 的持久化/重放保证；
//! 由进程内 tokio 定时任务直接复用 `DeployDeps`（SSH / DB / 平台）执行，避免长运行
//! workflow 造成的 history 无界增长，并在 Temporal 不可用时仍可持续巡检。

use std::time::Duration;

use sea_orm::EntityTrait;
use serde::{Deserialize, Serialize};

use crate::deploy::activities::{
    BOUNDED_STAGE_SSH_TIMEOUT_SECS, MarkStateReq, apply_state, run_on_node, stage_error,
    synthesized_output,
};
use crate::deploy::{DeployDeps, HEALTH_STAGE, load_runtime};
use crate::entities::deployment;
use crate::error::{ModelinferError, codes};
use crate::observability::{self, NodeRef, StageGlobal, kind, state};
use crate::time;

/// 单次健康巡检结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResp {
    pub ok: bool,
    pub detail: String,
}

/// 执行一次健康巡检并落库（`health_status` + 事件）。
///
/// 未声明 `health-probe` 的卡片视为通过且不写事件（提前返回）。
pub async fn probe_once(
    deps: &DeployDeps,
    deployment_id: &str,
) -> Result<ProbeResp, ModelinferError> {
    let rt = load_runtime(deps, deployment_id).await?;
    if !rt.lifecycle.contains_key(HEALTH_STAGE) {
        return Ok(ProbeResp { ok: true, detail: "未声明 health-probe，跳过".into() });
    }

    // 健康巡检运行记录
    let run_id = observability::store::create_run(
        &deps.db,
        deployment_id,
        kind::HEALTH,
        "scheduler",
        "健康巡检",
        "",
    )
    .await?;

    // 记录健康巡检步骤（进程级调度器，无 Temporal 活动）
    let step_id_name = observability::stage_step_id(HEALTH_STAGE);
    let step_title = observability::step_title(&step_id_name);
    let step = observability::store::start_step(
        &deps.db,
        &observability::StepStartReq {
            run_id: &run_id,
            deployment_id,
            kind: kind::HEALTH,
            step: &step_id_name,
            stage: HEALTH_STAGE,
            title: &step_title,
            orchestrator: observability::orchestrator::SCHEDULER,
            node_id: "",
            node_name: "",
            node_host: "",
        },
    )
    .await
    .ok();

    let mut failures = Vec::new();
    let mut stage_global = StageGlobal::new();
    for node in rt.ordered_stage_nodes(HEALTH_STAGE) {
        let script = rt.stage_script(HEALTH_STAGE);
        let node_ref = NodeRef::new(&node.id, &node.name, &node.host);
        let started_at = time::now_iso();
        let exec = run_on_node(
            deps,
            node,
            &script,
            Duration::from_secs(BOUNDED_STAGE_SSH_TIMEOUT_SECS),
        )
        .await;
        let finished_at = time::now_iso();
        let (output, err) = match exec {
            Ok(out) => {
                let err = if out.exit_code == 0 {
                    None
                } else {
                    Some(stage_error(node, &out))
                };
                (out, err)
            }
            Err(e) => (synthesized_output(&e), Some(e)),
        };
        let record = observability::script_req(
            &run_id,
            deployment_id,
            kind::HEALTH,
            HEALTH_STAGE,
            &node_ref,
            1,
            &output,
            &started_at,
            &finished_at,
        );
        if let Err(e) = observability::record_script(&deps.db, &record, &mut stage_global).await {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "记录健康巡检脚本失败（忽略）");
        }
        if let Some(e) = err {
            failures.push(format!("{}: {}", node.host, e.msg));
        }
    }
    if let Err(e) = observability::finalize_stage(
        &deps.db,
        &run_id,
        deployment_id,
        kind::HEALTH,
        HEALTH_STAGE,
        &stage_global,
    )
    .await
    {
        tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "写入健康巡检全局数据失败（忽略）");
    }

    let ok = failures.is_empty();
    let detail = if ok {
        "健康巡检通过".to_string()
    } else {
        failures.join("; ")
    };
    let health = if ok { "healthy" } else { "unhealthy" };
    if let Err(e) = observability::finish_and_prune(
        &deps.db,
        deployment_id,
        deps.settings_defaults,
        &run_id,
        if ok { state::SUCCEEDED } else { state::FAILED },
        &detail,
        if ok { "" } else { &detail },
    )
    .await
    {
        tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "收尾健康 run 失败（忽略）");
    }
    // 收尾健康巡检步骤
    if let Some(id) = step.as_deref() {
        let outcome = if ok {
            observability::store::finish_step(
                &deps.db,
                id,
                observability::step_state::SUCCEEDED,
                Some(&serde_json::json!({ "detail": detail, "failures": failures.len() })),
                "",
            )
            .await
        } else {
            observability::store::finish_step(
                &deps.db,
                id,
                observability::step_state::FAILED,
                None,
                &detail,
            )
            .await
        };
        if let Err(e) = outcome {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "收尾健康巡检步骤失败（忽略）");
        }
    }
    apply_state(
        &deps.db,
        &MarkStateReq {
            deployment_id: deployment_id.to_string(),
            state: None,
            desired_state: None,
            health_status: Some(health.into()),
            last_error: Some(if ok { String::new() } else { detail.clone() }),
        },
    )
    .await?;
    Ok(ProbeResp { ok, detail })
}

/// 巡检所有「运行中且未在停止」的部署（`state=running` 且 `desired_state != stopping`）。
pub async fn probe_active(deps: &DeployDeps) -> Result<(), ModelinferError> {
    let rows = deployment::Entity::find()
        .all(&deps.db)
        .await
        .map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("查询部署失败: {e}")))?;
    for d in rows {
        if d.state != "running" || d.desired_state == "stopping" {
            continue;
        }
        match probe_once(deps, &d.id).await {
            Ok(r) => tracing::info!(
                target: "modelinfer_deploy",
                deployment = %d.id,
                ok = r.ok,
                detail = %r.detail,
                "健康巡检完成"
            ),
            Err(e) => tracing::warn!(
                target: "modelinfer_deploy",
                deployment = %d.id,
                error = %e.msg,
                "健康巡检失败"
            ),
        }
    }
    Ok(())
}

/// 自动重启调度选项（与健康巡检同环执行，保证「先巡检、后决策」）
#[derive(Default)]
pub struct AutoRestartOptions {
    /// 生命周期控制（`None` = Temporal 不可用：只记录判定，不下发动作）
    pub control: Option<std::sync::Arc<dyn crate::deploy::control::Control>>,
    /// 自动重启配置（backoff 策略）
    pub config: crate::deploy::autorestart::AutoRestartConfig,
}

/// 启动进程级健康巡检调度器（后台 tokio 任务，非阻塞）。
///
/// 周期为 `interval_secs`（最小 5 秒）；每个周期依次执行：
///
/// 1. [`probe_active`]：对所有 `running` 运行配置执行健康巡检（写入 `health_status`）；
/// 2. [`crate::deploy::autorestart::tick`]：对开启自动重启的运行配置决策并下发停止 / 启动
///    （必须在巡检之后，保证决策看到本轮最新健康结果）；
/// 3. 本地反代 reconcile（`running + enabled` 确保监听，否则关闭）。
pub fn spawn_scheduler(deps: DeployDeps, interval_secs: u64, auto: AutoRestartOptions) {
    let interval = interval_secs.max(5);
    tokio::spawn(async move {
        tracing::info!(
            target: "modelinfer_deploy",
            interval,
            auto_restart = auto.control.is_some(),
            "部署健康巡检调度器启动（进程级，非 Temporal）"
        );
        loop {
            tokio::time::sleep(Duration::from_secs(interval)).await;
            if let Err(e) = probe_active(&deps).await {
                tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "健康巡检调度失败");
            }
            // 自动重启：巡检之后决策（同一周期；失败不影响后续 reconcile）
            if let Err(e) = crate::deploy::autorestart::tick(
                &deps.db,
                auto.control.as_deref(),
                &auto.config,
                crate::time::now(),
            )
            .await
            {
                tracing::warn!(target: "modelinfer_autorestart", error = %e.msg, "自动重启调度失败");
            }
            // 顺带 reconcile 本地反代（running+enabled → 确保监听；否则关闭）
            if let Err(e) =
                crate::localproxy::reconcile_all(&deps.db, &deps.local_proxy).await
            {
                tracing::warn!(target: "modelinfer_localproxy", error = %e.msg, "本地反代 reconcile 失败");
            }
        }
    });
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    use sea_orm::{ActiveModelTrait, Set};
    use serde_json::{Map, json};

    use crate::db;
    use crate::deploy::activities::{MarkStateReq, apply_state};
    use crate::deployments::{self, DeploymentCreate};
    use crate::nodes;
    use crate::platform::{DeviceInfo, MockPlatformClient, NfsShareInfo, VolumeInfo};
    use crate::ssh::{MockSshExec, SshExec};
    use crate::time;

    fn platform_ok() -> MockPlatformClient {
        let mut p = MockPlatformClient::new();
        p.app_configs = json!({
            "MODELINFER_MODEL_HUB_PATH": {
                "source_type": "host_path",
                "value": "/share/v1/model_hub",
                "display_path": "/share/v1/model_hub"
            }
        });
        p.volumes = vec![VolumeInfo {
            uuid: "v1".into(),
            name: "v1".into(),
            mount_point: "/share/v1".into(),
            state: "Running".into(),
        }];
        p.nfs_shares = vec![NfsShareInfo {
            uuid: "s-v1".into(),
            volume_uuid: "v1".into(),
            name: "v1".into(),
            mount_point: "/share/v1".into(),
            read_only: false,
            state: "Running".into(),
            mounted: true,
        }];
        p.device = DeviceInfo { ip_addr: vec!["10.0.0.5".into()] };
        p
    }

    async fn seed_card(db: &sea_orm::DatabaseConnection, lifecycle: serde_json::Value) -> String {
        let manifest = json!({
            "id": "com.ictrek.hp-test",
            "version": "1.0.0",
            "model": "org/w",
            "lifecycle": lifecycle,
        });
        let now = time::now_iso();
        crate::entities::plaza_model::ActiveModel {
            id: Set("com.ictrek.hp-test@1.0.0".into()),
            model_id: Set("com.ictrek.hp-test".into()),
            model: Set("org/w".into()),
            name: Set("hp".into()),
            version: Set("1.0.0".into()),
            source: Set("plaza".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/x".into()),
            package_path: Set("/tmp/x/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(db)
        .await
        .unwrap();
        "com.ictrek.hp-test@1.0.0".to_string()
    }

    async fn seed_deployment(
        db: &sea_orm::DatabaseConnection,
        lifecycle: serde_json::Value,
    ) -> String {
        let card = seed_card(db, lifecycle).await;
        let node = nodes::create_node(
            db,
            &MockSshExec::new().with_rule("hostname", 0, "h1"),
            crate::nodes::NodeCreate {
                profile: None,
                name: "n1".into(),
                host: "10.0.0.1".into(),
                ssh_port: 22,
                ssh_user: "root".into(),
                auth_method: "password".into(),
                ssh_password: "x".into(),
                ssh_key: String::new(),
                sudo_password: String::new(),
                hostname: Some("h1".into()),
                socket_ifname: Some("eth0".into()),
                ib_hca: Some(String::new()),
                fscache: false,
                node_exporter_port: crate::nodes::DEFAULT_NODE_EXPORTER_PORT,
                nvidia_gpu_exporter_port: crate::nodes::DEFAULT_NVIDIA_GPU_EXPORTER_PORT,
            },
        )
        .await
        .unwrap()
        .id;
        let tmp = tempfile::TempDir::new().unwrap();
        std::fs::create_dir_all(tmp.path().join("export/hf/org/w/current")).unwrap();
        deployments::create_deployment(
            db,
            tmp.path(),
            DeploymentCreate {
                name: "hp".into(),
                plaza_model_id: card,
                hub_source: "hf".into(),
                node_ids: vec![node],
                configs: Map::new(),
                            ..Default::default()
            },
        )
        .await
        .unwrap()
        .id
    }

    fn deps(ssh: Arc<dyn SshExec>, db: sea_orm::DatabaseConnection) -> DeployDeps {
        DeployDeps {
            db,
            ssh,
            platform: Arc::new(platform_ok()),
            hub_dir: String::new(),
            model_hub_path: String::new(),
            mount_dir: "/mnt/model_hub".into(),
            nfs_server: String::new(),
            image_hub: crate::images::ImageHubConfig::default(),
            settings_defaults: crate::observability::SettingsDefaults::default(),
            vrouter_client: Arc::new(crate::vrouter::MockVrouterClient::new()),
            vrouter_config: crate::vrouter::VrouterConfig::default(),
            local_proxy: Default::default(),
            monitor: crate::monitor::MonitorConfig::default(),
        }
    }

    async fn mark(db: &sea_orm::DatabaseConnection, id: &str, state: &str, desired: &str) {
        apply_state(
            db,
            &MarkStateReq {
                deployment_id: id.to_string(),
                state: Some(state.into()),
                desired_state: Some(desired.into()),
                health_status: None,
                last_error: None,
            },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn probe_active_probes_running_and_skips_others() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, json!({"health-probe": "./hp.sh"})).await;
        let ssh = Arc::new(MockSshExec::new().with_rule("health-probe", 0, "ok\n"));
        let deps = deps(ssh.clone(), db.clone());

        // stopped（初始）→ 不巡检
        probe_active(&deps).await.unwrap();
        assert!(ssh.recorded_calls().is_empty(), "非 running 不应巡检");

        // running → 巡检并置 healthy
        mark(&db, &id, "running", "running").await;
        probe_active(&deps).await.unwrap();
        assert_eq!(ssh.recorded_calls().len(), 1);
        assert!(ssh.recorded_calls()[0].contains("health-probe"));
        assert_eq!(deployments::get_deployment(&db, &id).await.unwrap().health_status, "healthy");

        // desired=stopping → 跳过
        apply_state(
            &db,
            &MarkStateReq {
                deployment_id: id.clone(),
                state: None,
                desired_state: Some("stopping".into()),
                health_status: None,
                last_error: None,
            },
        )
        .await
        .unwrap();
        probe_active(&deps).await.unwrap();
        assert_eq!(ssh.recorded_calls().len(), 1, "停止中不应再巡检");
    }

    #[tokio::test]
    async fn undeclared_health_probe_is_ok_and_silent() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, json!({"start": "./s.sh"})).await;
        mark(&db, &id, "running", "running").await;

        let ssh = Arc::new(MockSshExec::new());
        let deps = deps(ssh.clone(), db.clone());
        let r = probe_once(&deps, &id).await.unwrap();
        assert!(r.ok);
        assert!(ssh.recorded_calls().is_empty(), "未声明 health-probe 不应执行 SSH");
    }

    #[tokio::test]
    async fn probe_records_run_logs_data_and_prunes() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, json!({"health-probe": "./hp.sh"})).await;
        mark(&db, &id, "running", "running").await;
        let ssh = Arc::new(MockSshExec::new().with_rule(
            "health-probe",
            0,
            "modelinfer:x=1\nmodelinfer:{\"global\":{\"g\":1}}\n",
        ));
        let mut deps = deps(ssh, db.clone());
        // 仅保留最近 1 次健康 run
        deps.settings_defaults = crate::observability::SettingsDefaults {
            health_history_keep: 1,
            run_history_keep: 20,
        };
        probe_active(&deps).await.unwrap();
        probe_active(&deps).await.unwrap();

        let runs = crate::observability::store::list_runs(&db, &id, Some(kind::HEALTH), None)
            .await
            .unwrap();
        assert_eq!(runs.len(), 1, "保留策略应裁剪到最近 1 次");
        assert_eq!(runs[0].state, state::SUCCEEDED);
        let logs =
            crate::observability::store::list_run_logs(&db, &id, &runs[0].id, None, None)
                .await
                .unwrap();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].stdout, "modelinfer:x=1\nmodelinfer:{\"global\":{\"g\":1}}\n");
        let g = crate::observability::store::list_global_data(&db, &id, &runs[0].id)
            .await
            .unwrap();
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].data, json!({"g":1}));
        let node = crate::observability::store::list_node_data(&db, &id, &runs[0].id, None)
            .await
            .unwrap();
        assert_eq!(node.len(), 1);
        assert_eq!(node[0].data, json!({"x":"1"}));
        // 健康巡检步骤（scheduler 编排，无 Temporal 活动）
        let steps = crate::observability::store::list_steps(&db, &id, &runs[0].id)
            .await
            .unwrap();
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].step, "stage:health-probe");
        assert_eq!(steps[0].stage, "health-probe");
        assert_eq!(steps[0].orchestrator, crate::observability::orchestrator::SCHEDULER);
        assert_eq!(steps[0].state, crate::observability::step_state::SUCCEEDED);
        assert!(steps[0].finished_at.is_some());
    }

    #[tokio::test]
    async fn marks_unhealthy_on_probe_failure() {
        let db = db::connect("sqlite::memory:").await.unwrap();
        let id = seed_deployment(&db, json!({"health-probe": "./hp.sh"})).await;
        mark(&db, &id, "running", "running").await;

        let ssh = Arc::new(MockSshExec::new().with_rule("health-probe", 1, "boom\n"));
        let deps = deps(ssh, db.clone());
        probe_active(&deps).await.unwrap();
        assert_eq!(deployments::get_deployment(&db, &id).await.unwrap().health_status, "unhealthy");
        let runs = crate::observability::store::list_runs(&db, &id, Some(kind::HEALTH), None)
            .await
            .unwrap();
        let steps = crate::observability::store::list_steps(&db, &id, &runs[0].id)
            .await
            .unwrap();
        assert_eq!(steps[0].state, crate::observability::step_state::FAILED);
        assert!(!steps[0].error.is_empty());
    }
}
