//! 部署配置校验（纯函数，便于单测）

use serde_json::{Map, Value};

use crate::error::{ModelinferError, codes};

/// 节点简要信息（校验用）
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeBrief {
    pub id: String,
    pub name: String,
    pub host: String,
    pub hostname: String,
}

/// 部署配置是否可编辑（"未启动"语义 = 非活跃）
///
/// - 允许：`stopped`（启动前 / 停止后）、`failed`（启动失败，`desired_state=stopped`）；
/// - 拒绝：`starting` / `running` / `stopping` / `degraded`，以及 `desired_state == "stopping"`
///   的过渡窗口（此时停止清理尚未完成）。
pub fn is_editable(state: &str, desired_state: &str) -> bool {
    matches!(state, "stopped" | "failed") && desired_state != "stopping"
}

/// 所选节点 hostname MUST 互不相同（空 hostname 不参与判重）
pub fn check_unique_hostnames(nodes: &[NodeBrief]) -> Result<(), ModelinferError> {
    let mut seen: Vec<&str> = Vec::new();
    for n in nodes {
        let h = n.hostname.trim();
        if h.is_empty() {
            continue;
        }
        if seen.contains(&h) {
            return Err(ModelinferError::new(
                codes::DEPLOYMENT_CONFLICT,
                format!("所选节点主机名重复: {h}（请为节点设置不同的 hostname）"),
            ));
        }
        seen.push(h);
    }
    Ok(())
}

/// 合法生命周期顺序
pub const LIFECYCLE_ORDERS: [&str; 3] = ["head-first", "worker-first", "worker-first-reverse"];

/// 校验 manifest `lifecycle-order` 取值（创建 / 更新部署配置时调用）
pub fn check_lifecycle_order(manifest: &Value) -> Result<(), ModelinferError> {
    let Some(map) = manifest.get("lifecycle-order").and_then(Value::as_object) else {
        return Ok(());
    };
    for (stage, v) in map {
        let Some(s) = v.as_str() else {
            return Err(ModelinferError::new(
                codes::LIFECYCLE_ORDER_INVALID,
                format!("lifecycle-order.{stage} 必须是字符串"),
            ));
        };
        if !LIFECYCLE_ORDERS.contains(&s) {
            return Err(ModelinferError::new(
                codes::LIFECYCLE_ORDER_INVALID,
                format!("lifecycle-order.{stage} 取值非法: {s}"),
            ));
        }
    }
    Ok(())
}

/// 从 manifest `configs` 数组提取（保留原始 Value）
pub fn config_schema(manifest: &Value) -> Vec<Value> {
    manifest
        .get("configs")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

/// 读取 manifest 顶层 `caps.max-nodes`（模型卡片单次部署最大节点数）
///
/// 语义（见 `specs/docs/design/modelinfer/model-package.md` §2.7）：
/// - 无 `caps` / 无 `caps.max-nodes` / 值为 `null` → `0`（**不限制**）；
/// - 非负整数（含整值浮点，如 `1.0`）→ 该值；
/// - 负数 / 非数值 / 非整值浮点 / `caps` 非对象 → `20715`（**不静默放行**）；
/// - `caps` 内其余键（含平台 caps 的 `hardware` / `software` / metadata 键）一律忽略，
///   为后续扩展保留前向兼容。
pub fn caps_max_nodes(manifest: &Value) -> Result<u64, ModelinferError> {
    let Some(caps) = manifest.get("caps") else {
        return Ok(0);
    };
    if caps.is_null() {
        return Ok(0);
    }
    let Some(obj) = caps.as_object() else {
        return Err(ModelinferError::new(
            codes::MANIFEST_INVALID,
            "caps 必须是对象",
        ));
    };
    let Some(v) = obj.get("max-nodes").filter(|v| !v.is_null()) else {
        return Ok(0);
    };
    if let Some(i) = v.as_i64() {
        if i >= 0 {
            return Ok(i as u64);
        }
    } else if let Some(f) = v.as_f64()
        && f.fract() == 0.0
        && (0.0..=(u64::MAX as f64)).contains(&f)
    {
        return Ok(f as u64);
    }
    Err(ModelinferError::new(
        codes::MANIFEST_INVALID,
        format!("caps.max-nodes 必须是非负整数: {v}"),
    ))
}

/// 校验所选节点数满足 manifest `caps.max-nodes`（`0` = 不限制）
pub fn check_caps_node_limit(manifest: &Value, node_count: usize) -> Result<(), ModelinferError> {
    let max = caps_max_nodes(manifest)?;
    if max > 0 && node_count as u64 > max {
        return Err(ModelinferError::new(
            codes::CAPS_LIMIT_EXCEEDED,
            format!(
                "所选节点数 {node_count} 超出模型卡片 caps.max-nodes 声明的上限 {max}（该卡片最多支持 {max} 个节点）"
            ),
        ));
    }
    Ok(())
}

/// 依据卡片 `configs` 校验用户参数：类型检查 + 缺省填默认值 + 剔除未声明键
pub fn validate_config_values(
    schema: &[Value],
    values: &Map<String, Value>,
) -> Result<Map<String, Value>, ModelinferError> {
    let mut out = Map::new();
    for spec in schema {
        let Some(name) = spec.get("name").and_then(Value::as_str) else {
            continue;
        };
        let ty = spec.get("type").and_then(Value::as_str).unwrap_or("string");
        let provided = values.get(name).filter(|v| !v.is_null());
        let value = match provided {
            Some(v) => {
                check_type(name, ty, v)?;
                v.clone()
            }
            None => match spec.get("default") {
                Some(d) if !d.is_null() => d.clone(),
                _ => {
                    return Err(ModelinferError::new(
                        codes::PARAM_INVALID,
                        format!("缺少必填配置项: {name}"),
                    ));
                }
            },
        };
        // 枚举约束
        if let Some(enum_vals) = spec.get("enum").and_then(Value::as_array)
            && !enum_vals.contains(&value)
        {
            return Err(ModelinferError::new(
                codes::PARAM_INVALID,
                format!("配置项 {name} 取值不在允许范围"),
            ));
        }
        out.insert(name.to_string(), value);
    }
    Ok(out)
}

fn check_type(name: &str, ty: &str, v: &Value) -> Result<(), ModelinferError> {
    let ok = match ty {
        "integer" => v.is_i64() || v.is_u64() || (v.is_f64() && v.as_f64().is_some_and(|f| f.fract() == 0.0)),
        "number" => v.is_number(),
        "boolean" => v.is_boolean(),
        "string" => v.is_string(),
        _ => true,
    };
    if ok {
        Ok(())
    } else {
        Err(ModelinferError::new(
            codes::PARAM_INVALID,
            format!("配置项 {name} 类型应为 {ty}"),
        ))
    }
}

// ============ 运行配置覆盖（权重 / 镜像 / 额外环境变量） ============
//
// 设计见 `specs/docs/design/modelinfer/runtime-overrides.md`：
// - 权重覆盖 `(source, model_id)`，创建 / 更新时校验 model-hub 存在；
// - 镜像覆盖与 manifest `images` 同结构，非空**整体替换**；
// - 额外环境变量在卡片 `configs` 之后注入，禁止占用平台保留前缀与关键系统变量。

/// 镜像覆盖最大条目数（防御过大配置）
pub const MAX_IMAGES_OVERRIDE: usize = 32;
/// 额外环境变量最大条目数
pub const MAX_EXTRA_ENV: usize = 64;
/// 额外环境变量键名最大长度
pub const MAX_ENV_KEY_LEN: usize = 128;
/// 额外环境变量值最大长度（字符）
pub const MAX_ENV_VALUE_LEN: usize = 8192;
/// 权重 id 最大长度
pub const MAX_WEIGHT_ID_LEN: usize = 256;

/// 平台保留的环境变量前缀（禁止被 `extra_env` 占用）
pub const RESERVED_ENV_PREFIX: &str = "MODELINFER_";
/// 禁止被 `extra_env` 覆盖的关键系统变量
pub const FORBIDDEN_ENV_KEYS: [&str; 11] = [
    "PATH",
    "HOME",
    "SHELL",
    "IFS",
    "BASH_ENV",
    "ENV",
    "LD_PRELOAD",
    "LD_LIBRARY_PATH",
    "PWD",
    "OLDPWD",
    "TMPDIR",
];

/// 校验并归一权重覆盖，返回 `Some((source, model_id))` 或 `None`（未覆盖）
///
/// - `source` / `model_id` 同时为空（去空白后）→ `None`；
/// - 只提供其中一个 → `20745`（避免歧义状态）；
/// - `source` 仅允许 `hf` / `ms`；`model_id` 须为 `<org>/<repo>` 两段、无空白 / 上跳段。
pub fn validate_weight_override(
    source: &str,
    model_id: &str,
) -> Result<Option<(String, String)>, ModelinferError> {
    let source = source.trim();
    let model_id = model_id.trim();
    if source.is_empty() && model_id.is_empty() {
        return Ok(None);
    }
    if source.is_empty() || model_id.is_empty() {
        return Err(ModelinferError::new(
            codes::WEIGHT_OVERRIDE_INVALID,
            "权重覆盖的 source 与 model_id 必须同时提供或同时清空",
        ));
    }
    if !matches!(source, "hf" | "ms") {
        return Err(ModelinferError::new(
            codes::WEIGHT_OVERRIDE_INVALID,
            format!("权重覆盖来源只能是 hf 或 ms: {source}"),
        ));
    }
    if model_id.len() > MAX_WEIGHT_ID_LEN {
        return Err(ModelinferError::new(
            codes::WEIGHT_OVERRIDE_INVALID,
            format!("权重覆盖 id 过长（最多 {MAX_WEIGHT_ID_LEN} 字符）"),
        ));
    }
    let segs: Vec<&str> = model_id.split('/').collect();
    let valid = segs.len() == 2
        && segs.iter().all(|s| {
            !s.is_empty()
                && *s != "."
                && *s != ".."
                && !s.chars().any(|c| c.is_whitespace() || c.is_control() || c == '\\')
        });
    if !valid {
        return Err(ModelinferError::new(
            codes::WEIGHT_OVERRIDE_INVALID,
            format!("权重覆盖 id 必须是 <org>/<repo> 形式且不含空白: {model_id}"),
        ));
    }
    Ok(Some((source.to_string(), model_id.to_string())))
}

/// 校验镜像覆盖条目（保持用户书写形式）
///
/// 规则与 manifest `images` 一致：成员为字符串或含非空 `name` 的对象；`arch` 若给出须为
/// `amd64` / `arm64`（归一 `aarch64` / `x86_64`）；镜像引用须可归一（与 `image-hub.sh` 同规则）。
pub fn validate_images_override(items: &[Value]) -> Result<Vec<Value>, ModelinferError> {
    if items.is_empty() {
        return Ok(Vec::new());
    }
    if items.len() > MAX_IMAGES_OVERRIDE {
        return Err(ModelinferError::new(
            codes::IMAGE_OVERRIDE_INVALID,
            format!("镜像覆盖最多 {MAX_IMAGES_OVERRIDE} 条（当前 {}）", items.len()),
        ));
    }
    let mut out = Vec::with_capacity(items.len());
    for (idx, item) in items.iter().enumerate() {
        let no = idx + 1;
        match item {
            Value::String(s) => {
                let name = s.trim();
                if name.is_empty() {
                    return Err(invalid_image_override(no, "镜像名为空"));
                }
                crate::images::normalize_image_ref(name).map_err(|e| {
                    ModelinferError::new(
                        codes::IMAGE_OVERRIDE_INVALID,
                        format!("镜像覆盖第 {no} 条非法: {}", e.msg),
                    )
                })?;
                out.push(Value::String(name.to_string()));
            }
            Value::Object(map) => {
                let name = map
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| invalid_image_override(no, "对象成员缺少非空 name"))?;
                crate::images::normalize_image_ref(name).map_err(|e| {
                    ModelinferError::new(
                        codes::IMAGE_OVERRIDE_INVALID,
                        format!("镜像覆盖第 {no} 条非法: {}", e.msg),
                    )
                })?;
                let arch = map.get("arch").and_then(Value::as_str).unwrap_or("").trim();
                let arch = crate::images::normalize_arch(arch);
                if !arch.is_empty() && arch != "amd64" && arch != "arm64" {
                    return Err(invalid_image_override(
                        no,
                        &format!("arch 只能是 amd64 / arm64: {arch}"),
                    ));
                }
                let mut normalized = serde_json::Map::new();
                normalized.insert("name".to_string(), Value::String(name.to_string()));
                if !arch.is_empty() {
                    normalized.insert("arch".to_string(), Value::String(arch));
                }
                out.push(Value::Object(normalized));
            }
            _ => {
                return Err(invalid_image_override(no, "成员必须是字符串或对象"));
            }
        }
    }
    Ok(out)
}

fn invalid_image_override(no: usize, why: &str) -> ModelinferError {
    ModelinferError::new(
        codes::IMAGE_OVERRIDE_INVALID,
        format!("镜像覆盖第 {no} 条非法: {why}"),
    )
}

fn valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// 校验额外环境变量（值统一归一为字符串）
///
/// `card_config_names` 为卡片 `configs` 声明的参数名（同名冲突会被拒绝，避免两处取值语义不清）。
pub fn validate_extra_env(
    values: &Map<String, Value>,
    card_config_names: &[String],
) -> Result<Map<String, Value>, ModelinferError> {
    if values.is_empty() {
        return Ok(Map::new());
    }
    if values.len() > MAX_EXTRA_ENV {
        return Err(ModelinferError::new(
            codes::EXTRA_ENV_INVALID,
            format!("额外环境变量最多 {MAX_EXTRA_ENV} 个（当前 {}）", values.len()),
        ));
    }
    let mut out = Map::new();
    for (raw_key, raw_value) in values {
        let key = raw_key.trim();
        if key.is_empty() {
            return Err(ModelinferError::new(
                codes::EXTRA_ENV_INVALID,
                "额外环境变量名不能为空",
            ));
        }
        if key.len() > MAX_ENV_KEY_LEN || !valid_env_key(key) {
            return Err(ModelinferError::new(
                codes::EXTRA_ENV_INVALID,
                format!("额外环境变量名非法（须匹配 [A-Za-z_][A-Za-z0-9_]*）: {key}"),
            ));
        }
        if key.starts_with(RESERVED_ENV_PREFIX) {
            return Err(ModelinferError::new(
                codes::EXTRA_ENV_INVALID,
                format!("额外环境变量名不能使用平台保留前缀 {RESERVED_ENV_PREFIX}: {key}"),
            ));
        }
        if FORBIDDEN_ENV_KEYS.contains(&key) {
            return Err(ModelinferError::new(
                codes::EXTRA_ENV_INVALID,
                format!("额外环境变量名属于系统关键变量，禁止覆盖: {key}"),
            ));
        }
        if card_config_names.iter().any(|n| n == key) {
            return Err(ModelinferError::new(
                codes::EXTRA_ENV_INVALID,
                format!("额外环境变量与卡片配置项同名，请改用卡片参数设置: {key}"),
            ));
        }
        let text = match raw_value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            Value::Null => String::new(),
            _ => {
                return Err(ModelinferError::new(
                    codes::EXTRA_ENV_INVALID,
                    format!("额外环境变量值只支持字符串 / 数字 / 布尔: {key}"),
                ));
            }
        };
        if text.chars().count() > MAX_ENV_VALUE_LEN {
            return Err(ModelinferError::new(
                codes::EXTRA_ENV_INVALID,
                format!("额外环境变量值过长（最多 {MAX_ENV_VALUE_LEN} 字符）: {key}"),
            ));
        }
        out.insert(key.to_string(), Value::String(text));
    }
    Ok(out)
}

/// 自动重启策略：不自动重启（创建默认）
pub const AUTO_RESTART_POLICY_NEVER: &str = "never";
/// 自动重启策略：除用户显式停止外，启动失败 / 探活失败均自动重启（docker unless-stopped 语义）
pub const AUTO_RESTART_POLICY_UNLESS_STOP: &str = "unless-stop";
/// 连续探活失败次数阈值下限
pub const MIN_AUTO_RESTART_FAIL_COUNT: i64 = 1;
/// 连续探活失败次数阈值上限
pub const MAX_AUTO_RESTART_FAIL_COUNT: i64 = 100;
/// 连续探活失败次数阈值默认值（按默认 30s 巡检周期约 5 分钟）
pub const DEFAULT_AUTO_RESTART_FAIL_COUNT: i32 = 10;

/// 归一化自动重启策略（纯函数）
///
/// 接受 `never` / `unless-stop`；兼容 `unless_stopped` / `unless-stopped` 拼写。
/// 其它值返回 `20748`（`AUTO_RESTART_CONFIG_INVALID`）。
pub fn normalize_auto_restart_policy(raw: &str) -> Result<String, ModelinferError> {
    let normalized = raw.trim().to_ascii_lowercase();
    match normalized.as_str() {
        "" | "never" => Ok(AUTO_RESTART_POLICY_NEVER.to_string()),
        "unless-stop" | "unless_stop" | "unless-stopped" | "unless_stopped" => {
            Ok(AUTO_RESTART_POLICY_UNLESS_STOP.to_string())
        }
        _ => Err(ModelinferError::new(
            codes::AUTO_RESTART_CONFIG_INVALID,
            format!(
                "自动重启策略必须为 {AUTO_RESTART_POLICY_NEVER} 或 {AUTO_RESTART_POLICY_UNLESS_STOP}（当前 {raw}）"
            ),
        )),
    }
}

/// 归一化连续探活失败次数阈值（纯函数）
///
/// 合法区间 [`MIN_AUTO_RESTART_FAIL_COUNT`, `MAX_AUTO_RESTART_FAIL_COUNT`]；越界 / 非正返回
/// `20748`。
pub fn normalize_auto_restart_fail_count(raw: i64) -> Result<i32, ModelinferError> {
    if !(MIN_AUTO_RESTART_FAIL_COUNT..=MAX_AUTO_RESTART_FAIL_COUNT).contains(&raw) {
        return Err(ModelinferError::new(
            codes::AUTO_RESTART_CONFIG_INVALID,
            format!(
                "自动重启连续探活失败次数必须在 {MIN_AUTO_RESTART_FAIL_COUNT}~{MAX_AUTO_RESTART_FAIL_COUNT} 次之间（当前 {raw}）"
            ),
        ));
    }
    Ok(raw as i32)
}

/// 历史阈值（秒）→ 新阈值（次数）的兼容换算：按默认 30s 巡检周期折算并归一。
pub fn legacy_auto_restart_fail_count(fail_secs: i64) -> i32 {
    let count = (fail_secs.max(1) + 29) / 30;
    count.clamp(MIN_AUTO_RESTART_FAIL_COUNT, MAX_AUTO_RESTART_FAIL_COUNT) as i32
}

/// 生效权重 `(weight_id, source)`（纯函数）
///
/// 覆盖优先；否则为卡片 `model` + 用户选择的变体来源。`weight_id` 为空表示该运行配置不使用
/// 权重（无权重卡片且未覆盖）。
pub fn effective_weight<'a>(
    card_model: &'a str,
    hub_source: &'a str,
    over: Option<(&'a str, &'a str)>,
) -> (String, String) {
    match over {
        Some((source, model_id)) if !model_id.trim().is_empty() => {
            (model_id.trim().to_string(), source.trim().to_string())
        }
        _ => (card_model.trim().to_string(), hub_source.trim().to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn brief(hostname: &str) -> NodeBrief {
        NodeBrief {
            id: hostname.into(),
            name: hostname.into(),
            host: "10.0.0.1".into(),
            hostname: hostname.into(),
        }
    }

    #[test]
    fn lifecycle_order_validation() {
        assert!(check_lifecycle_order(&json!({})).is_ok());
        assert!(check_lifecycle_order(&json!({"lifecycle-order": {"start": "worker-first-reverse"}})).is_ok());
        assert_eq!(
            check_lifecycle_order(&json!({"lifecycle-order": {"start": "random"}})).unwrap_err().code,
            codes::LIFECYCLE_ORDER_INVALID
        );
        assert_eq!(
            check_lifecycle_order(&json!({"lifecycle-order": {"start": 1}})).unwrap_err().code,
            codes::LIFECYCLE_ORDER_INVALID
        );
        // 旧 camelCase 字段不被识别（无兼容别名）
        assert!(check_lifecycle_order(&json!({"lifecycleOrder": {"start": "random"}})).is_ok());
    }

    #[test]
    fn auto_restart_policy_normalization() {
        assert_eq!(normalize_auto_restart_policy("never").unwrap(), AUTO_RESTART_POLICY_NEVER);
        assert_eq!(
            normalize_auto_restart_policy("unless-stop").unwrap(),
            AUTO_RESTART_POLICY_UNLESS_STOP
        );
        assert_eq!(
            normalize_auto_restart_policy(" Unless_Stopped ").unwrap(),
            AUTO_RESTART_POLICY_UNLESS_STOP
        );
        assert_eq!(normalize_auto_restart_policy("").unwrap(), AUTO_RESTART_POLICY_NEVER);
        for bad in ["always", "unless-stoppedx", "restart"] {
            assert_eq!(
                normalize_auto_restart_policy(bad).unwrap_err().code,
                codes::AUTO_RESTART_CONFIG_INVALID,
                "bad={bad}"
            );
        }
    }

    #[test]
    fn auto_restart_fail_count_bounds() {
        assert_eq!(normalize_auto_restart_fail_count(1).unwrap(), 1);
        assert_eq!(normalize_auto_restart_fail_count(10).unwrap(), 10);
        assert_eq!(normalize_auto_restart_fail_count(100).unwrap(), 100);
        for bad in [0i64, -1, 101, 86_400] {
            let err = normalize_auto_restart_fail_count(bad).unwrap_err();
            assert_eq!(err.code, codes::AUTO_RESTART_CONFIG_INVALID, "bad={bad}");
        }
        assert_eq!(DEFAULT_AUTO_RESTART_FAIL_COUNT, 10);
        // 历史 300s ≈ 10 次；120s ≈ 4 次
        assert_eq!(legacy_auto_restart_fail_count(300), 10);
        assert_eq!(legacy_auto_restart_fail_count(120), 4);
        assert_eq!(legacy_auto_restart_fail_count(1), 1);
    }

    #[test]
    fn unique_hostname_check() {
        assert!(check_unique_hostnames(&[brief("a"), brief("b")]).is_ok());
        assert!(check_unique_hostnames(&[brief("a"), brief("")]).is_ok());
        let err = check_unique_hostnames(&[brief("a"), brief("a")]).unwrap_err();
        assert_eq!(err.code, codes::DEPLOYMENT_CONFLICT);
    }

    #[test]
    fn editable_only_when_inactive() {
        assert!(is_editable("stopped", "stopped"));
        assert!(is_editable("failed", "stopped"));
        // 过渡窗口：停止清理未完成
        assert!(!is_editable("stopped", "stopping"));
        // 活跃态
        assert!(!is_editable("starting", "running"));
        assert!(!is_editable("running", "running"));
        assert!(!is_editable("stopping", "stopping"));
        assert!(!is_editable("degraded", "running"));
    }

    #[test]
    fn caps_max_nodes_defaults_and_invalid() {
        // 无 caps / 无 max-nodes / null → 0（不限制）
        assert_eq!(caps_max_nodes(&json!({})).unwrap(), 0);
        assert_eq!(caps_max_nodes(&json!({"caps": {}})).unwrap(), 0);
        assert_eq!(caps_max_nodes(&json!({"caps": {"max-nodes": null}})).unwrap(), 0);
        // 非负整数 / 整值浮点
        assert_eq!(caps_max_nodes(&json!({"caps": {"max-nodes": 0}})).unwrap(), 0);
        assert_eq!(caps_max_nodes(&json!({"caps": {"max-nodes": 1}})).unwrap(), 1);
        assert_eq!(caps_max_nodes(&json!({"caps": {"max-nodes": 1.0}})).unwrap(), 1);
        // caps 内未知键（含平台 caps 键）一律忽略
        assert_eq!(
            caps_max_nodes(&json!({"caps": {"max-nodes": 1, "software": ["cuda"], "device-type": "e1001"}}))
                .unwrap(),
            1
        );
        // 非法取值：负数 / 字符串 / 小数 / caps 非对象 → 20715（不静默放行）
        for bad in [
            json!({"caps": {"max-nodes": -1}}),
            json!({"caps": {"max-nodes": "1"}}),
            json!({"caps": {"max-nodes": 1.5}}),
            json!({"caps": []}),
            json!({"caps": "x"}),
        ] {
            assert_eq!(caps_max_nodes(&bad).unwrap_err().code, codes::MANIFEST_INVALID, "{bad}");
        }
    }

    #[test]
    fn caps_node_limit_check() {
        let one = json!({"caps": {"max-nodes": 1}});
        // 未声明 caps → 恒通过
        assert!(check_caps_node_limit(&json!({}), 4).is_ok());
        // max-nodes = 0 → 不限制
        assert!(check_caps_node_limit(&json!({"caps": {"max-nodes": 0}}), 4).is_ok());
        // 边界：等于上限通过，超过即 20727
        assert!(check_caps_node_limit(&one, 1).is_ok());
        assert_eq!(
            check_caps_node_limit(&one, 2).unwrap_err().code,
            codes::CAPS_LIMIT_EXCEEDED
        );
        // 非法 max-nodes 取值在节点校验时同样暴露为 20715
        assert_eq!(
            check_caps_node_limit(&json!({"caps": {"max-nodes": -1}}), 1)
                .unwrap_err()
                .code,
            codes::MANIFEST_INVALID
        );
    }

    #[test]
    fn config_values_defaults_and_types() {
        let schema = vec![
            json!({"name":"A","type":"integer","default":1}),
            json!({"name":"B","type":"boolean","default":true}),
            json!({"name":"C","type":"string"}),
        ];
        let mut values = Map::new();
        values.insert("A".into(), json!(5));
        values.insert("B".into(), json!(false));
        values.insert("C".into(), json!("x"));
        values.insert("D".into(), json!("ignored"));
        let out = validate_config_values(&schema, &values).unwrap();
        assert_eq!(out.get("A"), Some(&json!(5)));
        assert_eq!(out.get("B"), Some(&json!(false)));
        assert!(!out.contains_key("D"));

        // 缺必填
        let mut v2 = Map::new();
        v2.insert("A".into(), json!(1));
        v2.insert("B".into(), json!(true));
        assert_eq!(
            validate_config_values(&schema, &v2).unwrap_err().code,
            codes::PARAM_INVALID
        );

        // 类型错误
        let mut v3 = Map::new();
        v3.insert("A".into(), json!("abc"));
        v3.insert("B".into(), json!(true));
        v3.insert("C".into(), json!("x"));
        assert_eq!(
            validate_config_values(&schema, &v3).unwrap_err().code,
            codes::PARAM_INVALID
        );
    }

    #[test]
    fn weight_override_validation() {
        // 未覆盖
        assert_eq!(validate_weight_override("", "").unwrap(), None);
        assert_eq!(validate_weight_override("  ", " ").unwrap(), None);
        // 合法（含跨来源）
        assert_eq!(
            validate_weight_override("ms", " Qwen/Qwen3-8B ").unwrap(),
            Some(("ms".to_string(), "Qwen/Qwen3-8B".to_string()))
        );
        assert_eq!(
            validate_weight_override("hf", "org/repo").unwrap(),
            Some(("hf".to_string(), "org/repo".to_string()))
        );
        // 不成对
        assert_eq!(
            validate_weight_override("hf", "").unwrap_err().code,
            codes::WEIGHT_OVERRIDE_INVALID
        );
        assert_eq!(
            validate_weight_override("", "org/repo").unwrap_err().code,
            codes::WEIGHT_OVERRIDE_INVALID
        );
        // 非法来源 / id 形态
        for (s, m) in [
            ("hf2", "org/repo"),
            ("hf", "repo"),
            ("hf", "org/repo/extra"),
            ("hf", "org//repo"),
            ("hf", "../repo"),
            ("hf", "org/re po"),
            ("hf", "org\\repo"),
        ] {
            assert_eq!(
                validate_weight_override(s, m).unwrap_err().code,
                codes::WEIGHT_OVERRIDE_INVALID,
                "{s}:{m}"
            );
        }
    }

    #[test]
    fn images_override_validation() {
        // 空 = 未覆盖
        assert!(validate_images_override(&[]).unwrap().is_empty());
        // 字符串 + 对象（arch 归一）
        let out = validate_images_override(&[
            json!("ubuntu:22.04"),
            json!({"name": "repo/x:1", "arch": "aarch64"}),
            json!({"name": "repo/y:2", "arch": ""}),
        ])
        .unwrap();
        assert_eq!(out[0], json!("ubuntu:22.04"));
        assert_eq!(out[1], json!({"name": "repo/x:1", "arch": "arm64"}));
        assert_eq!(out[2], json!({"name": "repo/y:2"}));
        // 非法
        for bad in [
            vec![json!(1)],
            vec![json!("")],
            vec![json!({"arch": "amd64"})],
            vec![json!({"name": "   "})],
            vec![json!({"name": "x:1", "arch": "riscv64"})],
            vec![json!("repo/x@sha256:abcd")],
        ] {
            assert_eq!(
                validate_images_override(&bad).unwrap_err().code,
                codes::IMAGE_OVERRIDE_INVALID,
                "{bad:?}"
            );
        }
        // 上限
        let too_many: Vec<Value> = (0..=MAX_IMAGES_OVERRIDE)
            .map(|i| json!(format!("repo/x{i}:1")))
            .collect();
        assert_eq!(
            validate_images_override(&too_many).unwrap_err().code,
            codes::IMAGE_OVERRIDE_INVALID
        );
    }

    #[test]
    fn extra_env_validation() {
        let card = vec!["MAX_MODEL_LEN".to_string()];
        // 合法：字符串 / 数字 / 布尔归一为字符串
        let mut ok = Map::new();
        ok.insert("A".into(), json!("x"));
        ok.insert("B".into(), json!(1));
        ok.insert("C".into(), json!(true));
        let out = validate_extra_env(&ok, &card).unwrap();
        assert_eq!(out.get("A"), Some(&json!("x")));
        assert_eq!(out.get("B"), Some(&json!("1")));
        assert_eq!(out.get("C"), Some(&json!("true")));
        // 空
        assert!(validate_extra_env(&Map::new(), &card).unwrap().is_empty());
        // 非法：键形态 / 保留前缀 / 系统变量 / 与 configs 同名 / 非法值类型
        for (k, v) in [
            ("1BAD", json!("x")),
            ("A-B", json!("x")),
            ("", json!("x")),
            ("MODELINFER_SERVE_PORT", json!("1")),
            ("PATH", json!("/tmp")),
            ("MAX_MODEL_LEN", json!(1)),
            ("OK", json!([1, 2])),
        ] {
            let mut m = Map::new();
            m.insert(k.into(), v);
            assert_eq!(
                validate_extra_env(&m, &card).unwrap_err().code,
                codes::EXTRA_ENV_INVALID,
                "{k}"
            );
        }
        // 值超长
        let mut long = Map::new();
        long.insert("OK".into(), json!("x".repeat(MAX_ENV_VALUE_LEN + 1)));
        assert_eq!(
            validate_extra_env(&long, &card).unwrap_err().code,
            codes::EXTRA_ENV_INVALID
        );
        // 条目超限
        let mut many = Map::new();
        for i in 0..=MAX_EXTRA_ENV {
            many.insert(format!("K{i}"), json!("v"));
        }
        assert_eq!(
            validate_extra_env(&many, &card).unwrap_err().code,
            codes::EXTRA_ENV_INVALID
        );
    }

    #[test]
    fn effective_weight_prefers_override() {
        // 覆盖优先
        assert_eq!(
            effective_weight("card/model", "hf", Some(("ms", "org/other"))),
            ("org/other".to_string(), "ms".to_string())
        );
        // 无覆盖
        assert_eq!(
            effective_weight("card/model", "ms", None),
            ("card/model".to_string(), "ms".to_string())
        );
        // 覆盖 model_id 为空 → 视为未覆盖
        assert_eq!(
            effective_weight("card/model", "hf", Some(("ms", "  "))),
            ("card/model".to_string(), "hf".to_string())
        );
        // 无权重卡片
        assert_eq!(
            effective_weight("", "", None),
            (String::new(), String::new())
        );
    }
}