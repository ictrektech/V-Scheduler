//! 手动启动的只读资源检查：在修改运行态或提交工作流前返回完整下载清单。

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::deploy::DeployNode;
use crate::entities::{deployment, node};
use crate::error::{ModelinferError, codes};
use crate::images::{self, ImageHubClient};
use crate::{model_hub, ssh::SshExec};

#[derive(Debug, Serialize)]
pub struct MissingWeight {
    pub source: String,
    pub model_id: String,
    pub overridden: bool,
}

#[derive(Debug, Serialize)]
pub struct MissingStartResources {
    pub weights: Vec<MissingWeight>,
    pub images: Vec<String>,
    pub images_source: String,
}

fn missing_weights(hub_dir: &Path, d: &deployment::Model) -> Vec<MissingWeight> {
    let (model_id, source) = super::validate::effective_weight(
        &d.weight_id,
        &d.hub_source,
        Some((&d.weight_override_source, &d.weight_override_id)),
    );
    if model_id.is_empty() || model_hub::variant_root(hub_dir, &source, &model_id).is_some() {
        return Vec::new();
    }
    vec![MissingWeight {
        source,
        model_id,
        overridden: !d.weight_override_id.trim().is_empty(),
    }]
}

pub async fn check(
    d: &deployment::Model,
    manifest: &Value,
    nodes: &[node::Model],
    hub_dir: &Path,
    ssh: &dyn SshExec,
    hub: &dyn ImageHubClient,
    concurrency: usize,
) -> Result<(), ModelinferError> {
    let weights = missing_weights(hub_dir, d);
    let overrides: Vec<Value> = serde_json::from_str(&d.images_override_json).unwrap_or_default();
    let effective_images = if overrides.is_empty() {
        images::manifest_images(manifest)?
    } else {
        images::effective_images(&[], &overrides)
    };
    let deploy_nodes: Vec<DeployNode> = nodes
        .iter()
        .enumerate()
        .map(|(idx, n)| DeployNode::from_model(n, if idx == 0 { "head" } else { "worker" }))
        .collect();
    let node_refs: Vec<&DeployNode> = deploy_nodes.iter().collect();
    let missing = MissingStartResources {
        weights,
        images: images::unavailable_images(ssh, hub, &node_refs, &effective_images, concurrency)
            .await?,
        images_source: images::images_source(&overrides).into(),
    };
    if missing.weights.is_empty() && missing.images.is_empty() {
        return Ok(());
    }
    Err(ModelinferError::new(
        codes::START_RESOURCES_MISSING,
        "启动所需资源缺失，请到 V-Scheduler 下载指定来源的权重，到 image-hub 下载指定镜像后重试",
    )
    .with_data(serde_json::json!(missing)))
}
