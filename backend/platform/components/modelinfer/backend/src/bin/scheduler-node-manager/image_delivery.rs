use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};

use modelinfer::{
    error::{ModelinferError, codes},
    ssh::{RealSshExec, SshExec, SshTarget},
};
use serde_json::{Value, json};

const PULL_TIMEOUT: Duration = Duration::from_secs(150);
const RELAY_TIMEOUT: Duration = Duration::from_secs(1800);

fn failure() -> ModelinferError {
    ModelinferError::new(codes::SSH_COMMAND_FAILED, "backend_image_pull_failed")
}

fn target_arch(arch: &str) -> Option<&'static str> {
    match arch {
        "aarch64" | "arm64" => Some("arm64"),
        "x86_64" | "amd64" => Some("amd64"),
        _ => None,
    }
}

fn registry_auth(document: &Value, repository: &str) -> Option<Vec<u8>> {
    let registry = repository.split_once('/')?.0;
    let credentials = document["auths"].as_object()?;
    let (_, entry) = credentials
        .iter()
        .filter(|(key, entry)| {
            let key = key.trim_start_matches("https://").trim_end_matches('/');
            (key == repository
                || repository
                    .strip_prefix(key)
                    .is_some_and(|rest| rest.starts_with('/')))
                && entry["auth"].as_str().is_some_and(|auth| !auth.is_empty())
        })
        .max_by_key(|(key, _)| key.len())?;
    serde_json::to_vec(&json!({"auths":{(registry):{"auth":entry["auth"]}}})).ok()
}

fn image_hub_directory() -> PathBuf {
    PathBuf::from("/var/lib/image-hub")
}

fn read_authfile(path: &Path) -> Option<Value> {
    let info = std::fs::symlink_metadata(path).ok()?;
    if !info.is_file() || info.len() > 256 * 1024 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub async fn prepare(
    ssh: &RealSshExec,
    target: &SshTarget,
    image: &str,
    repository: &str,
    hardware_arch: &str,
) -> Result<(), ModelinferError> {
    let arch = if let Some(arch) = target_arch(hardware_arch) {
        arch
    } else {
        let actual = ssh.run(target, "uname -m").await?;
        target_arch(actual.stdout.trim()).ok_or_else(failure)?
    };
    // image 有上游校验的 registry/repository/tag 字符集，arch 来自固定映射。
    let inspect = format!(
        "test \"$(docker image inspect --format '{{{{.Os}}}}/{{{{.Architecture}}}}' '{image}' 2>/dev/null)\" = 'linux/{arch}'"
    );
    if ssh.run(target, &inspect).await?.success() {
        return Ok(());
    }
    let hub = image_hub_directory();
    let authfile = hub.join("auth.json");
    let auth = read_authfile(&authfile);
    if let Some(credentials) = auth.as_ref().and_then(|doc| registry_auth(doc, repository)) {
        let pull = format!(
            "umask 077; cfg=$(mktemp -d) || exit 1; trap 'rm -rf \"$cfg\"' EXIT; \
             cat >\"$cfg/config.json\" && timeout 140s docker --config \"$cfg\" pull --platform 'linux/{arch}' '{image}' >/dev/null && {inspect}"
        );
        if ssh
            .run_with_docker_input(
                target,
                &pull,
                std::io::Cursor::new(credentials),
                PULL_TIMEOUT,
            )
            .await
            .is_ok_and(|result| result.success())
        {
            return Ok(());
        }
    }
    let pull = format!(
        "timeout 140s docker pull --platform 'linux/{arch}' '{image}' >/dev/null && {inspect}"
    );
    if ssh
        .run_with_timeout(target, &pull, PULL_TIMEOUT)
        .await
        .is_ok_and(|result| result.success())
    {
        return Ok(());
    }

    // docker-archive 以 tag 恢复引用。摘要引用仍由节点直接拉取。
    if image.contains('@') {
        return Err(failure());
    }
    let staging = tempfile::Builder::new()
        .prefix("backend-relay-")
        .tempdir_in(&hub)
        .map_err(|_| failure())?;
    let archive = staging.path().join("image.tar");
    let mut copy = tokio::process::Command::new("skopeo");
    copy.args(["--override-os", "linux", "--override-arch", arch, "copy"]);
    if auth.is_some() {
        copy.arg("--src-authfile").arg(&authfile);
    }
    copy.arg(format!("docker://{image}"))
        .arg(format!("docker-archive:{}:{image}", archive.display()))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    let downloaded = tokio::time::timeout(RELAY_TIMEOUT, copy.status())
        .await
        .is_ok_and(|result| result.is_ok_and(|status| status.success()));
    if !downloaded {
        return Err(failure());
    }
    let input = std::fs::File::open(&archive).map_err(|_| failure())?;
    let load = format!("docker image load >/dev/null && {inspect}");
    if ssh
        .run_with_docker_input(target, &load, input, RELAY_TIMEOUT)
        .await?
        .success()
    {
        Ok(())
    } else {
        Err(failure())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_platform_comes_from_target_hardware() {
        assert_eq!(target_arch("aarch64"), Some("arm64"));
        assert_eq!(target_arch("x86_64"), Some("amd64"));
        assert_eq!(target_arch("unknown"), None);
    }

    #[test]
    fn sends_only_matching_registry_credentials_and_prefers_repository_scope() {
        let doc = json!({"auths":{
            "registry.example": {"auth": "registry"},
            "registry.example/group": {"auth": "scoped"},
            "registry.example/group-other": {"auth": "other-group"},
            "other.example": {"auth": "other-registry"}
        }});
        let auth = registry_auth(&doc, "registry.example/group/voice").unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&auth).unwrap(),
            json!({"auths":{"registry.example":{"auth":"scoped"}}})
        );
        assert!(registry_auth(&doc, "unconfigured.example/group/voice").is_none());
    }
}
