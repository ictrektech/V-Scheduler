//! 生命周期脚本生成（纯函数，便于单测）
//!
//! - `env.sh`：注入 modelinfer 内置环境变量 + 卡片 `configs` 取值，并 `cd` 到工作目录；
//! - `stage_<name>.sh`：source `env.sh` 后按 manifest 语义执行（string → bash；array → exec）。

use serde_json::Value;

use crate::deploy::{DeployNode, DeployRuntime};
use crate::error::{ModelinferError, codes};

/// 在目标节点 source env.sh 时采集物理链路为 LINK_UP（sysfs 状态码 5）的 IB 设备。
/// 每设备只输出一次；NCCL 的 `=` 前缀精确匹配设备名，避免 mlx5_1 匹配到 mlx5_10。
/// 子 shell 隔离辅助变量 / locale；无 IB 或状态不可读时输出空串，不影响生命周期。
const IB_HCA_LINK_UP_EXPORT: &str = r#"export MODELINFER_NODE_IB_HCA_LINK_UP="$(
    export LC_ALL=C
    separator='='
    for device in /sys/class/infiniband/*; do
        for port_state in "$device"/ports/*/phys_state; do
            [ -r "$port_state" ] || continue
            IFS=' ' read -r state rest 2>/dev/null < "$port_state" || continue
            [ "$state" = '5:' ] || continue
            printf '%s%s' "$separator" "${device##*/}"
            separator=','
            break
        done
    done
)"
"#;

/// POSIX shell 单引号转义
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// 阶段脚本文件名（安全化）
pub fn stage_file(stage: &str) -> String {
    let safe: String = stage
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' || c == '_' { c } else { '_' })
        .collect();
    format!("stage_{safe}.sh")
}

/// 值 → 环境变量字符串
fn env_value(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// 构造静态环境变量列表（实时 LINK_UP 设备由 env.sh 在节点侧探测）
///
/// 注入顺序：平台内置变量 → 卡片 `configs` → 运行配置 `extra_env`。
/// 生效权重（覆盖优先）通过既有 `MODELINFER_MODEL_ID` / `MODELINFER_MODEL_PATH` 暴露；
/// 生效镜像按**当前节点架构**过滤后以 `MODELINFER_IMAGES`（空格分隔）+ `MODELINFER_IMAGE_<i>`
/// 暴露，卡片据此选择实际镜像（见 `specs/docs/design/modelinfer/runtime-overrides.md`）。
pub fn env_exports(rt: &DeployRuntime, node: &DeployNode) -> Vec<(String, String)> {
    let mut vars = vec![
        ("MODELINFER_NODE_ROLE".to_string(), node.role.clone()),
        ("MODELINFER_NODE_RANK".to_string(), rt.rank_of(&node.id).to_string()),
        ("MODELINFER_NODE_COUNT".to_string(), rt.nodes.len().to_string()),
        ("MODELINFER_NODE_IP".to_string(), node.host.clone()),
        ("MODELINFER_NODE_SOCKET_IFNAME".to_string(), node.socket_ifname.clone()),
        ("MODELINFER_NODE_IB_HCA".to_string(), node.ib_hca.clone()),
        ("MODELINFER_HEAD_IP".to_string(), rt.head_ip.clone()),
        ("MODELINFER_NODES".to_string(), rt.nodes_map.clone()),
        ("MODELINFER_MODEL_ID".to_string(), rt.weight_id.clone()),
        ("MODELINFER_MODEL_PATH".to_string(), rt.model_path_for(node).to_string()),
        ("MODELINFER_CARD_WEIGHT_ID".to_string(), rt.card_weight_id.clone()),
        ("MODELINFER_WEIGHT_OVERRIDDEN".to_string(), rt.weight_overridden.to_string()),
        ("MODELINFER_IMAGES_SOURCE".to_string(), rt.images_source.clone()),
        (
            "MODELINFER_IMAGES_OVERRIDDEN".to_string(),
            (rt.images_source == "override").to_string(),
        ),
        ("MODELINFER_INSTANCE_ID".to_string(), rt.deployment_id.clone()),
        ("MODELINFER_MODEL_HUB_ROOT".to_string(), rt.model_hub_root_for(node).to_string()),
        ("MODELINFER_SERVE_PORT".to_string(), rt.serve_port.to_string()),
        ("MODELINFER_SERVE_URL_PREFIX".to_string(), rt.serve_url_prefix.clone()),
        ("MODELINFER_SERVE_API_KEY".to_string(), rt.serve_api_key.clone()),
        ("MODELINFER_SERVE_MODEL_NAME".to_string(), rt.serve_model_name.clone()),
    ];
    // 生效镜像（按节点架构过滤；非法条目在创建期已拦截，这里防御式回退空列表）
    let images = crate::images::images_for_arch(&rt.manifest_images, &node.arch)
        .unwrap_or_default();
    let names: Vec<String> = images.iter().map(|i| i.raw.clone()).collect();
    vars.push(("MODELINFER_IMAGES".to_string(), names.join(" ")));
    vars.push(("MODELINFER_IMAGE_COUNT".to_string(), names.len().to_string()));
    for (idx, name) in names.iter().enumerate() {
        vars.push((format!("MODELINFER_IMAGE_{idx}"), name.clone()));
    }
    vars.push((
        "MODELINFER_IMAGE_DEFAULT".to_string(),
        names.first().cloned().unwrap_or_default(),
    ));
    for (k, v) in &rt.configs {
        vars.push((k.clone(), env_value(v)));
    }
    // 额外环境变量在卡片 configs 之后追加（同名冲突已在创建 / 更新期拒绝）
    for (k, v) in &rt.extra_env {
        vars.push((k.clone(), env_value(v)));
    }
    vars
}

/// 生成 env.sh
pub fn build_env_script(rt: &DeployRuntime, node: &DeployNode) -> String {
    let mut out = String::from("#!/bin/bash\n# modelinfer 注入的环境变量\n");
    out.push_str(&format!("cd {}\n", sh_quote(&rt.workdir)));
    out.push_str(IB_HCA_LINK_UP_EXPORT);
    for (k, v) in env_exports(rt, node) {
        out.push_str(&format!("export {k}={}\n", sh_quote(&v)));
    }
    out
}

/// 生成阶段脚本（manifest `lifecycle.<stage>` 的 string / array 两种形式）
pub fn build_stage_script(script: &Value, env_script: &str) -> Result<String, ModelinferError> {
    let mut out = String::from("#!/bin/bash\n");
    out.push_str(&format!("source {}\n", sh_quote(env_script)));
    out.push_str("set -e\n");
    match script {
        Value::String(s) => {
            out.push_str(s);
            if !s.ends_with('\n') {
                out.push('\n');
            }
        }
        Value::Array(items) => {
            // exec 直接执行：第一个成员为命令，其余为参数
            let mut parts = Vec::with_capacity(items.len());
            for it in items {
                let Some(s) = it.as_str() else {
                    return Err(ModelinferError::new(
                        codes::MANIFEST_INVALID,
                        "lifecycle 数组形式成员必须为字符串",
                    ));
                };
                parts.push(sh_quote(s));
            }
            if parts.is_empty() {
                return Err(ModelinferError::new(codes::MANIFEST_INVALID, "lifecycle 数组形式为空"));
            }
            out.push_str(&format!("exec {}\n", parts.join(" ")));
        }
        _ => {
            return Err(ModelinferError::new(
                codes::MANIFEST_INVALID,
                "lifecycle 阶段值必须是字符串或字符串数组",
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::Path;
    use std::process::Command;

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
            hostname: "node-1".into(),
            socket_ifname: "enp1s0".into(),
            ib_hca: "mlx5_0".into(),
            arch: "amd64".into(),
            fscache: false,
            role: "head".into(),
        }
    }

    #[test]
    fn quote() {
        assert_eq!(sh_quote("a'b"), "'a'\\''b'");
    }

    fn runtime() -> DeployRuntime {
        let mut lifecycle = serde_json::Map::new();
        lifecycle.insert("start".into(), json!("echo hi"));
        DeployRuntime {
            deployment_id: "d".into(),
            name: "n".into(),
            nodes: vec![],
            head_ip: "10.0.0.1".into(),
            nodes_map: "node-1:10.0.0.1".into(),
            card_id: "c".into(),
            weight_id: "Qwen/Qwen3-0.6B".into(),
            card_weight_id: String::new(),
            weight_overridden: false,
            weight_source: "hf".into(),
            model_path: "/mnt/model_hub/public/modelhub/export/hf/Qwen/Qwen3-0.6B/current".into(),
            model_hub_root: "/mnt/model_hub/public/modelhub".into(),
            local_model_path: "/share/model_hub/export/hf/Qwen/Qwen3-0.6B/current".into(),
            local_model_hub_root: "/share/model_hub".into(),
            serve_port: 8000,
            serve_api_key: "key".into(),
            serve_model_name: "Qwen3-0.6B".into(),
            serve_url_prefix: "/v1".into(),
            configs: {
                let mut m = serde_json::Map::new();
                m.insert("MAX".into(), json!(123));
                m
            },
            extra_env: serde_json::Map::new(),
            lifecycle,
            lifecycle_order: serde_json::Map::new(),
            lifecycle_timeout: serde_json::Map::new(),
            workdir: "/opt/modelinfer/deployments/d".into(),
            mount_dir: "/mnt/model_hub".into(),
            nfs: crate::nfs::NfsResolveResult::not_mounted(),
            package_path: String::new(),
            manifest_images: Vec::new(),
            images_source: "card".into(),
        }
    }

    #[test]
    fn local_node_uses_host_model_path() {
        let rt = runtime();
        let mut local = node();
        local.auth_method = "local".into();
        local.host = "10.100.10.24".into();
        let env = build_env_script(&rt, &local);
        assert!(env.contains("export MODELINFER_MODEL_PATH='/share/model_hub/export/hf/Qwen/Qwen3-0.6B/current'"));
        assert!(env.contains("export MODELINFER_MODEL_HUB_ROOT='/share/model_hub'"));
    }

    #[test]
    fn env_script_contains_all_vars() {
        let rt = runtime();
        let env = build_env_script(&rt, &node());
        for key in [
            "MODELINFER_NODE_ROLE",
            "MODELINFER_NODE_RANK",
            "MODELINFER_NODE_COUNT",
            "MODELINFER_NODE_IP",
            "MODELINFER_NODE_SOCKET_IFNAME",
            "MODELINFER_NODE_IB_HCA",
            "MODELINFER_NODE_IB_HCA_LINK_UP",
            "MODELINFER_HEAD_IP",
            "MODELINFER_NODES",
            "MODELINFER_MODEL_ID",
            "MODELINFER_MODEL_PATH",
            "MODELINFER_CARD_WEIGHT_ID",
            "MODELINFER_WEIGHT_OVERRIDDEN",
            "MODELINFER_IMAGES",
            "MODELINFER_IMAGE_COUNT",
            "MODELINFER_IMAGE_DEFAULT",
            "MODELINFER_IMAGES_SOURCE",
            "MODELINFER_IMAGES_OVERRIDDEN",
            "MODELINFER_INSTANCE_ID",
            "MODELINFER_MODEL_HUB_ROOT",
            "MODELINFER_SERVE_PORT",
            "MODELINFER_SERVE_URL_PREFIX",
            "MODELINFER_SERVE_API_KEY",
            "MODELINFER_SERVE_MODEL_NAME",
            "MAX",
        ] {
            assert!(env.contains(&format!("export {key}=")), "missing {key}");
        }
        assert!(env.contains("cd '/opt/modelinfer/deployments/d'"));
        // 实例标识 = 部署 id；model-hub 根注入
        assert!(env.contains("export MODELINFER_INSTANCE_ID='d'"));
        assert!(env.contains("export MODELINFER_MODEL_HUB_ROOT='/mnt/model_hub/public/modelhub'"));
    }

    fn write_ib_state(root: &Path, device: &str, port: u32, state: &str) {
        let dir = root.join(device).join("ports").join(port.to_string());
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("phys_state"), state).unwrap();
    }

    /// 执行完整 env.sh，并在子进程读取变量，验证 export 与既有变量兼容。
    fn run_env_with_ib_root(root: &Path) -> String {
        let temp = tempfile::tempdir().unwrap();
        let mut rt = runtime();
        rt.workdir = temp.path().to_str().unwrap().to_string();
        let script = build_env_script(&rt, &node())
            .replace("/sys/class/infiniband", &sh_quote(root.to_str().unwrap()));
        let env_path = temp.path().join("env.sh");
        std::fs::write(&env_path, script).unwrap();
        let output = Command::new("bash")
            .args(["-euc", r#"
                source "$1"
                bash -euc 'printf "%s\n%s\n%s" "$MODELINFER_NODE_IB_HCA_LINK_UP" "$MODELINFER_NODE_IB_HCA" "$MAX"'
            "#, "env-test"])
            .arg(&env_path)
            .env("MODELINFER_NODE_IB_HCA_LINK_UP", "stale-device")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(output.stderr.is_empty(), "{}", String::from_utf8_lossy(&output.stderr));
        String::from_utf8(output.stdout).unwrap()
    }

    #[test]
    fn env_script_collects_link_up_devices_once_in_nccl_format() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("IB fabric 'sysfs'");
        // 逆序创建，输出仍按设备名排序；多端口设备只出现一次。
        write_ib_state(&root, "roceP2p1s0f1", 1, "5: LinkUp\n");
        write_ib_state(&root, "mlx5_2", 1, "5: LinkUp\n");
        write_ib_state(&root, "mlx5_11", 1, "5: LinkUp\n");
        write_ib_state(&root, "mlx5_10", 1, "2: Polling\n");
        write_ib_state(&root, "mlx5_1", 1, "2: Polling\n");
        write_ib_state(&root, "mlx5_1", 2, "5: LinkUp\n");
        write_ib_state(&root, "mlx5_0", 1, "5: LinkUp\n");
        write_ib_state(&root, "mlx5_0", 2, "5: LinkUp\n");
        assert_eq!(
            run_env_with_ib_root(&root),
            "=mlx5_0,mlx5_1,mlx5_11,mlx5_2,roceP2p1s0f1\nmlx5_0\n123",
        );
    }

    #[test]
    fn env_script_link_up_is_empty_without_available_devices() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("infiniband");
        // sysfs 缺失 / 空目录 / 全部非 LINK_UP 都必须覆盖继承的旧值为空串。
        assert_eq!(run_env_with_ib_root(&root), "\nmlx5_0\n123");
        std::fs::create_dir(&root).unwrap();
        assert_eq!(run_env_with_ib_root(&root), "\nmlx5_0\n123");
        for (port, state) in [
            "0: <unknown>\n", "1: Sleep\n", "2: Polling\n", "3: Disabled\n",
            "4: PortConfigurationTraining\n", "6: LinkErrorRecovery\n", "7: Phy Test\n",
            "", "malformed\n",
        ].iter().enumerate() {
            write_ib_state(&root, "mlx5_0", port as u32 + 1, state);
        }
        assert_eq!(run_env_with_ib_root(&root), "\nmlx5_0\n123");
    }

    #[test]
    fn env_script_link_up_skips_missing_or_unreadable_port_state() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        // 用目录模拟 read 失败，不依赖测试进程是否为 root。
        std::fs::create_dir_all(root.join("mlx5_0/ports/1/phys_state")).unwrap();
        std::fs::create_dir_all(root.join("mlx5_0/ports/2")).unwrap();
        write_ib_state(root, "mlx5_1", 1, "5: LinkUp\n");
        assert_eq!(run_env_with_ib_root(root), "=mlx5_1\nmlx5_0\n123");
    }

    #[test]
    fn env_script_refreshes_link_up_devices_on_each_execution() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        write_ib_state(root, "mlx5_0", 1, "5: LinkUp\n");
        write_ib_state(root, "mlx5_1", 1, "2: Polling\n");
        assert_eq!(run_env_with_ib_root(root), "=mlx5_0\nmlx5_0\n123");
        write_ib_state(root, "mlx5_0", 1, "2: Polling\n");
        write_ib_state(root, "mlx5_1", 1, "5: LinkUp\n");
        assert_eq!(run_env_with_ib_root(root), "=mlx5_1\nmlx5_0\n123");
    }

    #[test]
    fn env_script_rank_matches_node_order() {
        let mut rt = {
            let mut lifecycle = serde_json::Map::new();
            lifecycle.insert("start".into(), json!("echo hi"));
            DeployRuntime {
                deployment_id: "d".into(),
                name: "n".into(),
                nodes: vec![],
                head_ip: "10.0.0.1".into(),
                nodes_map: String::new(),
                card_id: "c".into(),
                weight_id: "w".into(),
                card_weight_id: String::new(),
                weight_overridden: false,
                weight_source: "hf".into(),
                model_path: "/data/models/w".into(),
                model_hub_root: "/data/models".into(),
                local_model_path: "/host/data/models/w".into(),
                local_model_hub_root: "/host/data/models".into(),
                serve_port: 8000,
                serve_api_key: "k".into(),
                serve_model_name: "w".into(),
                serve_url_prefix: "/v1".into(),
                configs: serde_json::Map::new(),
                extra_env: serde_json::Map::new(),
                lifecycle,
                lifecycle_order: serde_json::Map::new(),
                lifecycle_timeout: serde_json::Map::new(),
                workdir: "/w".into(),
                mount_dir: "/mnt/model_hub".into(),
                nfs: crate::nfs::NfsResolveResult::not_mounted(),
                package_path: String::new(),
                manifest_images: Vec::new(),
                images_source: "card".into(),
            }
        };
        let mk = |id: &str| DeployNode {
            id: id.into(),
            name: id.into(),
            host: format!("10.0.0.{id}"),
            port: 22,
            user: "root".into(),
            auth_method: "password".into(),
            password: "x".into(),
            ssh_key: String::new(),
            sudo_password: String::new(),
            hostname: id.into(),
            socket_ifname: "eth0".into(),
            ib_hca: String::new(),
            arch: "amd64".into(),
            fscache: false,
            role: "worker".into(),
        };
        rt.nodes = vec![mk("0"), mk("1"), mk("2")];
        for (idx, n) in rt.nodes.clone().iter().enumerate() {
            let env = build_env_script(&rt, n);
            assert!(env.contains(&format!("export MODELINFER_NODE_RANK='{idx}'")), "rank mismatch");
            assert!(env.contains("export MODELINFER_NODE_COUNT='3'"));
        }
    }

    #[test]
    fn stage_script_string_and_array() {
        let envp = "/w/.modelinfer/env.sh";
        let s = build_stage_script(&json!("set -e\necho hello"), envp).unwrap();
        assert!(s.contains("source '/w/.modelinfer/env.sh'"));
        assert!(s.contains("echo hello"));

        let a = build_stage_script(&json!(["python", "x.py", "--tp", "2"]), envp).unwrap();
        assert!(a.contains("exec 'python' 'x.py' '--tp' '2'"));

        assert!(build_stage_script(&json!(123), envp).is_err());
    }

    #[test]
    fn stage_file_sanitize() {
        assert_eq!(stage_file("health-probe"), "stage_health-probe.sh");
        assert_eq!(stage_file("a/b"), "stage_a_b.sh");
    }

    /// 生效镜像 / 覆盖标识 / 额外环境变量注入
    #[test]
    fn env_script_images_and_extra_env() {
        let mut lifecycle = serde_json::Map::new();
        lifecycle.insert("start".into(), json!("echo hi"));
        let mut extra_env = serde_json::Map::new();
        extra_env.insert("DEBUG_MODE".into(), json!("1"));
        extra_env.insert("QUOTED".into(), json!("a'b"));
        let rt = DeployRuntime {
            deployment_id: "d".into(),
            name: "n".into(),
            nodes: vec![],
            head_ip: "10.0.0.1".into(),
            nodes_map: String::new(),
            card_id: "c".into(),
            weight_id: "org/effective".into(),
            card_weight_id: "org/card-default".into(),
            weight_overridden: true,
            weight_source: "ms".into(),
            model_path: "/mnt/model_hub/export/ms/org/effective/current".into(),
            model_hub_root: "/mnt/model_hub".into(),
            local_model_path: "/share/model_hub/export/ms/org/effective/current".into(),
            local_model_hub_root: "/share/model_hub".into(),
            serve_port: 8000,
            serve_api_key: "k".into(),
            serve_model_name: "effective".into(),
            serve_url_prefix: "/v1".into(),
            configs: serde_json::Map::new(),
            extra_env,
            lifecycle,
            lifecycle_order: serde_json::Map::new(),
            lifecycle_timeout: serde_json::Map::new(),
            workdir: "/w".into(),
            mount_dir: "/mnt/model_hub".into(),
            nfs: crate::nfs::NfsResolveResult::not_mounted(),
            package_path: String::new(),
            manifest_images: vec![
                json!("repo/main:1"),
                json!({"name": "repo/arm-only:2", "arch": "arm64"}),
                json!("repo/aux:3"),
            ],
            images_source: "override".into(),
        };

        // amd64：arm64-only 条目被过滤，顺序稳定
        let mut amd = node();
        amd.arch = "amd64".into();
        let env = build_env_script(&rt, &amd);
        assert!(env.contains("export MODELINFER_IMAGES='repo/main:1 repo/aux:3'"), "{env}");
        assert!(env.contains("export MODELINFER_IMAGE_COUNT='2'"));
        assert!(env.contains("export MODELINFER_IMAGE_0='repo/main:1'"));
        assert!(env.contains("export MODELINFER_IMAGE_1='repo/aux:3'"));
        assert!(env.contains("export MODELINFER_IMAGE_DEFAULT='repo/main:1'"));
        assert!(env.contains("export MODELINFER_IMAGES_SOURCE='override'"));
        assert!(env.contains("export MODELINFER_IMAGES_OVERRIDDEN='true'"));
        assert!(env.contains("export MODELINFER_WEIGHT_OVERRIDDEN='true'"));
        assert!(env.contains("export MODELINFER_CARD_WEIGHT_ID='org/card-default'"));
        assert!(env.contains("export MODELINFER_MODEL_ID='org/effective'"));
        // 额外环境变量（含单引号转义）
        assert!(env.contains("export DEBUG_MODE='1'"));
        assert!(env.contains("export QUOTED='a'\\''b'"), "{env}");

        // arm64：三个镜像全部参与
        let mut arm = node();
        arm.arch = "aarch64".into();
        let env = build_env_script(&rt, &arm);
        assert!(env.contains("export MODELINFER_IMAGE_COUNT='3'"), "{env}");
        assert!(env.contains("export MODELINFER_IMAGE_1='repo/arm-only:2'"));
    }
}
