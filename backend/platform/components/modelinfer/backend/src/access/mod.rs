//! 标准接入配置：运行配置的接入参数（模型名 / API Key / 端口 / URL 前缀）、自动推断与
//! 脚本 `global.serve` 回传合并。
//!
//! 设计（见 `openspec/changes/modelinfer-vrouter-registration/design.md`；统一网关检测迁移见
//! `openspec/changes/modelinfer-vscheduler-migration/design.md`）：
//! - **平台推断**：端口 / URL 前缀 / 模型名 / API Key 取运行配置（已含自动推断），host 取 head 节点；
//! - **脚本回传**：`start` 阶段脚本可经 `modelinfer:` 协议在 `global.serve` 回传最终值，
//!   **脚本值 > 平台推断**；
//! - 合并逻辑为纯函数，便于单测；网络 / 存储依赖在调用方完成。

use serde::Serialize;
use serde_json::{Map, Value};

/// 默认服务端口
pub const DEFAULT_SERVE_PORT: u16 = 8000;
/// 默认 API URL 前缀
pub const DEFAULT_URL_PREFIX: &str = "/v1";
/// 字段来源：平台推断
pub const SOURCE_PLATFORM: &str = "platform";
/// 字段来源：脚本回传
pub const SOURCE_SCRIPT: &str = "script";

/// 归一化 URL 前缀：保证以 `/` 开头、结尾无 `/`；空 / 非法 → 默认 `/v1`
///
/// - `/v1` → `/v1`；`v1` → `/v1`；`/v1/` → `/v1`；`/` / 空 → `/v1`
pub fn normalize_url_prefix(raw: &str) -> String {
    let s = raw.trim();
    if s.is_empty() {
        return DEFAULT_URL_PREFIX.to_string();
    }
    let s = s.trim_matches('/');
    if s.is_empty() {
        return DEFAULT_URL_PREFIX.to_string();
    }
    format!("/{s}")
}

/// 归一化端口：合法 `1..=65535` 原样返回，否则默认 `8000`
pub fn normalize_serve_port(raw: Option<u64>) -> u16 {
    match raw {
        Some(p) if (1..=65535).contains(&p) => p as u16,
        _ => DEFAULT_SERVE_PORT,
    }
}

/// 从权重 id 推断默认模型名（取末段），为空时回落卡片 id
///
/// - `Qwen/Qwen3.8-27B` → `Qwen3.8-27B`
/// - `deepseek-ai/DeepSeek-V4.1-Flash` → `DeepSeek-V4.1-Flash`
pub fn default_model_name(weight_id: &str, card_id: &str) -> String {
    let last = weight_id.trim().trim_end_matches('/').rsplit('/').next().unwrap_or("");
    if !last.is_empty() {
        return last.to_string();
    }
    card_id.trim().to_string()
}

/// 平台推断的接入默认值
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformAccess {
    /// head 节点地址（IP）
    pub host: String,
    pub port: u16,
    /// 归一化后的 URL 前缀
    pub url_prefix: String,
    pub model: String,
    pub api_key: String,
}

/// 解析后的接入配置
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedAccess {
    pub host: String,
    pub port: u16,
    pub url_prefix: String,
    pub model: String,
    pub api_key: String,
    /// OpenAI 兼容基地址（`http://<host>:<port><url_prefix>`；host 为空时为空串）
    pub base_url: String,
    /// 各字段来源（`platform` / `script`）
    pub sources: Map<String, Value>,
}

impl ResolvedAccess {
    /// OpenAI SDK `base_url`（与 [`Self::base_url`] 同值；语义命名）
    pub fn openai_base_url(&self) -> String {
        self.base_url.clone()
    }

    /// 浏览器访问地址（**不含** URL 前缀）：host 非空时 `http://<host>:<port>`，否则空串。
    ///
    /// 供 ComfyUI 等 Web UI 引擎展示访问地址（其服务根路径不是 OpenAI 的 `/v1`）。
    pub fn web_url(&self) -> String {
        if self.host.trim().is_empty() {
            String::new()
        } else {
            format!("http://{}:{}", self.host, self.port)
        }
    }
}

/// 有效服务端口：脚本 `global.serve.port`（合法 `1..=65535`）优先，否则回落 `default_port`。
///
/// 接入配置展示（`resolved.port`）与本地反代转发目标 MUST 使用同源有效端口，
/// 避免固定使用运行配置持久化的 `serve_port`（ComfyUI 卡片经脚本回传真实端口）。
pub fn effective_serve_port(default_port: u16, script_serve: Option<&Value>) -> u16 {
    script_serve
        .and_then(Value::as_object)
        .and_then(|o| o.get("port"))
        .and_then(as_u64)
        .filter(|p| (1..=65535).contains(p))
        .map(|p| p as u16)
        .unwrap_or(default_port)
}

fn as_u64(v: &Value) -> Option<u64> {
    match v {
        Value::Number(n) => n.as_u64(),
        Value::String(s) => s.trim().parse::<u64>().ok(),
        _ => None,
    }
}

fn as_non_empty_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// 合并平台默认与脚本回传的 `serve` 对象（脚本值 > 平台推断）
///
/// `script_serve` 为 `None` 或非对象时全部取平台默认。非法字段值被忽略并回落平台默认。
pub fn resolve_access(defaults: &PlatformAccess, script_serve: Option<&Value>) -> ResolvedAccess {
    let mut host = defaults.host.trim().to_string();
    let mut port = defaults.port;
    let mut url_prefix = normalize_url_prefix(&defaults.url_prefix);
    let mut model = defaults.model.trim().to_string();
    let mut api_key = defaults.api_key.clone();
    let mut sources: Map<String, Value> = Map::new();
    for f in ["host", "port", "url_prefix", "model", "api_key"] {
        sources.insert(f.to_string(), Value::String(SOURCE_PLATFORM.to_string()));
    }

    if let Some(obj) = script_serve.and_then(Value::as_object) {
        if let Some(ip) = obj.get("ip").and_then(as_non_empty_str) {
            host = ip;
            sources.insert("host".into(), Value::String(SOURCE_SCRIPT.into()));
        }
        if let Some(p) = obj.get("port").and_then(as_u64)
            && (1..=65535).contains(&p)
        {
            port = p as u16;
            sources.insert("port".into(), Value::String(SOURCE_SCRIPT.into()));
        }
        if let Some(p) = obj.get("url_prefix").and_then(as_non_empty_str) {
            url_prefix = normalize_url_prefix(&p);
            sources.insert("url_prefix".into(), Value::String(SOURCE_SCRIPT.into()));
        }
        if let Some(m) = obj.get("model").and_then(as_non_empty_str) {
            model = m;
            sources.insert("model".into(), Value::String(SOURCE_SCRIPT.into()));
        }
        if let Some(k) = obj.get("api_key").and_then(as_non_empty_str) {
            api_key = k;
            sources.insert("api_key".into(), Value::String(SOURCE_SCRIPT.into()));
        }
    }

    let base_url = if host.is_empty() {
        String::new()
    } else {
        format!("http://{host}:{port}{url_prefix}")
    };
    ResolvedAccess {
        host,
        port,
        url_prefix,
        model,
        api_key,
        base_url,
        sources,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn url_prefix_normalization() {
        assert_eq!(normalize_url_prefix("/v1"), "/v1");
        assert_eq!(normalize_url_prefix("v1"), "/v1");
        assert_eq!(normalize_url_prefix("/v1/"), "/v1");
        assert_eq!(normalize_url_prefix("/openai/v1/"), "/openai/v1");
        assert_eq!(normalize_url_prefix(""), DEFAULT_URL_PREFIX);
        assert_eq!(normalize_url_prefix("/"), DEFAULT_URL_PREFIX);
        assert_eq!(normalize_url_prefix("   "), DEFAULT_URL_PREFIX);
    }

    #[test]
    fn serve_port_normalization() {
        assert_eq!(normalize_serve_port(Some(8000)), 8000);
        assert_eq!(normalize_serve_port(Some(1)), 1);
        assert_eq!(normalize_serve_port(Some(65535)), 65535);
        assert_eq!(normalize_serve_port(Some(0)), DEFAULT_SERVE_PORT);
        assert_eq!(normalize_serve_port(Some(70000)), DEFAULT_SERVE_PORT);
        assert_eq!(normalize_serve_port(None), DEFAULT_SERVE_PORT);
    }

    #[test]
    fn model_name_inference() {
        assert_eq!(default_model_name("Qwen/Qwen3.8-27B", "c"), "Qwen3.8-27B");
        assert_eq!(
            default_model_name("deepseek-ai/DeepSeek-V4.1-Flash", "c"),
            "DeepSeek-V4.1-Flash"
        );
        assert_eq!(default_model_name("", "com.ictrek.card"), "com.ictrek.card");
        assert_eq!(default_model_name("/", "card"), "card");
    }

    fn defaults() -> PlatformAccess {
        PlatformAccess {
            host: "10.10.11.154".into(),
            port: 8000,
            url_prefix: "/v1".into(),
            model: "Qwen3.8-27B".into(),
            api_key: "KEY".into(),
        }
    }

    #[test]
    fn resolve_without_script_uses_platform() {
        let r = resolve_access(&defaults(), None);
        assert_eq!(r.host, "10.10.11.154");
        assert_eq!(r.port, 8000);
        assert_eq!(r.url_prefix, "/v1");
        assert_eq!(r.model, "Qwen3.8-27B");
        assert_eq!(r.api_key, "KEY");
        assert_eq!(r.base_url, "http://10.10.11.154:8000/v1");
        assert_eq!(r.sources.get("port"), Some(&json!("platform")));
    }

    #[test]
    fn resolve_script_overrides() {
        let serve = json!({"port": 9000, "ip": "10.10.11.155", "model": "custom", "url_prefix": "/openai/v1"});
        let r = resolve_access(&defaults(), Some(&serve));
        assert_eq!(r.host, "10.10.11.155");
        assert_eq!(r.port, 9000);
        assert_eq!(r.url_prefix, "/openai/v1");
        assert_eq!(r.model, "custom");
        // 未回传的 api_key 保留平台值
        assert_eq!(r.api_key, "KEY");
        assert_eq!(r.sources.get("host"), Some(&json!("script")));
        assert_eq!(r.sources.get("api_key"), Some(&json!("platform")));
        assert_eq!(r.base_url, "http://10.10.11.155:9000/openai/v1");
    }

    #[test]
    fn resolve_ignores_invalid_script_values() {
        let serve = json!({"port": 0, "ip": "  ", "model": "", "url_prefix": ""});
        let r = resolve_access(&defaults(), Some(&serve));
        assert_eq!(r.port, 8000);
        assert_eq!(r.host, "10.10.11.154");
        assert_eq!(r.model, "Qwen3.8-27B");
        assert_eq!(r.url_prefix, "/v1");
        // 非对象忽略
        let r2 = resolve_access(&defaults(), Some(&json!("nope")));
        assert_eq!(r2, resolve_access(&defaults(), None));
    }

    #[test]
    fn resolve_empty_host_base_url_empty() {
        let mut d = defaults();
        d.host = String::new();
        let r = resolve_access(&d, None);
        assert_eq!(r.base_url, "");
        assert_eq!(r.web_url(), "");
    }

    #[test]
    fn web_url_without_prefix() {
        let r = resolve_access(&defaults(), None);
        assert_eq!(r.web_url(), "http://10.10.11.154:8000");
        // 脚本回传端口后 web_url 随之变化
        let serve = json!({"port": 8188});
        let r = resolve_access(&defaults(), Some(&serve));
        assert_eq!(r.web_url(), "http://10.10.11.154:8188");
    }

    #[test]
    fn effective_serve_port_prefers_script() {
        // 脚本合法端口优先
        assert_eq!(effective_serve_port(8000, Some(&json!({"port": 8188}))), 8188);
        // 字符串端口亦识别
        assert_eq!(effective_serve_port(8000, Some(&json!({"port": "9000"}))), 9000);
        // 无脚本 / 非对象 / 非法端口 → 回落默认
        assert_eq!(effective_serve_port(8000, None), 8000);
        assert_eq!(effective_serve_port(8000, Some(&json!("nope"))), 8000);
        assert_eq!(effective_serve_port(8000, Some(&json!({"port": 0}))), 8000);
        assert_eq!(effective_serve_port(8000, Some(&json!({"port": 70000}))), 8000);
    }
}