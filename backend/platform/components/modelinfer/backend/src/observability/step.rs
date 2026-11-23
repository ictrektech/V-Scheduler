//! 运行步骤的纯函数辅助：步骤标识 / 中文标题 / 阶段关联

/// 生命周期阶段步骤标识（`stage:<阶段>`）
pub fn stage_step_id(stage: &str) -> String {
    format!("stage:{stage}")
}

/// 是否为生命周期阶段步骤
pub fn is_stage_step(step: &str) -> bool {
    step.starts_with("stage:")
}

/// 从阶段步骤标识提取阶段名
pub fn stage_of_step(step: &str) -> Option<&str> {
    step.strip_prefix("stage:")
}

/// 步骤中文标题（未知步骤回落为标识本身）
pub fn step_title(step: &str) -> String {
    let known = match step {
        "prepare" => Some("准备部署"),
        "check-node" => Some("节点连通性检查"),
        "ensure-images" => Some("镜像预检与分发"),
        "mount-nfs" => Some("挂载 NFS"),
        "distribute" => Some("分发模型与脚本"),
        "register-vrouter" => Some("注册到统一网关"),
        "cleanup" => Some("停止清理"),
        "umount" => Some("解除 NFS 挂载"),
        "deregister-vrouter" => Some("注销统一网关"),
        "finish" => Some("收尾"),
        _ => None,
    };
    if let Some(t) = known {
        return t.to_string();
    }
    if let Some(stage) = stage_of_step(step) {
        return format!("执行阶段 {stage}");
    }
    step.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stage_step_id_and_parse_roundtrip() {
        let id = stage_step_id("health-probe");
        assert_eq!(id, "stage:health-probe");
        assert!(is_stage_step(&id));
        assert_eq!(stage_of_step(&id), Some("health-probe"));
        assert!(!is_stage_step("prepare"));
        assert_eq!(stage_of_step("prepare"), None);
    }

    #[test]
    fn titles_for_known_and_stage_steps() {
        assert_eq!(step_title("ensure-images"), "镜像预检与分发");
        assert_eq!(step_title("stage:start"), "执行阶段 start");
        assert_eq!(step_title("unknown-x"), "unknown-x");
    }
}