//! 平台 access-token 读取（vos-app-spec §9.1 app-context-access-token）
//!
//! - 路径优先级：`VOS_APP_CONTEXT_PATH/access-token` → `/var/lib/vos-context/access-token`
//! - token 只读消费：不出本机 / 不跨节点分发 / 不写日志 / 不写响应
//! - 读取失败映射 `PROJECTS_CALL_FAILED(20508)`（token 缺失时编排接口不可用）

use std::path::{Path, PathBuf};

use crate::error::codes;
use crate::error::VcmError;

/// 默认 context 目录（compose 只读挂载 `/var/lib/vos-context:ro`）
const DEFAULT_CONTEXT_DIR: &str = "/var/lib/vos-context";

/// 解析 access-token 文件路径（纯函数，便于单测）
///
/// 优先级：`VOS_APP_CONTEXT_PATH`（平台注入，绝对路径）→ 默认 `/var/lib/vos-context`。
/// 环境变量注入但为空 → 回退默认。
pub fn access_token_path(env: &std::collections::HashMap<String, String>) -> PathBuf {
    match env.get("VOS_APP_CONTEXT_PATH") {
        Some(p) if !p.trim().is_empty() => Path::new(p.trim()).join("access-token"),
        _ => PathBuf::from(DEFAULT_CONTEXT_DIR).join("access-token"),
    }
}

/// 读取 access-token（trim + 非空校验）
pub fn read_access_token_from(path: &Path) -> Result<String, VcmError> {
    let raw = std::fs::read_to_string(path).map_err(|e| {
        VcmError::new(
            codes::PROJECTS_CALL_FAILED,
            format!("读取 access-token 失败（{}）: {e}", path.display()),
        )
    })?;
    let token = raw.trim().to_string();
    if token.is_empty() || token.chars().count() > 4096 {
        return Err(VcmError::new(
            codes::PROJECTS_CALL_FAILED,
            format!("access-token 非法（空或超长）: {}", path.display()),
        ));
    }
    Ok(token)
}

/// 便捷：从进程环境读取（bin 装配用）
pub fn read_access_token() -> Result<String, VcmError> {
    let env = std::env::vars().collect::<std::collections::HashMap<_, _>>();
    let path = access_token_path(&env);
    read_access_token_from(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_with(path: &str) -> std::collections::HashMap<String, String> {
        let mut m = std::collections::HashMap::new();
        if !path.is_empty() {
            m.insert("VOS_APP_CONTEXT_PATH".to_string(), path.to_string());
        }
        m
    }

    #[test]
    fn path_resolution_prefers_env() {
        let env = env_with("/custom/context");
        assert_eq!(
            access_token_path(&env),
            PathBuf::from("/custom/context/access-token")
        );
        // 未注入 → 默认
        let env = env_with("");
        assert_eq!(
            access_token_path(&env),
            PathBuf::from("/var/lib/vos-context/access-token")
        );
        // 注入但空值 → 默认
        let mut env = std::collections::HashMap::new();
        env.insert("VOS_APP_CONTEXT_PATH".to_string(), "  ".to_string());
        assert_eq!(
            access_token_path(&env),
            PathBuf::from("/var/lib/vos-context/access-token")
        );
    }

    #[test]
    fn read_token_ok_and_missing() {
        let tmp = tempfile::TempDir::new().unwrap();
        // 缺失 → Err(20508)
        let err = read_access_token_from(&tmp.path().join("nope")).unwrap_err();
        assert_eq!(err.code, codes::PROJECTS_CALL_FAILED);
        // 写入 token → 读取成功（trim）
        let f = tmp.path().join("access-token");
        std::fs::write(&f, "  abc123token\n").unwrap();
        assert_eq!(read_access_token_from(&f).unwrap(), "abc123token");
        // 空 → Err
        std::fs::write(&f, "   \n").unwrap();
        assert_eq!(
            read_access_token_from(&f).unwrap_err().code,
            codes::PROJECTS_CALL_FAILED
        );
    }
}