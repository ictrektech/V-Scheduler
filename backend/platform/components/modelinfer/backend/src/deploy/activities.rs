//! 部署编排 Activities
//!
//! 每个 Activity 只接受 `deployment_id`（+ 节点/阶段），内部实时加载运行上下文，
//! 避免 SSH 密码进入 Temporal workflow history。

use std::sync::Arc;
use std::time::Duration;

use sea_orm::{ActiveModelTrait, Set};
use serde::{Deserialize, Serialize};
use temporalio_common::error::ApplicationFailure;
use temporalio_macros::activities;
use temporalio_sdk::activities::{ActivityContext, ActivityError};

use crate::deploy::script::{build_env_script, build_stage_script, stage_file};
use crate::deploy::{
    DeployDeps, DeployNode, DeployPlan, DeployRuntime, DEFAULT_STAGE_ORDER,
    load_runtime, node_order_for, normalize_stage_order, plan_of,
};
use crate::deployments;
use crate::entities::deployment;
use crate::monitor;
use crate::error::{ModelinferError, codes};
use crate::images::{self, ImageEnsureReport, RealImageHubClient, RealSourceIpResolver};
use crate::nfs::{MountNode, mount_on_nodes_protected, umount_on_nodes};
use crate::nodes;
use crate::observability::{self, NodeRef, StageGlobal, kind, state};
use crate::plaza;
use crate::ssh::{SshOutput, SshTarget};
use crate::time;

/// 仅 deployment_id 的请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentIdReq {
    pub deployment_id: String,
    /// 运行 ID（由 workflow 从 `prepare` 计划回传；首次 `prepare` 为空）
    #[serde(default)]
    pub run_id: String,
}

/// 启动工作流请求：`trigger` 区分人工启动与自动重启（进入启动 run 记录）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentStartReq {
    pub deployment_id: String,
    /// 启动来源：`manual`（人工）/ `auto-restart`（探活失败自动重启）
    #[serde(default = "default_manual_trigger")]
    pub trigger: String,
    /// 运行 ID（由 workflow 从 `prepare` 计划回传；首次 `prepare` 为空）
    #[serde(default)]
    pub run_id: String,
}

fn default_manual_trigger() -> String {
    "manual".to_string()
}

/// 停止工作流请求：`force` 表示用户显式触发的「强制停止」（用于清除异常状态）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeploymentStopReq {
    pub deployment_id: String,
    #[serde(default)]
    pub force: bool,
}

/// 有界阶段 SSH 命令超时（健康巡检 / 停止清理等）：
/// 阶段自身不应无限阻塞，此处给一个有界上限，与 Activity / 调度超时对齐。
pub(crate) const BOUNDED_STAGE_SSH_TIMEOUT_SECS: u64 = 300;

/// 节点相关请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeReq {
    pub deployment_id: String,
    pub node_id: String,
    /// 运行 ID
    #[serde(default)]
    pub run_id: String,
}

/// 阶段相关请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageReq {
    pub deployment_id: String,
    pub stage: String,
    /// 启动运行 ID（记录脚本日志与结构化数据）
    #[serde(default)]
    pub run_id: String,
}

/// 运行收尾请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinishRunReq {
    pub deployment_id: String,
    pub run_id: String,
    pub state: String,
    pub summary: String,
    pub error: String,
}

/// 统一网关注册尝试结果（进入 workflow history 的轻量产物）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrouterAttempt {
    pub state: String,
    pub upstream_id: String,
    pub model_id: String,
    pub last_error: String,
}

/// 推理引擎探测结果（进入 workflow history 的轻量产物）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineAttempt {
    /// 引擎字面值（vllm / unknown）
    pub engine: String,
    pub version: String,
    /// script / probe / unknown
    pub source: String,
}

/// 推理指标注册尝试结果
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsAttempt {
    /// registered / error / unsupported
    pub state: String,
    pub service_id: String,
    pub last_error: String,
}

/// 节点监控部署请求（按组件）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeMonitorReq {
    pub node_id: String,
    /// 组件标识（`node-exporter` / `nvidia-gpu-exporter`；缺省 `node-exporter` 向后兼容）
    #[serde(default = "default_monitor_component")]
    pub component: String,
}

/// 缺省监控组件（向后兼容旧工作流输入）
fn default_monitor_component() -> String {
    crate::monitor::ComponentKind::NodeExporter.as_str().to_string()
}

/// 节点监控失败请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MonitorFailReq {
    pub node_id: String,
    #[serde(default = "default_monitor_component")]
    pub component: String,
    pub stage: String,
    pub error: String,
}

/// 状态更新请求
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarkStateReq {
    pub deployment_id: String,
    pub state: Option<String>,
    pub desired_state: Option<String>,
    pub health_status: Option<String>,
    pub last_error: Option<String>,
}

/// 部署 Activities（持有共享依赖）
#[derive(Clone)]
pub struct DeployActivities {
    deps: Arc<DeployDeps>,
}

impl DeployActivities {
    pub fn new(deps: DeployDeps) -> Self {
        Self { deps: Arc::new(deps) }
    }

    /// 开始一个运行步骤（记录失败仅告警，返回 `Some(step_id)`；`run_id` 为空则跳过）
    pub(crate) async fn begin_step(
        &self,
        deployment_id: &str,
        run_id: &str,
        kind: &str,
        step: &str,
        stage: &str,
        node: Option<&DeployNode>,
    ) -> Option<String> {
        if run_id.is_empty() {
            return None;
        }
        let title = observability::step_title(step);
        let req = observability::StepStartReq {
            run_id,
            deployment_id,
            kind,
            step,
            stage,
            title: &title,
            orchestrator: observability::orchestrator::TEMPORAL,
            node_id: node.map(|n| n.id.as_str()).unwrap_or(""),
            node_name: node.map(|n| n.name.as_str()).unwrap_or(""),
            node_host: node.map(|n| n.host.as_str()).unwrap_or(""),
        };
        match observability::store::start_step(&self.deps.db, &req).await {
            Ok(id) => Some(id),
            Err(e) => {
                tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "记录运行步骤失败（忽略并继续）");
                None
            }
        }
    }

    /// 收尾运行步骤为成功（记录失败仅告警）
    pub(crate) async fn end_step_ok(&self, step_id: Option<&str>, detail: serde_json::Value) {
        let Some(id) = step_id else { return };
        if let Err(e) = observability::store::finish_step(
            &self.deps.db,
            id,
            observability::step_state::SUCCEEDED,
            Some(&detail),
            "",
        )
        .await
        {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "收尾运行步骤失败（忽略）");
        }
    }

    /// 收尾运行步骤为失败（记录失败仅告警）
    pub(crate) async fn end_step_err(&self, step_id: Option<&str>, msg: &str) {
        let Some(id) = step_id else { return };
        if let Err(e) = observability::store::finish_step(
            &self.deps.db,
            id,
            observability::step_state::FAILED,
            None,
            msg,
        )
        .await
        {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "收尾运行步骤失败（忽略）");
        }
    }

    /// `ensure_images` 主体（不含步骤记录）
    async fn ensure_images_impl(
        &self,
        req: &DeploymentIdReq,
    ) -> Result<ImageEnsureReport, ActivityError> {
        let rt = load_runtime(&self.deps, &req.deployment_id).await.map_err(map_err)?;
        if rt.manifest_images.is_empty() {
            return Ok(ImageEnsureReport::default());
        }
        let candidates = self
            .deps
            .platform
            .device_info()
            .await
            .map(|d| d.ip_addr)
            .unwrap_or_default();
        let hub = RealImageHubClient::from_config(&self.deps.image_hub);
        let report = match images::ensure_images(
            self.deps.ssh.as_ref(),
            &hub,
            &RealSourceIpResolver,
            &rt,
            &self.deps.image_hub,
            &candidates,
        )
        .await
        {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    target: "modelinfer_deploy",
                    code = e.code,
                    error = %e.msg,
                    "镜像预检/分发失败"
                );
                return Err(act_err(e));
            }
        };

        Ok(report)
    }

    /// `mount_nfs` 主体（不含步骤记录），返回结果摘要
    async fn mount_nfs_impl(
        &self,
        req: &DeploymentIdReq,
    ) -> Result<serde_json::Value, ActivityError> {
        let rt = load_runtime(&self.deps, &req.deployment_id).await.map_err(map_err)?;
        let nodes = mount_nodes_of(&rt);
        if nodes.is_empty() {
            return Ok(serde_json::json!({
                "mount_source": "",
                "nodes": 0,
                "message": "仅本机节点，跳过 NFS 挂载",
            }));
        }
        // 共享节点保护：若本次挂载失败需要回滚，不得卸载其他活跃运行配置正在使用的 NFS。
        let protected = other_active_node_ids(&self.deps.db, &rt.deployment_id)
            .await
            .map_err(map_err)?;
        mount_on_nodes_protected(
            self.deps.ssh.as_ref(),
            &rt.nfs,
            &nodes,
            &rt.mount_dir,
            &protected,
        )
        .await
        .map_err(map_err)?;
        Ok(serde_json::json!({
            "mount_source": rt.nfs.mount_source,
            "nodes": nodes.len(),
            "shared_protected_nodes": protected.len(),
        }))
    }

    /// `distribute_model` 主体（不含步骤记录），返回结果摘要
    async fn distribute_model_impl(
        &self,
        req: &DeploymentIdReq,
    ) -> Result<serde_json::Value, ActivityError> {
        let rt = load_runtime(&self.deps, &req.deployment_id).await.map_err(map_err)?;
        let pkg = std::path::Path::new(&rt.package_path);
        if !pkg.exists() {
            return Err(act_err(ModelinferError::new(
                codes::MODEL_DISTRIBUTION_FAILED,
                format!("模型包不存在: {}", rt.package_path),
            )));
        }
        let file_overrides =
            deployments::file_overrides::for_deployment(&self.deps.db, &req.deployment_id)
                .await
                .map_err(map_err)?;
        for node in rt.ordered_nodes() {
            let mk = self
                .deps
                .ssh
                .run(&target_of(node), &format!("mkdir -p {}/.modelinfer", rt.workdir))
                .await
                .map_err(map_err)?;
            if !mk.success() {
                return Err(act_err(ModelinferError::new(
                    codes::MODEL_DISTRIBUTION_FAILED,
                    format!("节点 {} 创建目录失败: {}", node.host, mk.stderr.trim()),
                )));
            }
            self.deps
                .ssh
                .upload(&target_of(node), pkg, &rt.remote_package())
                .await
                .map_err(map_err)?;
            let untar = self
                .deps
                .ssh
                .run(
                    &target_of(node),
                    &format!("tar xzf {} -C {}", rt.remote_package(), rt.workdir),
                )
                .await
                .map_err(map_err)?;
            if !untar.success() {
                return Err(act_err(ModelinferError::new(
                    codes::MODEL_DISTRIBUTION_FAILED,
                    format!("节点 {} 解压失败: {}", node.host, untar.stderr.trim()),
                )));
            }
            // 卡片文件 override：模型包解压完成后、生命周期脚本执行前按相对路径覆盖。
            for (rel, content) in &file_overrides {
                let remote = format!("{}/{}", rt.workdir, rel);
                upload_script(&self.deps, node, &remote, content)
                    .await
                    .map_err(map_err)?;
                if rel.ends_with(".sh") {
                    let chmod = self
                        .deps
                        .ssh
                        .run(
                            &target_of(node),
                            &format!(
                                "chmod 0755 -- {}",
                                crate::deploy::script::sh_quote(&remote)
                            ),
                        )
                        .await
                        .map_err(map_err)?;
                    if !chmod.success() {
                        return Err(act_err(ModelinferError::new(
                            codes::MODEL_DISTRIBUTION_FAILED,
                            format!(
                                "节点 {} 设置 override 脚本可执行权限失败: {}",
                                node.host,
                                chmod.stderr.trim()
                            ),
                        )));
                    }
                }
            }
            // env.sh
            let env_content = build_env_script(&rt, node);
            upload_script(&self.deps, node, &rt.env_script(), &env_content)
                .await
                .map_err(map_err)?;
            // 阶段脚本（全量，便于停止阶段）
            for stage in crate::deploy::ALL_STAGES {
                if let Some(script) = rt.lifecycle.get(stage) {
                    let content = build_stage_script(script, &rt.env_script()).map_err(map_err)?;
                    let remote = format!("{}/.modelinfer/{}", rt.workdir, stage_file(stage));
                    upload_script(&self.deps, node, &remote, &content).await.map_err(map_err)?;
                }
            }
        }
        Ok(serde_json::json!({
            "nodes": rt.nodes.len(),
            "file_overrides": file_overrides.len(),
        }))
    }

    /// `run_stage` 主体（不含步骤记录）：逐节点执行阶段脚本，记录日志 / 结构化数据
    async fn run_stage_body(
        &self,
        rt: &DeployRuntime,
        req: &StageReq,
    ) -> Result<(), ActivityError> {
        let mut stage_global = StageGlobal::new();
        for node in rt.ordered_stage_nodes(&req.stage) {
            let script = rt.stage_script(&req.stage);
            let node_ref = NodeRef::new(&node.id, &node.name, &node.host);
            // 阶段脚本可能长时间阻塞（如 head 等待服务就绪），SSH 命令超时按 manifest
            // 声明的阶段超时放宽，避免被缺省 90s 强杀。
            let timeout = Duration::from_secs(rt.stage_timeout_secs(&req.stage));
            let mut last_err = None;
            for attempt in 1..=2 {
                let started_at = time::now_iso();
                let exec = run_on_node(&self.deps, node, &script, timeout).await;
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
                    &req.run_id,
                    &req.deployment_id,
                    kind::START,
                    &req.stage,
                    &node_ref,
                    attempt,
                    &output,
                    &started_at,
                    &finished_at,
                );
                if let Err(e) =
                    observability::record_script(&self.deps.db, &record, &mut stage_global).await
                {
                    tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "记录脚本执行失败（忽略）");
                }
                match err {
                    None => {
                        last_err = None;
                        break;
                    }
                    Some(e) => {
                        last_err = Some(e);
                    }
                }
            }
            if let Some(e) = last_err {
                return Err(act_err(e));
            }
        }
        if let Err(e) = observability::finalize_stage(
            &self.deps.db,
            &req.run_id,
            &req.deployment_id,
            kind::START,
            &req.stage,
            &stage_global,
        )
        .await
        {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "写入阶段全局数据失败（忽略）");
        }
        Ok(())
    }

    /// `register_vrouter` 主体（不含步骤记录）
    async fn register_vrouter_impl(
        &self,
        req: &DeploymentIdReq,
    ) -> Result<VrouterAttempt, ActivityError> {
        let d = crate::vrouter::ensure_registered_force(
            &self.deps.db,
            self.deps.platform.as_ref(),
            self.deps.vrouter_client.as_ref(),
            &self.deps.vrouter_config,
            &req.deployment_id,
        )
        .await
        .map_err(map_err)?;
        // 本地反代：启动成功后一并确保监听（best-effort；失败仅记录，不影响启动）
        if let Err(e) = crate::localproxy::ensure(
            &self.deps.db,
            &self.deps.local_proxy,
            &req.deployment_id,
        )
        .await
        {
            tracing::warn!(
                target: "modelinfer_localproxy",
                deployment = %req.deployment_id,
                error = %e.msg,
                "本地反代 ensure 失败"
            );
        }
        Ok(VrouterAttempt {
            state: d.vrouter_state,
            upstream_id: d.vrouter_upstream_id,
            model_id: d.vrouter_model,
            last_error: d.vrouter_last_error,
        })
    }

    /// `detect_inference_engine` 主体（不含步骤记录）
    async fn detect_engine_impl(
        &self,
        req: &DeploymentIdReq,
    ) -> Result<EngineAttempt, ActivityError> {
        let d = deployments::get_deployment(&self.deps.db, &req.deployment_id)
            .await
            .map_err(map_err)?;
        // 1. 卡片脚本回传（读取本次 start 阶段的阶段全局对象）
        let from_script = if req.run_id.is_empty() {
            None
        } else {
            observability::store::global_of_stage(&self.deps.db, &d.id, &req.run_id, "start")
                .await
                .ok()
                .flatten()
                .and_then(|g| crate::inference::engine_from_global(&g))
        };
        // 2. 缺失则回落到卡片 manifest 声明的引擎（`inference-engine`）
        let from_manifest = if from_script.is_some() {
            None
        } else {
            plaza::get_model(&self.deps.db, &d.plaza_model_id)
                .await
                .ok()
                .and_then(|c| serde_json::from_str::<serde_json::Value>(&c.manifest_json).ok())
                .and_then(|m| crate::inference::engine_from_manifest(&m))
        };
        // 3. 仍缺失则端口探测推断
        let info = match from_script {
            Some(i) => i,
            None => match from_manifest {
                Some(i) => i,
                None => self.probe_engine(&d).await,
            },
        };
        crate::inference::store::save_engine(&self.deps.db, &d.id, &info)
            .await
            .map_err(map_err)?;
        Ok(EngineAttempt {
            engine: info.engine.as_str().to_string(),
            version: info.version,
            source: info.source,
        })
    }

    /// 对 head 节点执行端口探测（失败落 `unknown`，不报错）
    async fn probe_engine(&self, d: &deployment::Model) -> crate::inference::EngineInfo {
        let Ok(node) = nodes::get_node(&self.deps.db, &d.head_node_id).await else {
            return crate::inference::EngineInfo::unknown();
        };
        let target = nodes::ssh_target_of(&node);
        let port = u16::try_from(d.serve_port.max(1)).unwrap_or(8000);
        let cmd = crate::inference::probe_command(port);
        match self
            .deps
            .ssh
            .run_with_timeout(
                &target,
                &cmd,
                Duration::from_secs(crate::inference::PROBE_TIMEOUT_SECS),
            )
            .await
        {
            Ok(o) => {
                let probe = crate::inference::parse_probe_output(&o.stdout);
                crate::inference::infer_engine_from_probe(&probe)
            }
            Err(_) => crate::inference::EngineInfo::unknown(),
        }
    }

    /// `register_inference_metrics` 主体（不含步骤记录）
    async fn register_metrics_impl(
        &self,
        req: &DeploymentIdReq,
    ) -> Result<MetricsAttempt, ActivityError> {
        let d = deployments::get_deployment(&self.deps.db, &req.deployment_id)
            .await
            .map_err(map_err)?;
        if !crate::inference::engine_metrics_supported(&d.inference_engine) {
            crate::inference::store::mark_unsupported(&self.deps.db, &d.id)
                .await
                .map_err(map_err)?;
            return Ok(MetricsAttempt {
                state: crate::inference::metrics_state::UNSUPPORTED.to_string(),
                service_id: String::new(),
                last_error: String::new(),
            });
        }
        let node = nodes::get_node(&self.deps.db, &d.head_node_id)
            .await
            .map_err(map_err)?;
        let dep = crate::inference::deployment_of(&d, &node.host);
        let svc = dep.service_spec();
        let consul = monitor::consul::RealConsulClient::new(self.deps.monitor.consul());
        match monitor::consul::ConsulClient::register(&consul, &svc).await {
            Ok(()) => {
                crate::inference::store::mark_registered(&self.deps.db, &d.id, &svc.id)
                    .await
                    .map_err(map_err)?;
                Ok(MetricsAttempt {
                    state: crate::inference::metrics_state::REGISTERED.to_string(),
                    service_id: svc.id,
                    last_error: String::new(),
                })
            }
            Err(e) => {
                let _ = crate::inference::store::mark_error(&self.deps.db, &d.id, &e.msg).await;
                tracing::warn!(
                    target: "modelinfer_inference",
                    deployment = %d.id,
                    code = e.code,
                    error = %e.msg,
                    "推理指标注册失败（由 reconcile 兜底）"
                );
                Ok(MetricsAttempt {
                    state: crate::inference::metrics_state::ERROR.to_string(),
                    service_id: String::new(),
                    last_error: e.msg,
                })
            }
        }
    }
}

/// 注销运行配置的推理指标 consul 注册（best-effort；服务不存在视为成功），并置回未注册状态。
///
/// 供停止清理（`cleanup`）与删除运行配置复用。
pub async fn deregister_inference_metrics(
    db: &sea_orm::DatabaseConnection,
    monitor_cfg: &crate::monitor::MonitorConfig,
    deployment_id: &str,
) -> Result<(), ModelinferError> {
    let d = match crate::deployments::get_deployment(db, deployment_id).await {
        Ok(d) => d,
        // 运行配置已删除 / 不存在：无注册可清理，视为成功
        Err(_) => return Ok(()),
    };
    if crate::inference::engine_metrics_supported(&d.inference_engine)
        || !d.metrics_service_id.trim().is_empty()
    {
        // 注销候选：优先台账记录的服务 ID，其次按当前引擎推导；均不可用时对两引擎前缀依次尝试
        // （幂等：服务不存在视为成功），避免引擎字段漂移后残留注册。
        let mut ids: Vec<String> = Vec::new();
        if !d.metrics_service_id.trim().is_empty() {
            ids.push(d.metrics_service_id.clone());
        }
        let derived = crate::inference::metrics_service_id(&d.inference_engine, &d.id);
        if !derived.is_empty() && !ids.contains(&derived) {
            ids.push(derived);
        }
        if ids.is_empty() {
            ids.extend(
                crate::inference::all_service_id_prefixes()
                    .iter()
                    .map(|p| format!("{p}{}", d.id)),
            );
        }
        let consul = monitor::consul::RealConsulClient::new(monitor_cfg.consul());
        for svc_id in &ids {
            if let Err(e) = monitor::consul::ConsulClient::deregister(&consul, svc_id).await {
                tracing::warn!(
                    target: "modelinfer_inference",
                    deployment = %d.id,
                    code = e.code,
                    error = %e.msg,
                    "推理指标 consul 注销失败（由 reconcile 兜底）"
                );
            }
        }
    }
    crate::inference::store::clear(db, &d.id).await?;
    Ok(())
}

fn act_err(e: ModelinferError) -> ActivityError {
    ActivityError::from(ApplicationFailure::non_retryable(anyhow::anyhow!(
        "[{}] {}",
        e.code,
        e.msg
    )))
}

/// 提取 `ActivityError` 的可读信息（用于步骤 error 字段）
fn act_err_msg(e: &ActivityError) -> String {
    match e {
        ActivityError::Application(app) => app.to_string(),
        other => format!("{other:?}"),
    }
}

fn map_err(e: ModelinferError) -> ActivityError {
    act_err(e)
}

/// 更新部署状态字段（仅更新 Some 字段）
pub async fn apply_state(
    db: &sea_orm::DatabaseConnection,
    req: &MarkStateReq,
) -> Result<(), ModelinferError> {
    let current = deployments::get_deployment(db, &req.deployment_id).await?;
    let mut am: deployment::ActiveModel = current.into();
    if let Some(s) = &req.state {
        am.state = Set(s.clone());
    }
    if let Some(d) = &req.desired_state {
        am.desired_state = Set(d.clone());
    }
    if let Some(h) = &req.health_status {
        am.health_status = Set(h.clone());
    }
    if let Some(e) = &req.last_error {
        am.last_error = Set(e.clone());
    }
    am.updated_at = Set(time::now_iso());
    am.update(db).await.map_err(|e| ModelinferError::new(codes::DB_FAILED, format!("更新部署状态失败: {e}")))?;
    Ok(())
}

/// 其他活跃运行配置引用的节点 id（去重、排序）
///
/// 用于共享节点上的 NFS 卸载保护：同一节点可被多个运行配置同时使用，停止 / 回滚一个配置
/// 时不得卸载其他活跃配置仍需要的挂载。
async fn other_active_node_ids(
    db: &sea_orm::DatabaseConnection,
    deployment_id: &str,
) -> Result<Vec<String>, ModelinferError> {
    let mut ids: Vec<String> = deployments::occupied_nodes(db, deployment_id)
        .await?
        .into_iter()
        .map(|o| o.node_id)
        .collect();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn mount_nodes_of(rt: &DeployRuntime) -> Vec<MountNode> {
    rt.nodes
        .iter()
        .filter(|n| !DeployRuntime::is_local_node(n))
        .map(|n| MountNode {
            id: n.id.clone(),
            name: n.name.clone(),
            host: n.host.clone(),
            port: n.port,
            user: n.user.clone(),
            auth_method: n.auth_method.clone(),
            password: n.password.clone(),
            ssh_key: n.ssh_key.clone(),
            sudo_password: n.sudo_password.clone(),
            fscache: n.fscache,
        })
        .collect()
}

pub(crate) fn target_of(n: &DeployNode) -> SshTarget {
    if DeployRuntime::is_local_node(n) {
        return SshTarget::local(n.host.clone());
    }
    SshTarget::from_credentials(
        n.host.clone(),
        n.port,
        n.user.clone(),
        &n.auth_method,
        &n.password,
        &n.ssh_key,
    ).with_sudo_password(&n.sudo_password)
}

/// 在节点上执行阶段脚本（返回完整输出；非零退出**不**在此处报错，由调用点判定）
pub(crate) async fn run_on_node(
    deps: &DeployDeps,
    node: &DeployNode,
    script: &str,
    timeout: Duration,
) -> Result<SshOutput, ModelinferError> {
    deps.ssh
        .run_with_timeout(&target_of(node), &format!("bash {script}"), timeout)
        .await
}

/// 非零退出 → 阶段失败错误
pub(crate) fn stage_error(node: &DeployNode, out: &SshOutput) -> ModelinferError {
    ModelinferError::new(
        codes::LIFECYCLE_STAGE_FAILED,
        format!(
            "节点 {} 执行失败(exit={}): {}{}",
            node.host,
            out.exit_code,
            out.stdout.trim(),
            out.stderr.trim()
        ),
    )
}

/// 传输层失败时的合成输出（用于记录一条日志）
pub(crate) fn synthesized_output(err: &ModelinferError) -> SshOutput {
    SshOutput {
        exit_code: -1,
        stdout: String::new(),
        stderr: err.msg.clone(),
    }
}

/// 写入本地临时脚本文件并上传到节点
async fn upload_script(
    deps: &DeployDeps,
    node: &DeployNode,
    remote: &str,
    content: &str,
) -> Result<(), ModelinferError> {
    let dir = std::env::temp_dir().join("modelinfer-deploy");
    std::fs::create_dir_all(&dir)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("创建临时目录失败: {e}")))?;
    let local = dir.join(format!("{}-{}", ulid::Ulid::new(), remote.rsplit('/').next().unwrap_or("script")));
    std::fs::write(&local, content)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("写临时脚本失败: {e}")))?;
    deps.ssh.upload(&target_of(node), &local, remote).await?;
    let _ = std::fs::remove_file(&local);
    Ok(())
}

#[activities]
impl DeployActivities {
    /// 准备：加载上下文、创建启动 run、置 starting、记录 `prepare` 步骤
    #[activity]
    pub async fn prepare(
        self: Arc<Self>,
        ctx: ActivityContext,
        req: DeploymentStartReq,
    ) -> Result<DeployPlan, ActivityError> {
        // Temporal 工作流 id：仅作按需查询句柄（不落 activity 历史）
        let workflow_id = ctx
            .info()
            .workflow_execution
            .as_ref()
            .map(|e| e.workflow_id.clone())
            .unwrap_or_default();
        // 创建启动运行记录（run_id 经计划回传，避免 workflow 生成非确定性标识）。
        // 必须先于 load_runtime：启动失败（含 prepare 早期失败）必须留下 failed run，
        // 自动重启据此识别「首次启动失败」。
        let trigger = if req.trigger.trim().is_empty() {
            "manual"
        } else {
            req.trigger.trim()
        };
        let run_id = observability::store::create_run(
            &self.deps.db,
            &req.deployment_id,
            kind::START,
            trigger,
            if trigger == "auto-restart" {
                "自动重启：开始启动"
            } else {
                "开始启动"
            },
            &workflow_id,
        )
        .await
        .map_err(map_err)?;
        let rt = match load_runtime(&self.deps, &req.deployment_id).await {
            Ok(rt) => rt,
            Err(e) => {
                let _ = observability::finish_and_prune(
                    &self.deps.db,
                    &req.deployment_id,
                    self.deps.settings_defaults,
                    &run_id,
                    state::FAILED,
                    &format!("准备阶段失败: {}", e.msg),
                    &e.msg,
                )
                .await;
                return Err(map_err(e));
            }
        };
        let plan = plan_of(&rt, &run_id);
        let step = self
            .begin_step(&req.deployment_id, &run_id, kind::START, "prepare", "", None)
            .await;
        let outcome = apply_state(
            &self.deps.db,
            &MarkStateReq {
                deployment_id: req.deployment_id.clone(),
                state: Some("starting".into()),
                desired_state: Some("running".into()),
                health_status: Some("unknown".into()),
                last_error: Some(String::new()),
            },
        )
        .await
        .map_err(map_err);
        match &outcome {
            Ok(()) => {
                self.end_step_ok(
                    step.as_deref(),
                    serde_json::json!({
                        "nodes": rt.nodes.len(),
                        "head": rt.head_ip,
                        "model_path": rt.model_path,
                        "stages": plan.stages_start,
                    }),
                )
                .await;
            }
            Err(e) => {
                let detail = act_err_msg(e);
                self.end_step_err(step.as_deref(), &detail).await;
                let _ = observability::finish_and_prune(
                    &self.deps.db,
                    &req.deployment_id,
                    self.deps.settings_defaults,
                    &run_id,
                    state::FAILED,
                    &format!("准备阶段失败: {detail}"),
                    &detail,
                )
                .await;
            }
        }
        outcome?;
        Ok(plan)
    }

    /// 检查单节点 SSH 可连接性
    #[activity]
    pub async fn check_node(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: NodeReq,
    ) -> Result<(), ActivityError> {
        let rt = load_runtime(&self.deps, &req.deployment_id).await.map_err(map_err)?;
        let node = rt
            .node(&req.node_id)
            .ok_or_else(|| act_err(ModelinferError::new(codes::NOT_FOUND, "节点不在部署配置中")))?;
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, "check-node", "", Some(node))
            .await;
        let outcome = self.deps.ssh.test(&target_of(node)).await.map_err(map_err);
        match &outcome {
            Ok(()) => {
                self.end_step_ok(step.as_deref(), serde_json::json!({ "host": node.host })).await;
            }
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome
    }

    /// 启动前镜像预检 / 分发：逐节点检查卡片 `manifest.images`，缺失时经 image-hub 拉取
    ///
    /// 失败语义：hub 上亦不存在 → `20728`（中止，不拉取）；registry 不可达 → `20730`；
    /// 拉取失败 / 拉取后仍缺失 → `20729`。失败时尚未挂载 NFS / 分发模型，无需清理。
    #[activity]
    pub async fn ensure_images(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: DeploymentIdReq,
    ) -> Result<ImageEnsureReport, ActivityError> {
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, "ensure-images", "", None)
            .await;
        let outcome = self.ensure_images_impl(&req).await;
        match &outcome {
            Ok(rep) => {
                self.end_step_ok(
                    step.as_deref(),
                    serde_json::json!({
                        "node_count": rep.node_count,
                        "image_count": rep.image_count,
                        "nodes_missing": rep.nodes_missing,
                        "pulls": rep.pulls.len(),
                    }),
                )
                .await;
            }
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome
    }

    /// 在各节点挂载 NFS
    #[activity]
    pub async fn mount_nfs(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: DeploymentIdReq,
    ) -> Result<(), ActivityError> {
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, "mount-nfs", "", None)
            .await;
        let outcome = self.mount_nfs_impl(&req).await;
        match &outcome {
            Ok(detail) => self.end_step_ok(step.as_deref(), detail.clone()).await,
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome.map(|_| ())
    }

    /// 分发 model.tar.gz + 环境/阶段脚本到各节点
    #[activity]
    pub async fn distribute_model(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: DeploymentIdReq,
    ) -> Result<(), ActivityError> {
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, "distribute", "", None)
            .await;
        let outcome = self.distribute_model_impl(&req).await;
        match &outcome {
            Ok(detail) => self.end_step_ok(step.as_deref(), detail.clone()).await,
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome.map(|_| ())
    }

    /// 在单节点执行某阶段脚本（失败重试 1 次），head 优先、其余顺序执行
    ///
    /// 每次执行（含重试）记录脚本日志；阶段结束写入全局结构化数据。
    #[activity]
    pub async fn run_stage(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: StageReq,
    ) -> Result<(), ActivityError> {
        let rt = load_runtime(&self.deps, &req.deployment_id).await.map_err(map_err)?;
        let Some(_) = rt.lifecycle.get(&req.stage) else {
            // 未声明阶段：跳过（不记录步骤）
            return Ok(());
        };
        let step_id = observability::stage_step_id(&req.stage);
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, &step_id, &req.stage, None)
            .await;
        let node_count = rt.ordered_stage_nodes(&req.stage).len();
        let outcome = self.run_stage_body(&rt, &req).await;
        match &outcome {
            Ok(()) => {
                self.end_step_ok(
                    step.as_deref(),
                    serde_json::json!({ "stage": req.stage, "nodes": node_count }),
                )
                .await;
            }
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome
    }

    /// 收尾运行记录（幂等）
    #[activity]
    pub async fn finish_run(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: FinishRunReq,
    ) -> Result<(), ActivityError> {
        // 兜底：将仍处于 running 的步骤置为失败，避免悬挂步骤（正常路径下应已显式收尾）
        let msg = if req.error.is_empty() { "步骤未显式收尾" } else { req.error.as_str() };
        if let Err(e) = observability::store::fail_running_steps(&self.deps.db, &req.run_id, msg).await
        {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "收尾悬挂步骤失败（忽略）");
        }
        observability::finish_and_prune(
            &self.deps.db,
            &req.deployment_id,
            self.deps.settings_defaults,
            &req.run_id,
            &req.state,
            &req.summary,
            &req.error,
        )
        .await
        .map_err(map_err)?;
        Ok(())
    }

    /// 更新部署状态
    #[activity]
    pub async fn mark_state(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: MarkStateReq,
    ) -> Result<(), ActivityError> {
        apply_state(&self.deps.db, &req).await.map_err(map_err)?;
        Ok(())
    }

    /// 启动后尽力注册到统一网关（best-effort）
    ///
    /// 失败 MUST NOT 使启动工作流失败：仅更新运行配置的注册状态（并在启动 run 中记录步骤）。
    #[activity]
    pub async fn register_vrouter(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: DeploymentIdReq,
    ) -> Result<VrouterAttempt, ActivityError> {
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, "register-vrouter", "", None)
            .await;
        let outcome = self.register_vrouter_impl(&req).await;
        match &outcome {
            Ok(a) => {
                self.end_step_ok(
                    step.as_deref(),
                    serde_json::json!({
                        "state": a.state,
                        "model": a.model_id,
                        "upstream": a.upstream_id,
                        "last_error": a.last_error,
                    }),
                )
                .await;
            }
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome
    }

    /// 启动后推理引擎探测（best-effort）：脚本 `global.inference_engine` → 端口探测 → unknown。
    ///
    /// 结果写回运行配置（引擎 / 版本 / 来源），MUST NOT 使启动工作流失败。
    #[activity]
    pub async fn detect_inference_engine(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: DeploymentIdReq,
    ) -> Result<EngineAttempt, ActivityError> {
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, "detect-engine", "", None)
            .await;
        let outcome = self.detect_engine_impl(&req).await;
        match &outcome {
            Ok(a) => {
                self.end_step_ok(
                    step.as_deref(),
                    serde_json::json!({ "engine": a.engine, "version": a.version, "source": a.source }),
                )
                .await;
            }
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome
    }

    /// 启动后推理指标注册（best-effort）：受支持引擎 → 注册 consul；否则置 `unsupported`。
    ///
    /// 失败 MUST NOT 使启动工作流失败：仅写回注册状态（并在启动 run 中记录步骤）。
    #[activity]
    pub async fn register_inference_metrics(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: DeploymentIdReq,
    ) -> Result<MetricsAttempt, ActivityError> {
        let step = self
            .begin_step(&req.deployment_id, &req.run_id, kind::START, "register-metrics", "", None)
            .await;
        let outcome = self.register_metrics_impl(&req).await;
        match &outcome {
            Ok(a) => {
                self.end_step_ok(
                    step.as_deref(),
                    serde_json::json!({ "state": a.state, "service_id": a.service_id, "last_error": a.last_error }),
                )
                .await;
            }
            Err(e) => self.end_step_err(step.as_deref(), &act_err_msg(e)).await,
        }
        outcome
    }

    /// 停止清理：执行停止阶段（失败继续）→ 解除 NFS 挂载 → state=stopped
    #[activity]
    pub async fn cleanup(
        self: Arc<Self>,
        ctx: ActivityContext,
        req: DeploymentIdReq,
    ) -> Result<(), ActivityError> {
        // 直接加载（不解析 NFS，避免平台不可用时卡住清理）
        let d = deployments::get_deployment(&self.deps.db, &req.deployment_id)
            .await
            .map_err(map_err)?;
        let workflow_id = ctx
            .info()
            .workflow_execution
            .as_ref()
            .map(|e| e.workflow_id.clone())
            .unwrap_or_default();
        let node_ids: Vec<String> = serde_json::from_str(&d.node_ids_json).unwrap_or_default();
        let models = nodes::load_models(&self.deps.db, Some(&node_ids))
            .await
            .map_err(map_err)?;
        let card = plaza::get_model(&self.deps.db, &d.plaza_model_id).await.ok();
        let manifest = card
            .as_ref()
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c.manifest_json).ok())
            .unwrap_or(serde_json::Value::Null);
        let lifecycle = manifest
            .get("lifecycle")
            .and_then(|l| l.as_object())
            .cloned()
            .unwrap_or_default();
        let lifecycle_order = manifest
            .get("lifecycle-order")
            .and_then(|m| m.as_object())
            .cloned()
            .unwrap_or_default();
        let workdir = crate::deploy::deployment_workdir(&d.id);

        let dep_nodes: Vec<DeployNode> = models
            .iter()
            .enumerate()
            .map(|(idx, n)| DeployNode {
                id: n.id.clone(),
                name: n.name.clone(),
                host: n.host.clone(),
                port: n.ssh_port as u16,
                user: n.ssh_user.clone(),
                auth_method: n.auth_method.clone(),
                password: n.ssh_password.clone(),
                ssh_key: n.ssh_key.clone(),
                sudo_password: n.sudo_password.clone(),
                hostname: n.hostname.clone(),
                socket_ifname: n.socket_ifname.clone(),
                ib_hca: n.ib_hca.clone(),
                arch: n.arch.clone(),
                fscache: n.fscache,
                role: if idx == 0 { "head".into() } else { "worker".into() },
            })
            .collect();

        // 停止运行记录（自包含：创建 → 记录 → 收尾）
        let stop_run_id = observability::store::create_run(
            &self.deps.db,
            &d.id,
            kind::STOP,
            "auto",
            "开始停止清理",
            &workflow_id,
        )
        .await
        .map_err(map_err)?;

        // 停止阶段（按 lifecycle-order 声明的节点顺序；单节点失败继续）
        let mut warnings: Vec<String> = Vec::new();
        for stage in crate::deploy::STAGES_STOP {
            if !lifecycle.contains_key(stage) {
                continue;
            }
            let order = normalize_stage_order(
                lifecycle_order
                    .get(stage)
                    .and_then(|v| v.as_str())
                    .unwrap_or(DEFAULT_STAGE_ORDER),
            );
            let refs: Vec<&DeployNode> = dep_nodes.iter().collect();
            let stage_nodes = node_order_for(&refs, &order);
            let step_id = observability::stage_step_id(stage);
            let step = self
                .begin_step(&d.id, &stop_run_id, kind::STOP, &step_id, stage, None)
                .await;
            let mut stage_failed: Option<String> = None;
            let mut stage_global = StageGlobal::new();
            for node in stage_nodes {
                let script = format!("{workdir}/.modelinfer/{}", stage_file(stage));
                let started_at = time::now_iso();
                let exec = run_on_node(
                    &self.deps,
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
                let node_ref = NodeRef::new(&node.id, &node.name, &node.host);
                let record = observability::script_req(
                    &stop_run_id,
                    &d.id,
                    kind::STOP,
                    stage,
                    &node_ref,
                    1,
                    &output,
                    &started_at,
                    &finished_at,
                );
                if let Err(e) =
                    observability::record_script(&self.deps.db, &record, &mut stage_global).await
                {
                    tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "记录停止脚本执行失败（忽略）");
                }
                if let Some(e) = err {
                    let msg = format!("{}@{}: {}", stage, node.host, e.msg);
                    warnings.push(msg.clone());
                    stage_failed = Some(msg);
                }
            }
            if let Err(e) = observability::finalize_stage(
                &self.deps.db,
                &stop_run_id,
                &d.id,
                kind::STOP,
                stage,
                &stage_global,
            )
            .await
            {
                tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "写入停止阶段全局数据失败（忽略）");
            }
            match &stage_failed {
                None => {
                    self.end_step_ok(
                        step.as_deref(),
                        serde_json::json!({ "stage": stage, "nodes": dep_nodes.len() }),
                    )
                    .await;
                }
                Some(msg) => self.end_step_err(step.as_deref(), msg).await,
            }
        }

        // 解除挂载
        let mount_nodes: Vec<MountNode> = dep_nodes
            .iter()
            .filter(|n| !DeployRuntime::is_local_node(n))
            .map(|n| MountNode {
                id: n.id.clone(),
                name: n.name.clone(),
                host: n.host.clone(),
                port: n.port,
                user: n.user.clone(),
                auth_method: n.auth_method.clone(),
                password: n.password.clone(),
                ssh_key: n.ssh_key.clone(),
                sudo_password: n.sudo_password.clone(),
                fscache: n.fscache,
            })
            .collect();
        let umount_step = self
            .begin_step(&d.id, &stop_run_id, kind::STOP, "umount", "", None)
            .await;
        let had_remote_mounts = !mount_nodes.is_empty();
        // 共享节点保护：其他活跃运行配置仍引用该节点时保留 NFS 挂载。
        // DB 读取失败时保守地保护全部节点（宁可残留挂载，也不破坏其他运行中的配置）。
        let protected = other_active_node_ids(&self.deps.db, &d.id)
            .await
            .unwrap_or_else(|_| node_ids.clone());
        let (skipped, to_umount): (Vec<MountNode>, Vec<MountNode>) = mount_nodes
            .into_iter()
            .partition(|n| protected.iter().any(|id| id == &n.id));
        if to_umount.is_empty() {
            let message = if had_remote_mounts {
                "节点均被其他活跃运行配置共享，保留 NFS 挂载"
            } else {
                "仅本机节点，跳过 NFS 卸载"
            };
            self.end_step_ok(
                umount_step.as_deref(),
                serde_json::json!({
                    "nodes": 0,
                    "skipped_shared_nodes": skipped.len(),
                    "message": message,
                }),
            )
            .await;
        } else if let Err(e) =
            umount_on_nodes(self.deps.ssh.as_ref(), &to_umount, &self.deps.mount_dir).await
        {
            warnings.push(format!("umount: {}", e.msg));
            self.end_step_err(umount_step.as_deref(), &e.msg).await;
        } else {
            self.end_step_ok(
                umount_step.as_deref(),
                serde_json::json!({
                    "nodes": to_umount.len(),
                    "skipped_shared_nodes": skipped.len(),
                }),
            )
            .await;
        }

        // 注销推理指标 consul 注册（best-effort；失败由 reconcile 兜底）
        if let Err(e) = deregister_inference_metrics(&self.deps.db, &self.deps.monitor, &d.id).await {
            tracing::warn!(target: "modelinfer_inference", deployment = %d.id, error = %e.msg, "推理指标注销失败（忽略）");
        }

        // 清除容器清单（容器已由停止 / 清理脚本移除）
        if let Err(e) = observability::store::clear_containers(&self.deps.db, &d.id).await {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "清除容器清单失败（忽略）");
        }
        // 收尾可能仍处于 running 的启动 run（停止会终止在途启动工作流）
        if let Err(e) = observability::store::fail_stale_runs(
            &self.deps.db,
            &d.id,
            &stop_run_id,
            "被停止清理中断",
        )
        .await
        {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "收尾在途 run 失败（忽略）");
        }

        let summary = if warnings.is_empty() {
            "部署已停止并完成清理".to_string()
        } else {
            format!("部署已停止（{} 项告警）：{}", warnings.len(), warnings.join("; "))
        };
        if let Err(e) = observability::finish_and_prune(
            &self.deps.db,
            &d.id,
            self.deps.settings_defaults,
            &stop_run_id,
            state::SUCCEEDED,
            &summary,
            &warnings.join("; "),
        )
        .await
        {
            tracing::warn!(target: "modelinfer_deploy", error = %e.msg, "收尾停止 run 失败（忽略）");
        }

        // 关闭本地反代（best-effort；释放监听端口）
        if let Err(e) =
            crate::localproxy::stop(&self.deps.db, &self.deps.local_proxy, &d.id).await
        {
            warnings.push(format!("local-proxy: {}", e.msg));
            tracing::warn!(target: "modelinfer_localproxy", deployment = %d.id, error = %e.msg, "本地反代关闭失败");
        }

        // 尽力注销统一网关注册（best-effort；失败仅告警）
        let dereg_step = self
            .begin_step(&d.id, &stop_run_id, kind::STOP, "deregister-vrouter", "", None)
            .await;
        match crate::vrouter::deregister(
            &self.deps.db,
            self.deps.platform.as_ref(),
            self.deps.vrouter_client.as_ref(),
            &self.deps.vrouter_config,
            &d.id,
        )
        .await
        {
            Ok(Some(w)) => {
                warnings.push(format!("vrouter: {w}"));
                self.end_step_err(dereg_step.as_deref(), &w).await;
            }
            Ok(None) => {
                self.end_step_ok(
                    dereg_step.as_deref(),
                    serde_json::json!({ "state": "deregistered" }),
                )
                .await;
            }
            Err(e) => {
                warnings.push(format!("vrouter: {}", e.msg));
                self.end_step_err(dereg_step.as_deref(), &e.msg).await;
            }
        }

        apply_state(
            &self.deps.db,
            &MarkStateReq {
                deployment_id: d.id.clone(),
                state: Some("stopped".into()),
                desired_state: Some("stopped".into()),
                health_status: Some("unknown".into()),
                last_error: Some(String::new()),
            },
        )
        .await
        .map_err(map_err)?;

        // 共享节点补卸载：并发停止两个共享同一节点的配置时，双方在 umount 阶段可能都看到
        // 对方仍处于 stopping 而跳过；本配置已置 stopped 后再检查一次，若已无其他活跃配置
        // 引用该节点则补执行卸载（best-effort，幂等）。
        if !skipped.is_empty() {
            let protected_after = other_active_node_ids(&self.deps.db, &d.id)
                .await
                .unwrap_or_else(|_| skipped.iter().map(|n| n.id.clone()).collect());
            let retry: Vec<MountNode> = skipped
                .into_iter()
                .filter(|n| !protected_after.iter().any(|id| id == &n.id))
                .collect();
            if !retry.is_empty()
                && let Err(e) =
                    umount_on_nodes(self.deps.ssh.as_ref(), &retry, &self.deps.mount_dir).await
            {
                tracing::warn!(
                    target: "modelinfer_deploy",
                    deployment = %d.id,
                    error = %e.msg,
                    "停止后补卸载共享 NFS 失败（忽略）"
                );
            }
        }
        Ok(())
    }

    // ================= 节点监控组件部署（按组件） =================

    /// 记录一个监控部署步骤（best-effort；步骤记录失败不阻断流程）
    async fn monitor_record(
        &self,
        node_id: &str,
        component: &str,
        name: &str,
        title: &str,
        result: &Result<String, ModelinferError>,
    ) {
        let step = match result {
            Ok(detail) => monitor::store::MonitorStep::new(name, title).done(true, detail.clone()),
            Err(e) => monitor::store::MonitorStep::new(name, title).done(false, e.msg.clone()),
        };
        if let Err(e) = monitor::store::append_step(&self.deps.db, node_id, component, step).await {
            tracing::warn!(target: "modelinfer_monitor", error = %e.msg, "记录监控步骤失败（忽略）");
        }
    }

    /// 解析请求中的组件种类（未知 → node-exporter，防御式）
    fn component_kind(req: &NodeMonitorReq) -> monitor::ComponentKind {
        monitor::ComponentKind::parse(&req.component).unwrap_or(monitor::ComponentKind::NodeExporter)
    }

    /// 由节点 + 组件种类构造监控组件规格
    fn monitor_component(
        &self,
        node: &crate::entities::node::Model,
        kind: monitor::ComponentKind,
    ) -> monitor::MonitorComponent {
        monitor::component_for(node, kind, &self.deps.monitor)
    }

    /// 准备：初始化监控记录（置 deploying、清步骤）
    #[activity]
    pub async fn monitor_prepare(
        self: Arc<Self>,
        ctx: ActivityContext,
        req: NodeMonitorReq,
    ) -> Result<(), ActivityError> {
        let node = nodes::get_node(&self.deps.db, &req.node_id).await.map_err(map_err)?;
        let kind = Self::component_kind(&req);
        let comp = self.monitor_component(&node, kind);
        let svc = monitor::service_spec(&node, &comp);
        let workflow_id = ctx
            .info()
            .workflow_execution
            .as_ref()
            .map(|e| e.workflow_id.clone())
            .unwrap_or_default();
        let outcome = monitor::store::begin(
            &self.deps.db,
            &node.id,
            kind.as_str(),
            &workflow_id,
            &comp.image,
            &comp.category,
            &svc.id,
            &svc.name,
        )
        .await;
        match &outcome {
            Ok(()) => {
                self.monitor_record(&node.id, kind.as_str(), "prepare", "准备", &Ok("已初始化监控记录".into()))
                    .await
            }
            Err(e) => self.monitor_record(&node.id, kind.as_str(), "prepare", "准备", &Err(ModelinferError::new(e.code, e.msg.clone()))).await,
        }
        outcome.map_err(map_err)?;
        Ok(())
    }

    /// 镜像就绪：节点本地 → image-hub 分发（绝不 docker pull）
    #[activity]
    pub async fn monitor_ensure_image(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: NodeMonitorReq,
    ) -> Result<(), ActivityError> {
        let node = nodes::get_node(&self.deps.db, &req.node_id).await.map_err(map_err)?;
        let kind = Self::component_kind(&req);
        let target = nodes::ssh_target_of(&node);
        let comp = self.monitor_component(&node, kind);
        let candidates = self
            .deps
            .platform
            .device_info()
            .await
            .map(|d| d.ip_addr)
            .unwrap_or_default();
        let hub = RealImageHubClient::from_config(&self.deps.image_hub);
        let workdir = monitor::monitor_workdir(&node.id);
        let res = monitor::ensure_component_image(
            self.deps.ssh.as_ref(),
            &hub,
            &RealSourceIpResolver,
            &self.deps.image_hub,
            &candidates,
            &target,
            &workdir,
            &comp.image,
        )
        .await
        .map(|pulled| if pulled { format!("已从 image-hub 拉取 {}", comp.image) } else { format!("节点本地已具备 {}", comp.image) })
        .inspect_err(|e| {
            tracing::warn!(target: "modelinfer_monitor", code = e.code, error = %e.msg, "监控镜像就绪失败");
        });
        self.monitor_record(&node.id, kind.as_str(), "ensure-image", "镜像就绪", &res).await;
        res.map(|_| ()).map_err(act_err)
    }

    /// 启动监控组件容器并校验 running（GPU 组件先探测容器运行时）
    #[activity]
    pub async fn monitor_deploy(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: NodeMonitorReq,
    ) -> Result<(), ActivityError> {
        let node = nodes::get_node(&self.deps.db, &req.node_id).await.map_err(map_err)?;
        let kind = Self::component_kind(&req);
        let target = nodes::ssh_target_of(&node);
        let comp = self.monitor_component(&node, kind);
        let gpu_flag = if comp.gpu {
            monitor::detect_gpu_flag(self.deps.ssh.as_ref(), &target).await
        } else {
            String::new()
        };
        let res = monitor::deploy_component(self.deps.ssh.as_ref(), &target, &comp, &gpu_flag)
            .await
            .map(|()| format!("容器 {} 运行中（端口 {}）", comp.container, comp.port))
            .inspect_err(|e| {
                tracing::warn!(target: "modelinfer_monitor", code = e.code, error = %e.msg, "监控组件启动失败");
            });
        self.monitor_record(&node.id, kind.as_str(), "deploy", "启动容器", &res).await;
        res.map(|_| ()).map_err(act_err)
    }

    /// 注册 consul 服务
    #[activity]
    pub async fn monitor_register_consul(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: NodeMonitorReq,
    ) -> Result<(), ActivityError> {
        let node = nodes::get_node(&self.deps.db, &req.node_id).await.map_err(map_err)?;
        let kind = Self::component_kind(&req);
        let comp = self.monitor_component(&node, kind);
        let svc = monitor::service_spec(&node, &comp);
        let client = monitor::consul::RealConsulClient::new(self.deps.monitor.consul());
        let res = monitor::consul::ConsulClient::register(&client, &svc)
            .await
            .map(|()| format!("已注册 {}（{}:{}）", svc.id, svc.address, svc.port))
            .inspect_err(|e| {
                tracing::warn!(target: "modelinfer_monitor", code = e.code, error = %e.msg, "consul 注册失败");
            });
        self.monitor_record(&node.id, kind.as_str(), "register-consul", "注册 consul", &res).await;
        res.map(|_| ()).map_err(act_err)
    }

    /// 校验监控组件可用（端口可达 + 指标内容可用）
    #[activity]
    pub async fn monitor_verify(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: NodeMonitorReq,
    ) -> Result<(), ActivityError> {
        let node = nodes::get_node(&self.deps.db, &req.node_id).await.map_err(map_err)?;
        let kind = Self::component_kind(&req);
        let target = nodes::ssh_target_of(&node);
        let comp = self.monitor_component(&node, kind);
        let res = monitor::verify_component(self.deps.ssh.as_ref(), &target, &comp)
            .await
            .map(|()| format!("端口 {} 可达且指标内容可用", comp.port));
        self.monitor_record(&node.id, kind.as_str(), "verify", "校验指标", &res).await;
        res.map(|_| ()).map_err(act_err)
    }

    /// 收尾：置 deployed
    #[activity]
    pub async fn monitor_finish(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: NodeMonitorReq,
    ) -> Result<(), ActivityError> {
        let kind = Self::component_kind(&req);
        monitor::store::finish(&self.deps.db, &req.node_id, kind.as_str(), monitor::store::state::DEPLOYED, "")
            .await
            .map_err(map_err)?;
        Ok(())
    }

    /// 失败：置 failed 并记录错误与步骤
    #[activity]
    pub async fn monitor_fail(
        self: Arc<Self>,
        _ctx: ActivityContext,
        req: MonitorFailReq,
    ) -> Result<(), ActivityError> {
        let kind = monitor::ComponentKind::parse(&req.component).unwrap_or(monitor::ComponentKind::NodeExporter);
        self.monitor_record(
            &req.node_id,
            kind.as_str(),
            &req.stage,
            &req.stage,
            &Err(ModelinferError::new(codes::MONITOR_DEPLOY_FAILED, req.error.clone())),
        )
        .await;
        monitor::store::finish(&self.deps.db, &req.node_id, kind.as_str(), monitor::store::state::FAILED, &req.error)
            .await
            .map_err(map_err)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::MockPlatformClient;

    fn node() -> DeployNode {
        DeployNode {
            id: "n1".into(),
            name: "n1".into(),
            host: "10.0.0.1".into(),
            port: 22,
            user: "root".into(),
            auth_method: "password".into(),
            password: "x".into(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            hostname: "n1".into(),
            socket_ifname: "eth0".into(),
            ib_hca: String::new(),
            arch: "amd64".into(),
            fscache: false,
            role: "head".into(),
        }
    }

    async fn deps_with(ssh: Arc<dyn crate::ssh::SshExec>) -> DeployDeps {
        let db = crate::db::connect("sqlite::memory:").await.expect("内存库");
        DeployDeps {
            db,
            ssh,
            platform: Arc::new(MockPlatformClient::new()),
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

    #[test]
    fn local_deploy_node_target_uses_nsenter() {
        let mut n = node();
        n.auth_method = "local".into();
        n.password.clear();
        let target = target_of(&n);
        assert!(target.is_local());
        assert_eq!(target.auth.kind(), "local");
    }

    /// `run_on_node` MUST 把调用方给定的超时透传到 SSH 执行器（长启动阶段不再被 90s 强杀）
    #[tokio::test]
    async fn run_on_node_passes_timeout_to_ssh() {
        let ssh = Arc::new(crate::ssh::MockSshExec::new().with_rule("stage_start", 0, "ok\n"));
        let deps = deps_with(ssh.clone()).await;
        run_on_node(
            &deps,
            &node(),
            "/w/.modelinfer/stage_start.sh",
            Duration::from_secs(900),
        )
        .await
        .unwrap();
        assert_eq!(
            ssh.recorded_timeouts(),
            vec![Duration::from_secs(900)],
            "阶段超时应透传到 SSH"
        );
    }

    #[tokio::test]
    async fn distribute_model_uploads_file_overrides_after_package() {
        use sea_orm::{ActiveModelTrait, Set};
        let ssh = Arc::new(crate::ssh::MockSshExec::new());
        let mut deps = deps_with(ssh.clone()).await;
        let tmp = tempfile::tempdir().unwrap();
        deps.model_hub_path = tmp.path().display().to_string();
        let card_dir = tmp.path().join("card");
        let src_dir = card_dir.join("src");
        std::fs::create_dir_all(src_dir.join("scripts")).unwrap();
        std::fs::write(src_dir.join("manifest.yaml"), "id: c\nversion: '1'\n").unwrap();
        std::fs::write(src_dir.join("scripts/start.sh"), "echo original\n").unwrap();
        std::fs::write(card_dir.join("model.tar.gz"), b"mock").unwrap();

        let now = crate::time::now_iso();
        crate::entities::node::ActiveModel {
            id: Set("n1".into()),
            name: Set("local-n1".into()),
            host: Set("127.0.0.1".into()),
            access_mode: Set("local".into()),
            auth_method: Set("local".into()),
            created_at: Set(now.clone()),
            updated_at: Set(now.clone()),
            ..Default::default()
        }
        .insert(&deps.db)
        .await
        .unwrap();
        crate::entities::plaza_model::ActiveModel {
            id: Set("c@1".into()),
            model_id: Set("c".into()),
            model: Set(String::new()),
            name: Set("c".into()),
            version: Set("1".into()),
            source: Set("local".into()),
            source_id: Set(String::new()),
            path: Set(src_dir.display().to_string()),
            package_path: Set(card_dir.join("model.tar.gz").display().to_string()),
            package_sha256: Set(String::new()),
            manifest_json: Set("{}".into()),
            state: Set("available".into()),
            size_bytes: Set(4),
            created_at: Set(now.clone()),
            updated_at: Set(now.clone()),
        }
        .insert(&deps.db)
        .await
        .unwrap();
        crate::entities::deployment::ActiveModel {
            id: Set("d1".into()),
            name: Set("d1".into()),
            plaza_model_id: Set("c@1".into()),
            card_id: Set("c".into()),
            node_ids_json: Set("[\"n1\"]".into()),
            head_node_id: Set("n1".into()),
            serve_port: Set(8000),
            created_at: Set(now.clone()),
            updated_at: Set(now.clone()),
            ..Default::default()
        }
        .insert(&deps.db)
        .await
        .unwrap();
        crate::entities::deployment_file_override::ActiveModel {
            id: Set("o1".into()),
            deployment_id: Set("d1".into()),
            path: Set("scripts/start.sh".into()),
            content: Set("echo override\n".into()),
            size_bytes: Set(14),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(&deps.db)
        .await
        .unwrap();

        let acts = DeployActivities::new(deps);
        let out = acts
            .distribute_model_impl(&DeploymentIdReq {
                deployment_id: "d1".into(),
                run_id: String::new(),
            })
            .await
            .unwrap();
        assert_eq!(out["file_overrides"], 1);
        let remote = "/opt/modelinfer/deployments/d1/scripts/start.sh";
        assert!(
            ssh.recorded_uploads().iter().any(|u| u.ends_with(remote)),
            "uploads={:?}",
            ssh.recorded_uploads()
        );
        assert!(
            ssh.recorded_calls()
                .iter()
                .any(|c| c.contains(&format!("chmod 0755 -- '{}'", remote))),
            "calls={:?}",
            ssh.recorded_calls()
        );
    }

    /// 播种一个节点 + 运行配置（可选 start 阶段全局对象）
    async fn seed_node_deployment(db: &sea_orm::DatabaseConnection, engine_global: Option<&str>) {
        use sea_orm::{ActiveModelTrait, Set};
        let now = crate::time::now_iso();
        crate::entities::node::ActiveModel {
            id: Set("n1".into()),
            name: Set("n1".into()),
            host: Set("10.0.0.1".into()),
            created_at: Set(now.clone()),
            updated_at: Set(now.clone()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
        crate::entities::deployment::ActiveModel {
            id: Set("d1".into()),
            name: Set("d1".into()),
            plaza_model_id: Set("c@1".into()),
            head_node_id: Set("n1".into()),
            serve_port: Set(8000),
            created_at: Set(now.clone()),
            updated_at: Set(now.clone()),
            ..Default::default()
        }
        .insert(db)
        .await
        .unwrap();
        if let Some(g) = engine_global {
            crate::entities::run_global_data::ActiveModel {
                id: Set("g1".into()),
                run_id: Set("r1".into()),
                deployment_id: Set("d1".into()),
                kind: Set("start".into()),
                stage: Set("start".into()),
                global_json: Set(g.into()),
                created_at: Set(now),
                ..Default::default()
            }
            .insert(db)
            .await
            .unwrap();
        }
    }

    #[tokio::test]
    async fn detect_engine_from_script_global_skips_probe() {
        let ssh = Arc::new(crate::ssh::MockSshExec::new());
        let deps = deps_with(ssh.clone()).await;
        seed_node_deployment(&deps.db, Some(r#"{"inference_engine": {"type": "vllm", "version": "0.9.3"}}"#)).await;
        let acts = DeployActivities::new(deps);
        let out = acts
            .detect_engine_impl(&DeploymentIdReq { deployment_id: "d1".into(), run_id: "r1".into() })
            .await
            .unwrap();
        assert_eq!(out.engine, "vllm");
        assert_eq!(out.version, "0.9.3");
        assert_eq!(out.source, "script");
        // 脚本命中时不应发起端口探测
        assert!(ssh.recorded_calls().iter().all(|c| !c.contains("/version")));
    }

    #[tokio::test]
    async fn detect_engine_by_probe_when_script_absent() {
        let ssh = Arc::new(crate::ssh::MockSshExec::new().with_rule(
            "/version",
            0,
            "{\"version\":\"0.9.3\"}\n---MODELINFER-METRICS---\nvllm:num_requests_running 0.0\n",
        ));
        let deps = deps_with(ssh.clone()).await;
        seed_node_deployment(&deps.db, None).await;
        let acts = DeployActivities::new(deps);
        let out = acts
            .detect_engine_impl(&DeploymentIdReq { deployment_id: "d1".into(), run_id: "r1".into() })
            .await
            .unwrap();
        assert_eq!(out.engine, "vllm");
        assert_eq!(out.source, "probe");
        assert!(ssh.recorded_calls().iter().any(|c| c.contains("127.0.0.1:8000/version")));
    }

    #[tokio::test]
    async fn detect_engine_from_manifest_declaration_skips_probe() {
        use sea_orm::{ActiveModelTrait, Set};
        let ssh = Arc::new(crate::ssh::MockSshExec::new());
        let deps = deps_with(ssh.clone()).await;
        seed_node_deployment(&deps.db, None).await;
        // 卡片声明 comfyui 引擎（无权重）
        let manifest = serde_json::json!({
            "id": "c", "version": "1", "inference-engine": "comfyui",
        });
        let now = crate::time::now_iso();
        crate::entities::plaza_model::ActiveModel {
            id: Set("c@1".into()),
            model_id: Set("c".into()),
            model: Set(String::new()),
            name: Set("c".into()),
            version: Set("1".into()),
            source: Set("local".into()),
            source_id: Set(String::new()),
            path: Set("/tmp/c".into()),
            package_path: Set("/tmp/c/model.tar.gz".into()),
            package_sha256: Set(String::new()),
            manifest_json: Set(serde_json::to_string(&manifest).unwrap()),
            state: Set("available".into()),
            size_bytes: Set(0),
            created_at: Set(now.clone()),
            updated_at: Set(now),
        }
        .insert(&deps.db)
        .await
        .unwrap();

        let acts = DeployActivities::new(deps);
        let out = acts
            .detect_engine_impl(&DeploymentIdReq { deployment_id: "d1".into(), run_id: String::new() })
            .await
            .unwrap();
        assert_eq!(out.engine, "comfyui");
        assert_eq!(out.source, "manifest");
        // manifest 命中时不应发起端口探测
        assert!(ssh.recorded_calls().iter().all(|c| !c.contains("/version")));
    }

    #[tokio::test]
    async fn detect_engine_failure_falls_back_to_unknown() {
        let ssh = Arc::new(crate::ssh::MockSshExec::new()); // 无规则 → 空输出
        let deps = deps_with(ssh).await;
        seed_node_deployment(&deps.db, None).await;
        let acts = DeployActivities::new(deps);
        let out = acts
            .detect_engine_impl(&DeploymentIdReq { deployment_id: "d1".into(), run_id: String::new() })
            .await
            .unwrap();
        assert_eq!(out.engine, "unknown");
        assert_eq!(out.source, "unknown");
    }

    #[tokio::test]
    async fn register_metrics_marks_unsupported_for_unknown_engine() {
        let ssh = Arc::new(crate::ssh::MockSshExec::new());
        let deps = deps_with(ssh).await;
        seed_node_deployment(&deps.db, None).await; // inference_engine 为空
        let acts = DeployActivities::new(deps);
        let out = acts
            .register_metrics_impl(&DeploymentIdReq { deployment_id: "d1".into(), run_id: String::new() })
            .await
            .unwrap();
        assert_eq!(out.state, "unsupported");
        let d = crate::deployments::get_deployment(&acts.deps.db, "d1").await.unwrap();
        assert_eq!(d.metrics_state, "unsupported");
    }
}
