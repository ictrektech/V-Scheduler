//! 统一 HTTP 响应与错误模型（对齐 specs/docs/design/vos-http-response-spec/）
//!
//! - 响应体统一 `ResultBody { code, category, data, msg, version }`
//! - `code == 0` 成功；`code != 0` 失败（前端按 code 判成败，msg 仅调试辅助）
//! - HTTP 状态码统一 200（业务错误走 body.code）
//! - 本组件错误码区间 `[20800, +∞)`（与 204xx/205xx/206xx/207xx 区段区分）

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};

/// 应用 category 字面值（对齐 vos-http-response-spec 应用自定义段）
pub const CATEGORY: &str = "image-hub";

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
        // [20000, +∞) 应用自定义区间：本组件错误码 [20800, +∞)
        _ => CATEGORY,
    }
}

/// 本组件业务错误码（category = "image-hub"，区间 [20800, +∞)）
pub mod codes {
    /// 参数校验失败（仓库名 / 标签非法，删除请求缺仓库）
    pub const PARAM_INVALID: i32 = 20801;
    /// registry 不可达 / 请求失败
    pub const REGISTRY_UNREACHABLE: i32 = 20802;
    /// 目标 manifest / 仓库 / 标签不存在
    pub const MANIFEST_NOT_FOUND: i32 = 20803;
    /// registry 删除失败（未开启删除 / registry 返回错误）
    pub const DELETE_FAILED: i32 = 20804;
    /// 平台 API 调用失败（access-token 缺失 / 平台错误）
    pub const PLATFORM_CALL_FAILED: i32 = 20805;
    /// 本地 IO / 数据目录失败
    pub const IO_FAILED: i32 = 20806;
    /// skopeo 执行失败（非零退出 / 无法启动 / 上游拉取失败）
    pub const SKOPEO_FAILED: i32 = 20807;
    /// 异步任务不存在
    pub const TASK_NOT_FOUND: i32 = 20808;
    /// 镜像引用非法（格式错误 / digest 引用 / 非法架构选择）
    pub const IMAGE_REF_INVALID: i32 = 20809;
    /// 任务被取消（skopeo 被取消终止时的内部映射；不作为 API 错误码返回）
    pub const TASK_CANCELLED: i32 = 20810;
    /// registry 垃圾回收执行失败（启动失败 / 非零退出 / 超时；任务失败时的内部映射）
    pub const GC_FAILED: i32 = 20811;
    /// registry 垃圾回收能力不可用（registry 二进制缺失 / 存储目录未配置或不可读）
    pub const GC_UNAVAILABLE: i32 = 20812;
}

/// 本组件错误：携带 code + msg，转换为 ResultBody JSON 响应
#[derive(Debug, thiserror::Error)]
#[error("{msg}")]
pub struct ImageHubError {
    pub code: i32,
    pub msg: String,
}

impl ImageHubError {
    pub fn new(code: i32, msg: impl Into<String>) -> Self {
        Self {
            code,
            msg: msg.into(),
        }
    }
}

/// 从 trackerror `Error` 映射为本组件错误（保留 errno 与诊断信息）
impl From<trackerror::Error> for ImageHubError {
    fn from(e: trackerror::Error) -> Self {
        ImageHubError {
            code: e.as_errno(),
            msg: e.get_error_msg(),
        }
    }
}

impl IntoResponse for ImageHubError {
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
        assert_eq!(category_for_code(20701), CATEGORY);
        assert_eq!(category_for_code(20801), CATEGORY);
        assert_eq!(category_for_code(20809), CATEGORY);
        assert_eq!(category_for_code(20810), CATEGORY);
        assert_eq!(category_for_code(20811), CATEGORY);
        assert_eq!(category_for_code(20812), CATEGORY);
        assert_eq!(category_for_code(99999), CATEGORY);
    }

    #[test]
    fn extended_codes_are_in_own_segment() {
        // 20807-20810 顺延且不与既有 20801-20806 冲突
        let all = [
            codes::PARAM_INVALID,
            codes::REGISTRY_UNREACHABLE,
            codes::MANIFEST_NOT_FOUND,
            codes::DELETE_FAILED,
            codes::PLATFORM_CALL_FAILED,
            codes::IO_FAILED,
            codes::SKOPEO_FAILED,
            codes::TASK_NOT_FOUND,
            codes::IMAGE_REF_INVALID,
            codes::TASK_CANCELLED,
            codes::GC_FAILED,
            codes::GC_UNAVAILABLE,
        ];
        assert_eq!(codes::SKOPEO_FAILED, 20807);
        assert_eq!(codes::TASK_NOT_FOUND, 20808);
        assert_eq!(codes::IMAGE_REF_INVALID, 20809);
        assert_eq!(codes::TASK_CANCELLED, 20810);
        assert_eq!(codes::GC_FAILED, 20811);
        assert_eq!(codes::GC_UNAVAILABLE, 20812);
        let mut sorted = all.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), all.len(), "错误码不得重复");
        assert!(all.iter().all(|c| *c >= 20800));
    }

    #[test]
    fn result_body_ok_and_failed() {
        let ok = ResultBody::ok(42i32, "0.1.0");
        assert_eq!(ok.code, 0);
        assert_eq!(ok.category, "ok");
        assert_eq!(ok.data, Some(42));

        let err = ResultBody::<serde_json::Value>::failed(20801, "bad", "0.1.0");
        assert_eq!(err.code, 20801);
        assert_eq!(err.category, CATEGORY);
        assert!(err.data.is_none());
    }
}