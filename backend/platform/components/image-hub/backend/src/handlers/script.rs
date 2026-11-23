//! `GET <url-base>/image-hub.sh`：一键客户端脚本分发
//!
//! 见 `components/image-hub/openspec/changes/image-hub-client-script/`（D-IH-68 / D-IH-75）。

use std::path::PathBuf;

use axum::{
    body::Body,
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};

/// 读取脚本文件并构造响应：命中 → `200` + `text/x-shellscript`（逐字节透传，脚本不含动态内容）；
/// 缺失 / 读失败 → `404` 纯文本。
///
/// 使用**显式路由**而非依赖静态托管：避免 SPA fallback 在文件缺失时回落为 `index.html`
/// （把 HTML 当脚本执行）。
pub async fn serve_script(path: &PathBuf) -> Response {
    match tokio::fs::read(path).await {
        Ok(bytes) => (
            StatusCode::OK,
            [(header::CONTENT_TYPE, "text/x-shellscript; charset=utf-8")],
            Body::from(bytes),
        )
            .into_response(),
        Err(_) => (
            StatusCode::NOT_FOUND,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            Body::from("image-hub.sh not found\n"),
        )
            .into_response(),
    }
}