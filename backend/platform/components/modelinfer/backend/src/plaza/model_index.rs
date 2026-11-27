//! 模型广场 index.yml / manifest.yaml 解析（纯函数，便于单测）
//!
//! 协议见 `specs/docs/design/modelinfer/model-plaza-server.md`。

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{ModelinferError, codes};

/// index.yml 顶层结构
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ModelIndex {
    #[serde(default, rename = "apiVersion")]
    pub api_version: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub models: Vec<ModelIndexEntry>,
}

/// index.yml 中的一个模型卡片条目
///
/// 条目字段一律 kebab-case；展示元数据（`name` / `description` / `category` / `tags`）由卡片
/// `manifest.yaml` 顶层派生，`name` 直接沿用 manifest 的多语言值。
#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ModelIndexEntry {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub name: Option<Value>,
    #[serde(default)]
    pub description: Option<Value>,
    #[serde(default)]
    pub category: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub icon: String,
    /// 权重 id（manifest `model`；旧索引可缺省）
    #[serde(default)]
    pub model: String,
    /// 镜像声明（manifest `images`；旧索引可缺省）
    #[serde(default)]
    pub images: Vec<Value>,
    #[serde(default)]
    pub manifest: String,
    #[serde(default)]
    pub package: String,
    #[serde(default, rename = "size-bytes")]
    pub size_bytes: Option<i64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub assets: Vec<Value>,
}

/// manifest.yaml 解析出的关键字段（部署与校验用）
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ManifestMeta {
    pub id: String,
    #[serde(default)]
    pub name: Option<Value>,
    #[serde(default)]
    pub description: Option<Value>,
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub model: String,
    /// 推理引擎类型（顶层 kebab `inference-engine`；缺省 / 空 = 未声明）
    #[serde(default, rename = "inference-engine")]
    pub inference_engine: String,
    #[serde(default)]
    pub images: Vec<Value>,
    #[serde(default)]
    pub configs: Vec<Value>,
    #[serde(default)]
    pub lifecycle: serde_json::Map<String, Value>,
    /// 各生命周期阶段的节点执行顺序（阶段 → `head-first` / `worker-first` /
    /// `worker-first-reverse`；缺省 `head-first`）
    #[serde(default, rename = "lifecycle-order")]
    pub lifecycle_order: serde_json::Map<String, Value>,
    /// 各生命周期阶段的执行超时（阶段 → 秒；缺省 1800，上限 86400）
    #[serde(default, rename = "lifecycle-timeout")]
    pub lifecycle_timeout: serde_json::Map<String, Value>,
    /// manifest 原文（归一化为 JSON 值）
    #[serde(skip)]
    pub raw: Value,
}

/// 解析 index.yml
pub fn parse_index(yaml: &str) -> Result<ModelIndex, ModelinferError> {
    let idx: ModelIndex = serde_yaml::from_str(yaml).map_err(|e| {
        ModelinferError::new(codes::PLAZA_FETCH_FAILED, format!("解析 index.yml 失败: {e}"))
    })?;
    if !idx.kind.is_empty() && idx.kind != "ModelIndex" {
        return Err(ModelinferError::new(
            codes::PLAZA_FETCH_FAILED,
            format!("index.yml kind 非法: {}", idx.kind),
        ));
    }
    Ok(idx)
}

/// 解析 manifest.yaml（校验必填 id / version / model）
pub fn parse_manifest(yaml: &str) -> Result<ManifestMeta, ModelinferError> {
    let yaml_value: serde_yaml::Value = serde_yaml::from_str(yaml).map_err(|e| {
        ModelinferError::new(codes::MANIFEST_INVALID, format!("解析 manifest 失败: {e}"))
    })?;
    let meta: ManifestMeta = serde_yaml::from_value(yaml_value.clone()).map_err(|e| {
        ModelinferError::new(codes::MANIFEST_INVALID, format!("manifest 字段类型错误: {e}"))
    })?;
    if meta.id.trim().is_empty() {
        return Err(ModelinferError::new(codes::MANIFEST_INVALID, "manifest 缺 id"));
    }
    if meta.version.trim().is_empty() {
        return Err(ModelinferError::new(codes::MANIFEST_INVALID, "manifest 缺 version"));
    }
    // `model`（权重 id）自 ComfyUI 支持起为**可选**：缺省 / 空表示无权重卡片（按需加载权重）。
    let mut meta = meta;
    meta.raw = serde_json::to_value(&yaml_value).unwrap_or(serde_json::Value::Null);
    Ok(meta)
}

/// 拼接 URL：`join_url("http://h:1/store", "meta/x/manifest.yaml")` → `http://h:1/store/meta/x/manifest.yaml`
pub fn join_url(base: &str, rel: &str) -> String {
    let base = base.trim_end_matches('/');
    let rel = rel.trim_start_matches('/');
    if rel.is_empty() {
        base.to_string()
    } else {
        format!("{base}/{rel}")
    }
}

/// 归一化来源地址：去空白与尾斜杠
pub fn normalize_base_url(raw: &str) -> String {
    raw.trim().trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    const INDEX: &str = r#"
apiVersion: vos.ictrek.com/v1alpha1
kind: ModelIndex
models:
  - id: com.ictrek.modelinfer.test-mock
    version: 1.0.0
    name: { zh-CN: 模拟模型, en: Mock Model }
    description: { zh-CN: 测试 }
    category: [test]
    tags: [mock]
    icon: meta/com.ictrek.modelinfer.test-mock/1.0.0/icon.png
    model: Qwen/Qwen3-0.6B
    images:
      - registry.ictrek.internal/library/ubuntu:22.04
    manifest: meta/com.ictrek.modelinfer.test-mock/1.0.0/manifest.yaml
    package: models/com.ictrek.modelinfer.test-mock/1.0.0/model.tar.gz
    size-bytes: 1024
    sha256: abc123
"#;

    const MANIFEST: &str = r#"
id: com.ictrek.modelinfer.test-mock
name: { zh-CN: 模拟模型 }
description: { zh-CN: 测试 }
version: 1.0.0
author: ictrek
category: [test]
tags: [mock]
model: Qwen/Qwen3-0.6B
images:
  - registry.ictrek.internal/library/ubuntu:22.04
configs:
  - { name: MOCK_SERVE_DELAY, type: integer, default: 0 }
lifecycle:
  start: ./scripts/start.sh
  health-probe: ./scripts/health.sh
"#;

    #[test]
    fn parse_index_ok() {
        let idx = parse_index(INDEX).unwrap();
        assert_eq!(idx.kind, "ModelIndex");
        assert_eq!(idx.models.len(), 1);
        let e = &idx.models[0];
        assert_eq!(e.version, "1.0.0");
        assert_eq!(e.size_bytes, Some(1024));
        assert_eq!(e.category, vec!["test".to_string()]);
        assert_eq!(e.tags, vec!["mock".to_string()]);
        assert_eq!(e.model, "Qwen/Qwen3-0.6B");
        assert_eq!(e.images.len(), 1);
        // name 为 manifest 的多语言对象
        assert_eq!(
            e.name.as_ref().and_then(|v| v.get("zh-CN")).and_then(Value::as_str),
            Some("模拟模型")
        );
    }

    #[test]
    fn parse_index_bad_kind() {
        let bad = "kind: Other\nmodels: []\n";
        assert_eq!(parse_index(bad).unwrap_err().code, codes::PLAZA_FETCH_FAILED);
    }

    #[test]
    fn parse_manifest_ok_and_required() {
        let m = parse_manifest(MANIFEST).unwrap();
        assert_eq!(m.model, "Qwen/Qwen3-0.6B");
        assert_eq!(m.configs.len(), 1);
        assert!(m.lifecycle.contains_key("start"));

        let missing = "id: x\nversion: 1.0.0\n";
        // `model` 可选：缺省合法；仅缺 id / version 才非法
        assert!(parse_manifest(missing).is_ok());
        let missing_id = "version: 1.0.0\n";
        assert_eq!(parse_manifest(missing_id).unwrap_err().code, codes::MANIFEST_INVALID);
    }

    #[test]
    fn parse_manifest_inference_engine_and_optional_model() {
        let yaml = r#"
id: com.ictrek.comfyui-gb10
version: 1.0.0
inference-engine: comfyui
images:
  - nvidia/cuda:13.1.1-devel-ubuntu24.04
lifecycle:
  start: ./scripts/start.sh
"#;
        let m = parse_manifest(yaml).unwrap();
        assert_eq!(m.model, "");
        assert_eq!(m.inference_engine, "comfyui");

        // 未声明引擎 → 空串
        let m2 = parse_manifest("id: x\nversion: 1.0.0\nmodel: org/w\n").unwrap();
        assert_eq!(m2.inference_engine, "");
    }

    #[test]
    fn lifecycle_order_and_timeout_parsed() {
        let yaml = r#"
id: x
version: 1.0.0
model: org/weight
lifecycle-order:
  start: worker-first-reverse
lifecycle-timeout:
  start: 3600
lifecycle:
  start: ./scripts/start.sh
"#;
        let m = parse_manifest(yaml).unwrap();
        assert_eq!(
            m.lifecycle_order.get("start").and_then(Value::as_str),
            Some("worker-first-reverse")
        );
        assert_eq!(m.lifecycle_timeout.get("start").and_then(Value::as_u64), Some(3600));
    }

    #[test]
    fn legacy_camel_case_keys_are_ignored() {
        let yaml = r#"
id: x
version: 1.0.0
model: org/weight
lifecycleOrder:
  start: worker-first-reverse
lifecycleTimeout:
  start: 3600
"#;
        let m = parse_manifest(yaml).unwrap();
        assert!(m.lifecycle_order.is_empty());
        assert!(m.lifecycle_timeout.is_empty());
    }

    #[test]
    fn join_and_normalize_url() {
        assert_eq!(join_url("http://h:1/store/", "/meta/x"), "http://h:1/store/meta/x");
        assert_eq!(normalize_base_url(" http://h:1/store/ "), "http://h:1/store");
    }
}