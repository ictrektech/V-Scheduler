//! 统一 HTTP 响应与错误模型（对齐 specs/docs/design/vos-http-response-spec/）
//!
//! - 响应体统一 `ResultBody { code, category, data, msg, version }`
//! - `code == 0` 成功；`code != 0` 失败（前端按 code 判成败，msg 仅调试辅助）
//! - HTTP 状态码统一 200（业务错误走 body.code）
//! - 本组件错误码区间 `[20500, +∞)`，category 回退见 `category_for_code`

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

/// 应用 category 字面值（对齐 vos-http-response-spec 应用自定义段）
pub const CATEGORY: &str = "vos-cluster-manager";

/// VOS HTTP 统一响应顶层结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResultBody<T> {
    pub code: i32,
    #[serde(default = "default_category")]
    pub category: String,
    pub data: Option<T>,
    pub msg: String,
    pub version: String,
}

fn default_category() -> String {
    "ok".to_owned()
}

impl<T> ResultBody<T> {
    pub fn ok(data: T, version: impl Into<String>) -> Self {
        Self {
            code: 0,
            category: "ok".to_owned(),
            data: Some(data),
            msg: String::new(),
            version: version.into(),
        }
    }

    pub fn ok_empty(version: impl Into<String>) -> ResultBody<Option<T>> {
        ResultBody {
            code: 0,
            category: "ok".to_owned(),
            data: None,
            msg: String::new(),
            version: version.into(),
        }
    }

    pub fn failed(code: i32, msg: impl Into<String>, version: impl Into<String>) -> Self {
        Self {
            code,
            category: category_for_code(code).to_owned(),
            data: None,
            msg: msg.into(),
            version: version.into(),
        }
    }
}

/// 按 code 范围回退 category（对齐 vos-http-response-spec §3.3.3）：
/// 前端在 category 缺失时按区间回退使用。
pub fn category_for_code(code: i32) -> &'static str {
    match code {
        0 => "ok",
        1..=9999 => "system",
        10000..=19999 => "vos-platform",
        // [20000, +∞) 应用自定义区间：本组件错误码 [20500, +∞)
        _ => CATEGORY,
    }
}

/// 本组件业务错误码（category = "vos-cluster-manager"，区间 [20500, +∞)）
pub mod codes {
    /// 参数校验失败（非法名称 / 大小 / 格式 / 路径）
    pub const PARAM_INVALID: i32 = 20501;
    /// 数据目录 / 文件 IO 失败
    pub const IO_FAILED: i32 = 20502;
    /// 本地管理状态库初始化 / 访问失败
    pub const DB_FAILED: i32 = 20503;
    /// etcd 连接 / 读写失败（集群状态权威信源）
    pub const ETCD_FAILED: i32 = 20504;
    /// 资源不存在（集群 / 节点 / 任务 / 配置）
    pub const NOT_FOUND: i32 = 20505;
    /// 资源已存在（节点重复 / 集群重复初始化）
    pub const EXISTS: i32 = 20506;
    /// 集群状态不支持该操作（未初始化 / 初始化中 / 拓扑约束不满足）
    pub const STATE_NOT_SUPPORTED: i32 = 20507;
    /// 平台 projects API 调用失败（etcd / postgres-ha 子项目动态启停）
    pub const PROJECTS_CALL_FAILED: i32 = 20508;
    /// 跨节点远程调用失败（SSH / HTTP 凭据校验、节点不可达）
    pub const REMOTE_NODE_FAILED: i32 = 20509;
    /// 凭据校验失败（管理员密码错误 / 节点拒绝）
    pub const CREDENTIAL_INVALID: i32 = 20510;
    /// 集群创建 / 成员加入编排失败（含 saga 补偿后的终态失败）
    pub const ORCHESTRATION_FAILED: i32 = 20511;
    /// postgres HA（patroni）子项目异常（bootstrap/join 失败、无主、未就绪）
    pub const POSTGRES_HA_FAILED: i32 = 20512;
    /// haproxy 子项目异常（配置渲染/校验失败等，二期 S1；projects 调用失败仍归 20508）
    pub const HAPROXY_FAILED: i32 = 20513;
    /// 集群 temporal 子项目异常（二期 S2；自举失败 / DB 经 haproxy 不可达 / 无就绪实例）
    pub const TEMPORAL_FAILED: i32 = 20514;
    /// etcd 认证/TLS 异常（角色/用户创建失败、凭据缺失、启用认证被拒、证书签发失败）
    pub const ETCD_AUTH_FAILED: i32 = 20515;
    /// 存储网关 VIP 配置异常（网关集合非法 / 排空中的网关仍被要求启用 / DB 不可用）
    pub const GATEWAY_FAILED: i32 = 20516;
}

/// 本组件错误：携带 code + msg（可选 trackerror 上下文），转换为 ResultBody JSON 响应
#[derive(Debug, thiserror::Error)]
#[error("{msg}")]
pub struct VcmError {
    pub code: i32,
    pub msg: String,
}

impl VcmError {
    pub fn new(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }
}

/// 从 trackerror `Error` 映射为本组件错误（保留 errno 与诊断信息）
impl From<trackerror::Error> for VcmError {
    fn from(e: trackerror::Error) -> Self {
        VcmError {
            code: e.as_errno(),
            msg: e.get_error_msg(),
        }
    }
}

impl IntoResponse for VcmError {
    fn into_response(self) -> Response {
        let body = ResultBody::<serde_json::Value>::failed(
            self.code,
            self.msg,
            env!("CARGO_PKG_VERSION"),
        );
        (StatusCode::OK, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_range_fallback() {
        assert_eq!(category_for_code(0), "ok");
        assert_eq!(category_for_code(1), "system");
        assert_eq!(category_for_code(10001), "vos-platform");
        assert_eq!(category_for_code(20501), CATEGORY);
        assert_eq!(category_for_code(99999), CATEGORY);
    }

    #[test]
    fn result_body_ok_and_failed() {
        let ok = ResultBody::ok(42i32, "0.1.0");
        assert_eq!(ok.code, 0);
        assert_eq!(ok.category, "ok");
        assert_eq!(ok.data, Some(42));

        let err = ResultBody::<serde_json::Value>::failed(20501, "bad", "0.1.0");
        assert_eq!(err.code, 20501);
        assert_eq!(err.category, CATEGORY);
        assert!(err.data.is_none());
    }
}