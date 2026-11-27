//! 节点命令执行抽象：SSH 密码/私钥登录、sudo root、本机 nsenter 与测试用 Mock。
//!
//! 参考 `components/ai-ray-cluster` 的 paramiko 用法：连接失败重试、命令超时、
//! 异常分层。此处以 trait 注入，便于领域逻辑单测。

use std::io::{Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::UnboundedSender;

use crate::error::{ModelinferError, codes};

/// 流式输出发送端（分块）
pub type StreamSink = UnboundedSender<Vec<u8>>;

/// SSH 认证方式（凭据只入不显，见 `SshTarget` 手写 `Debug`）
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SshAuth {
    /// SSH 登录密码
    Password(String),
    /// SSH 私钥内容（OpenSSH / PEM），经内存认证（不落盘）
    PrivateKey(String),
    /// 本机节点：经 nsenter 进入宿主机 root 命名空间执行（无凭据）
    Local,
}

/// 手写 `Debug`：只输出认证类型，**绝不输出凭据明文**
impl std::fmt::Debug for SshAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SshAuth({})", self.kind())
    }
}

impl SshAuth {
    /// 由认证方式字面值 + 凭据构造（未知 / 空 → 视为密码）
    pub fn from_parts(auth_method: &str, password: &str, key: &str) -> Self {
        if auth_method.trim() == "key" {
            Self::PrivateKey(key.to_string())
        } else {
            Self::Password(password.to_string())
        }
    }

    /// 认证方式字面值（`password` / `key` / `local`）
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Password(_) => "password",
            Self::PrivateKey(_) => "key",
            Self::Local => "local",
        }
    }

    /// 凭据是否已提供（非空）
    pub fn is_configured(&self) -> bool {
        match self {
            Self::Password(p) => !p.is_empty(),
            Self::PrivateKey(k) => !k.trim().is_empty(),
            Self::Local => true,
        }
    }
}

/// SSH 目标
#[derive(Clone)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    /// 独立 sudo 密码；空值时密码认证复用 SSH 密码，密钥认证要求免密 sudo。
    pub sudo_password: String,
}

/// 手写 `Debug`：只输出连接信息与认证类型，**绝不输出凭据明文**（防日志泄漏）
impl std::fmt::Debug for SshTarget {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SshTarget")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("user", &self.user)
            .field("auth", &self.auth.kind())
            .finish()
    }
}

impl SshTarget {
    /// 密码认证（兼容既有调用点）
    pub fn new(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        password: impl Into<String>,
    ) -> Self {
        Self::with_auth(host, port, user, SshAuth::Password(password.into()))
    }

    /// 显式认证方式
    pub fn with_auth(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        auth: SshAuth,
    ) -> Self {
        Self {
            host: host.into(),
            port,
            user: user.into(),
            auth,
            sudo_password: String::new(),
        }
    }

    /// 本机执行目标（host 仅用于展示 / 节点地址，不参与连接）
    pub fn local(host: impl Into<String>) -> Self {
        Self::with_auth(host, 0, "root", SshAuth::Local)
    }

    /// 是否为本机 nsenter 执行目标
    pub fn is_local(&self) -> bool {
        matches!(self.auth, SshAuth::Local)
    }

    /// 私钥认证
    pub fn private_key(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self::with_auth(host, port, user, SshAuth::PrivateKey(key.into()))
    }

    pub fn with_sudo_password(mut self, password: impl Into<String>) -> Self {
        self.sudo_password = password.into();
        self
    }

    fn elevation_password(&self) -> Option<&str> {
        if self.user == "root" {
            None
        } else if !self.sudo_password.is_empty() {
            Some(&self.sudo_password)
        } else if let SshAuth::Password(password) = &self.auth {
            Some(password)
        } else {
            None
        }
    }

    /// 由「认证方式 + 密码 + 私钥」三个字段构造（供节点记录 / 部署节点使用）
    pub fn from_credentials(
        host: impl Into<String>,
        port: u16,
        user: impl Into<String>,
        auth_method: &str,
        password: &str,
        key: &str,
    ) -> Self {
        Self::with_auth(host, port, user, SshAuth::from_parts(auth_method, password, key))
    }
}

/// 命令执行结果
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SshOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

impl SshOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }
}

/// SSH 执行器抽象
#[async_trait]
pub trait SshExec: Send + Sync {
    /// 执行命令；SSH 会话建立/认证失败返回 20704；命令非零退出仍在 Ok(output) 中返回
    ///
    /// 使用实现内置的缺省命令超时（生产为 90s）。长时间阻塞的**生命周期阶段脚本**
    /// （如 head 节点等待服务就绪的 `start`）MUST 改用 [`SshExec::run_with_timeout`]。
    async fn run(&self, target: &SshTarget, cmd: &str) -> Result<SshOutput, ModelinferError>;

    /// 带显式超时的命令执行：用于生命周期阶段脚本。
    ///
    /// 缺省实现忽略 `_timeout` 并委托 [`SshExec::run`]，测试替身无需重复实现；
    /// 生产实现（`RealSshExec`）以该超时同时约束**外层等待**与 **SSH 会话读超时**，
    /// 连接建立超时保持不变（10s）。
    async fn run_with_timeout(
        &self,
        target: &SshTarget,
        cmd: &str,
        _timeout: Duration,
    ) -> Result<SshOutput, ModelinferError> {
        self.run(target, cmd).await
    }

    /// Send private input over the authenticated SSH session to an unprivileged command.
    async fn run_with_input(
        &self,
        _target: &SshTarget,
        _cmd: &str,
        _input: Vec<u8>,
        _timeout: Duration,
    ) -> Result<SshOutput, ModelinferError> {
        Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "SSH stdin is unavailable"))
    }

    /// 连通性与 root 执行权限测试；非零退出不可视为成功。
    async fn test(&self, target: &SshTarget) -> Result<(), ModelinferError> {
        let out = self.run(target, ROOT_CHECK).await?;
        if out.success() {
            Ok(())
        } else if target.user != "root" {
            Err(sudo_error())
        } else {
            Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "无法以 root 权限执行命令"))
        }
    }

    /// 上传本地文件到远端路径（SCP）；用于分发 model.tar.gz 与生成的脚本
    async fn upload(
        &self,
        target: &SshTarget,
        local: &Path,
        remote: &str,
    ) -> Result<(), ModelinferError>;

    /// **流式执行**：把标准输出分块推送到 `sink`（用于 `docker logs -f` 等跟随命令）。
    ///
    /// 缺省实现退化为一次性执行（`stdout` 作为单块发送），便于仅实现 `run` 的测试替身；
    /// 生产实现（`RealSshExec`）为真正的非阻塞流式读取。接收端（`sink`）被丢弃时，
    /// 实现 MUST 尽快结束并释放 SSH 会话。
    async fn stream(
        &self,
        target: &SshTarget,
        cmd: &str,
        sink: StreamSink,
        idle_timeout: Duration,
    ) -> Result<(), ModelinferError> {
        let out = self.run_with_timeout(target, cmd, idle_timeout).await?;
        let _ = sink.send(out.stdout.into_bytes());
        Ok(())
    }
}

/// 生产实现：`ssh2` + `spawn_blocking`
pub struct RealSshExec {
    connect_timeout: Duration,
    command_timeout: Duration,
    require_root: bool,
}

impl Default for RealSshExec {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            command_timeout: Duration::from_secs(90),
            require_root: true,
        }
    }
}

impl RealSshExec {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn node_manager() -> Self {
        Self { require_root: false, ..Self::default() }
    }

    fn elevate(&self, target: &SshTarget) -> bool {
        self.require_root || !target.sudo_password.is_empty()
    }

    /// 把文件流式送入已登记节点的命令，避免把多 GB 镜像归档读入内存。
    pub async fn run_with_docker_input(
        &self,
        target: &SshTarget,
        cmd: &str,
        mut input: impl Read + Send + 'static,
        timeout: Duration,
    ) -> Result<SshOutput, ModelinferError> {
        let elevate = self.elevate(target);
        let target = target.clone();
        let cmd = cmd.to_owned();
        let connect_timeout = self.connect_timeout;
        let operation = tokio::task::spawn_blocking(move || {
            let session = connect_session(&target, connect_timeout, timeout)?;
            let mut channel = if elevate {
                open_root_channel_mode(&session, &target, &cmd, true)?
            } else {
                open_channel(&session, &format!("/bin/sh -c {}", crate::deploy::script::sh_quote(&cmd)))?
            };
            std::io::copy(&mut input, &mut channel).map_err(|_| {
                ModelinferError::new(codes::SSH_COMMAND_FAILED, "Unable to stream SSH stdin")
            })?;
            channel.send_eof().map_err(|_| {
                ModelinferError::new(codes::SSH_COMMAND_FAILED, "Unable to close SSH stdin")
            })?;
            collect_output(&session, channel, timeout)
        });
        match tokio::time::timeout(timeout, operation).await {
            Ok(Ok(result)) => result,
            _ => Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "SSH file input operation failed")),
        }
    }
}

#[async_trait]
impl SshExec for RealSshExec {
    async fn test(&self, target: &SshTarget) -> Result<(), ModelinferError> {
        let elevated = self.elevate(target);
        if self.run(target, if elevated { ROOT_CHECK } else { "true" }).await?.success() {
            Ok(())
        } else if elevated && target.user != "root" {
            Err(sudo_error())
        } else {
            Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "SSH command check failed"))
        }
    }

    async fn run(&self, target: &SshTarget, cmd: &str) -> Result<SshOutput, ModelinferError> {
        if target.is_local() {
            return run_local(cmd, self.command_timeout).await;
        }
        let elevate = self.elevate(target);
        let target = target.clone();
        let cmd = cmd.to_string();
        let connect_timeout = self.connect_timeout;
        let command_timeout = self.command_timeout;

        let join = tokio::task::spawn_blocking(move || {
            run_blocking(&target, &cmd, connect_timeout, command_timeout, elevate)
        });

        match tokio::time::timeout(self.command_timeout, join).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                format!("SSH 任务执行异常: {e}"),
            )),
            Err(_) => Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                "SSH 命令执行超时",
            )),
        }
    }

    /// 带显式超时的命令执行：仅放宽命令 / 读超时，连接超时保持 10s
    async fn run_with_timeout(
        &self,
        target: &SshTarget,
        cmd: &str,
        timeout: Duration,
    ) -> Result<SshOutput, ModelinferError> {
        if target.is_local() {
            return run_local(cmd, timeout).await;
        }
        let elevate = self.elevate(target);
        let target = target.clone();
        let cmd = cmd.to_string();
        let connect_timeout = self.connect_timeout;

        let join = tokio::task::spawn_blocking(move || {
            run_blocking(&target, &cmd, connect_timeout, timeout, elevate)
        });

        match tokio::time::timeout(timeout, join).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                format!("SSH 任务执行异常: {e}"),
            )),
            Err(_) => Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                format!("SSH 命令执行超时（{}s）", timeout.as_secs()),
            )),
        }
    }

    async fn run_with_input(
        &self,
        target: &SshTarget,
        cmd: &str,
        input: Vec<u8>,
        timeout: Duration,
    ) -> Result<SshOutput, ModelinferError> {
        let target = target.clone();
        let cmd = format!("/bin/sh -c {}", crate::deploy::script::sh_quote(cmd));
        let connect_timeout = self.connect_timeout;
        let operation = tokio::task::spawn_blocking(move || {
            let session = connect_session(&target, connect_timeout, timeout)?;
            let mut channel = open_channel(&session, &cmd)?;
            channel.write_all(&input).map_err(|_| {
                ModelinferError::new(codes::SSH_COMMAND_FAILED, "Unable to write SSH stdin")
            })?;
            channel.send_eof().map_err(|_| {
                ModelinferError::new(codes::SSH_COMMAND_FAILED, "Unable to close SSH stdin")
            })?;
            collect_output(&session, channel, timeout)
        });
        match tokio::time::timeout(timeout, operation).await {
            Ok(Ok(result)) => result,
            _ => Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "SSH input operation failed")),
        }
    }

    async fn upload(
        &self,
        target: &SshTarget,
        local: &Path,
        remote: &str,
    ) -> Result<(), ModelinferError> {
        if target.is_local() {
            return upload_local(local, remote).await;
        }
        let target = target.clone();
        let local = local.to_path_buf();
        let remote = remote.to_string();
        let connect_timeout = self.connect_timeout;
        let command_timeout = self.command_timeout;
        let join = tokio::task::spawn_blocking(move || {
            upload_blocking(&target, &local, &remote, connect_timeout, command_timeout)
        });
        match tokio::time::timeout(self.command_timeout, join).await {
            Ok(Ok(result)) => result,
            Ok(Err(e)) => Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                format!("SCP 任务执行异常: {e}"),
            )),
            Err(_) => Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                "SCP 上传超时",
            )),
        }
    }

    /// 真正的流式执行：非阻塞通道循环读取，接收端关闭即退出
    async fn stream(
        &self,
        target: &SshTarget,
        cmd: &str,
        sink: StreamSink,
        idle_timeout: Duration,
    ) -> Result<(), ModelinferError> {
        if target.is_local() {
            return stream_local(cmd, sink, idle_timeout).await;
        }
        let target = target.clone();
        let cmd = cmd.to_string();
        let connect_timeout = self.connect_timeout;
        // 不设整体墙钟超时（跟随日志可无限长）；仅把读超时用于会话构造。
        let join = tokio::task::spawn_blocking(move || {
            stream_blocking(&target, &cmd, connect_timeout, idle_timeout, sink)
        });
        match join.await {
            Ok(result) => result,
            Err(e) => Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                format!("SSH 流式任务执行异常: {e}"),
            )),
        }
    }
}

/// 本机执行：进入宿主机 mount/uts/ipc/net/pid 命名空间，以宿主 root 运行命令。
const NSENTER_ARGS: [&str; 8] = ["-t", "1", "-m", "-u", "-i", "-n", "-p", "--"];

fn local_command(cmd: &str) -> Result<tokio::process::Command, ModelinferError> {
    if let Ok(enabled) = std::env::var("SCHEDULER_MODELINFER_HOST_ACCESS") {
        if enabled != "true" || std::env::var("SCHEDULER_MODELINFER_PID_MODE").as_deref() != Ok("host") {
            return Err(local_exec_error("在 VOS 应用配置中开启 ModelInfer 本机免密访问，并将 PID 模式设为 host"));
        }
    }
    let mut c = tokio::process::Command::new("nsenter");
    c.args(NSENTER_ARGS)
        .arg("/usr/bin/env")
        .args(["-i", "PATH=/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin", "HOME=/root"])
        .arg("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());
    Ok(c)
}

fn local_exec_error(detail: impl Into<String>) -> ModelinferError {
    ModelinferError::new(
        codes::LOCAL_EXEC_FAILED,
        format!(
            "本机节点执行失败: {}。请确认 modelinfer 容器以 pid: host 和 privileged: true 运行，且宿主机提供 nsenter",
            detail.into()
        ),
    )
}

async fn run_local(cmd: &str, timeout: Duration) -> Result<SshOutput, ModelinferError> {
    let child = local_command(cmd)?
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| local_exec_error(format!("启动 nsenter 失败: {e}")))?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| local_exec_error(format!("命令执行超时（{}s）", timeout.as_secs())))?
        .map_err(|e| local_exec_error(format!("等待命令结束失败: {e}")))?;
    Ok(SshOutput {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

async fn upload_local(local: &Path, remote: &str) -> Result<(), ModelinferError> {
    let bytes = std::fs::read(local)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("读取本地文件失败 {}: {e}", local.display())))?;
    let parent = Path::new(remote)
        .parent()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "/".to_string());
    let cmd = format!(
        "set -e; mkdir -p -- {parent}; cat > {remote}; chmod 0644 -- {remote}",
        parent = crate::deploy::script::sh_quote(&parent),
        remote = crate::deploy::script::sh_quote(remote)
    );
    let mut child = local_command(&cmd)?
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| local_exec_error(format!("启动 nsenter 上传失败: {e}")))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&bytes)
            .await
            .map_err(|e| local_exec_error(format!("写入宿主机文件失败: {e}")))?;
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| local_exec_error(format!("等待上传结束失败: {e}")))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(local_exec_error(format!(
            "上传退出码 {}: {}",
            output.status.code().unwrap_or(-1),
            String::from_utf8_lossy(&output.stderr).trim()
        )))
    }
}

async fn stream_local(
    cmd: &str,
    sink: StreamSink,
    idle_timeout: Duration,
) -> Result<(), ModelinferError> {
    let mut child = local_command(cmd)?
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| local_exec_error(format!("启动 nsenter 流式命令失败: {e}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| local_exec_error("无法获取 stdout"))?;
    let mut stderr = child
        .stderr
        .take()
        .ok_or_else(|| local_exec_error("无法获取 stderr"))?;
    let stderr_drain = tokio::spawn(async move {
        let mut sink_bytes = Vec::new();
        let _ = stderr.read_to_end(&mut sink_bytes).await;
        sink_bytes
    });
    let mut buf = [0u8; 8192];
    loop {
        if sink.is_closed() {
            let _ = child.kill().await;
            stderr_drain.abort();
            return Ok(());
        }
        match tokio::time::timeout(idle_timeout, stdout.read(&mut buf)).await {
            Ok(Ok(0)) => break,
            Ok(Ok(n)) => {
                if sink.send(buf[..n].to_vec()).is_err() {
                    let _ = child.kill().await;
                    stderr_drain.abort();
                    return Ok(());
                }
            }
            Ok(Err(e)) => {
                let _ = child.kill().await;
                stderr_drain.abort();
                return Err(local_exec_error(format!("读取 stdout 失败: {e}")));
            }
            Err(_) => {
                let _ = child.kill().await;
                stderr_drain.abort();
                return Err(local_exec_error(format!(
                    "日志流空闲超时（{}s）",
                    idle_timeout.as_secs()
                )));
            }
        }
    }
    let status = child
        .wait()
        .await
        .map_err(|e| local_exec_error(format!("等待流式命令结束失败: {e}")))?;
    if !status.success() {
        let err = stderr_drain.await.unwrap_or_default();
        return Err(local_exec_error(format!(
            "流式命令退出码 {}: {}",
            status.code().unwrap_or(-1),
            String::from_utf8_lossy(&err).trim()
        )));
    }
    stderr_drain.abort();
    Ok(())
}

/// 阻塞式流式读取（非阻塞通道 + 轮询；接收端关闭即退出）
fn stream_blocking(
    target: &SshTarget,
    cmd: &str,
    connect_timeout: Duration,
    idle_timeout: Duration,
    sink: StreamSink,
) -> Result<(), ModelinferError> {
    let sess = connect_session(target, connect_timeout, idle_timeout)?;
    let mut channel = open_root_channel(&sess, target, cmd)?;
    // 非阻塞 + 轮询：既能及时响应接收端关闭，也避免阻塞在无数据的读上
    sess.set_blocking(false);
    let mut buf = [0u8; 8192];
    loop {
        if sink.is_closed() {
            // 跟随命令可能不会自行结束，不能等待远端关闭。
            let _ = channel.close();
            return Ok(());
        }
        // 日志流仅转发 stdout，但仍需排空 stderr 以免堵住 SSH 窗口。
        match channel.stderr().read(&mut buf) {
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("读取 SSH 日志失败: {e}"))),
        }
        match channel.read(&mut buf) {
            Ok(0) if channel.eof() => break,
            Ok(0) => std::thread::sleep(Duration::from_millis(150)),
            Ok(n) => {
                if sink.send(buf[..n].to_vec()).is_err() {
                    let _ = channel.close();
                    return Ok(());
                }
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(150));
            }
            Err(e) => return Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("读取 SSH 日志失败: {e}"))),
        }
    }
    sess.set_blocking(true);
    let _ = channel.wait_close();
    Ok(())
}

fn run_blocking(
    target: &SshTarget,
    cmd: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
    elevate: bool,
) -> Result<SshOutput, ModelinferError> {
    let sess = connect_session(target, connect_timeout, read_timeout)?;

    let channel = if elevate {
        open_root_channel(&sess, target, cmd)?
    } else {
        open_channel(&sess, &format!("/bin/sh -c {}", crate::deploy::script::sh_quote(cmd)))?
    };
    collect_output(&sess, channel, read_timeout)
}

pub(crate) const ROOT_CHECK: &str = "test \"$(id -u)\" -eq 0";
const ROOT_READY: &[u8] = b"MODELINFER_ROOT_READY\n";

fn sudo_error() -> ModelinferError {
    ModelinferError::new(
        codes::NODE_SUDO_FAILED,
        "无法通过 sudo 以 root 执行命令：请确认账号具有非交互 sudo 权限，并提供正确的 sudo 密码或配置免密 sudo",
    )
}

/// 整段命令在 root shell 中执行，覆盖管道、重定向和环境变量。
/// 密码通过通道 stdin 单独发送，绝不进入 shell 命令参数。
#[cfg(test)]
fn root_command(target: &SshTarget, cmd: &str) -> String {
    root_command_mode(target, cmd, false)
}

fn root_command_mode(target: &SshTarget, cmd: &str, keep_stdin: bool) -> String {
    if target.user == "root" {
        return cmd.to_string();
    }
    let auth = if target.elevation_password().is_some() { "-S -p ''" } else { "-n" };
    let reset = if keep_stdin { "" } else { "exec </dev/null; " };
    let body = format!("{ROOT_CHECK} || exit 1; printf 'MODELINFER_ROOT_READY\\n'; {reset}{cmd}");
    format!("sudo -k -H {auth} -u root -- /bin/sh -c {}", crate::deploy::script::sh_quote(&body))
}

fn open_channel(sess: &ssh2::Session, cmd: &str) -> Result<ssh2::Channel, ModelinferError> {
    let mut channel = sess.channel_session()
        .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("打开 SSH 通道失败: {e}")))?;
    channel.exec(cmd)
        .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("执行命令失败: {e}")))?;
    Ok(channel)
}

fn open_root_channel(
    sess: &ssh2::Session,
    target: &SshTarget,
    cmd: &str,
) -> Result<ssh2::Channel, ModelinferError> {
    open_root_channel_mode(sess, target, cmd, false)
}

fn open_root_channel_mode(
    sess: &ssh2::Session,
    target: &SshTarget,
    cmd: &str,
    keep_stdin: bool,
) -> Result<ssh2::Channel, ModelinferError> {
    if target.elevation_password().is_some_and(|p| p.contains(['\r', '\n', '\0'])) {
        return Err(sudo_error());
    }
    let mut channel = open_channel(sess, &root_command_mode(target, cmd, keep_stdin))?;
    if let Some(password) = target.elevation_password() {
        channel.write_all(password.as_bytes()).map_err(|_| sudo_error())?;
        channel.write_all(b"\n").map_err(|_| sudo_error())?;
    }
    if !keep_stdin {
        channel.send_eof()
            .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("关闭 SSH 输入失败: {e}")))?;
    }
    if target.user != "root" {
        // 握手标记由成功提权后的 shell 输出，不让 sudo 失败混入业务退出码或日志流。
        let mut ready = vec![0; ROOT_READY.len()];
        channel.read_exact(&mut ready).map_err(|_| sudo_error())?;
        if ready != ROOT_READY {
            return Err(sudo_error());
        }
    }
    Ok(channel)
}

/// 同时排空 stdout/stderr，避免远端 stderr 填满窗口时 stdout 读取死锁。
fn collect_output(
    sess: &ssh2::Session,
    mut channel: ssh2::Channel,
    timeout: Duration,
) -> Result<SshOutput, ModelinferError> {
    sess.set_blocking(false);
    let started = std::time::Instant::now();
    let mut stdout = Vec::new();
    let mut stderr = Vec::new();
    let result = (|| {
        loop {
            let mut read_any = false;
            for (mut stream, output) in [(channel.stream(0), &mut stdout), (channel.stderr(), &mut stderr)] {
                let mut buf = [0u8; 8192];
                match stream.read(&mut buf) {
                    Ok(n) => { output.extend_from_slice(&buf[..n]); read_any |= n > 0; }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => return Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("读取 SSH 输出失败: {e}"))),
                }
            }
            if !read_any && channel.eof() { break; }
            if started.elapsed() >= timeout {
                return Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "SSH 命令执行超时"));
            }
            if !read_any { std::thread::sleep(Duration::from_millis(10)); }
        }
        Ok(())
    })();
    sess.set_blocking(true);
    result?;
    channel.wait_close()
        .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("等待 SSH 命令结束失败: {e}")))?;
    let exit_code = channel.exit_status()
        .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("读取 SSH 退出码失败: {e}")))?;
    Ok(SshOutput {
        exit_code,
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
    })
}

/// 建立并认证 SSH 会话
fn connect_session(
    target: &SshTarget,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<ssh2::Session, ModelinferError> {
    let addr = resolve(target)?;
    let tcp = TcpStream::connect_timeout(&addr, connect_timeout).map_err(|e| {
        ModelinferError::new(codes::SSH_CONNECT_FAILED, format!("连接 {}:{} 失败: {e}", target.host, target.port))
    })?;

    let mut sess = ssh2::Session::new()
        .map_err(|e| ModelinferError::new(codes::SSH_CONNECT_FAILED, format!("初始化 SSH 会话失败: {e}")))?;
    sess.set_tcp_stream(tcp);
    sess.set_timeout(read_timeout.as_millis() as u32);
    sess.handshake().map_err(|e| {
        ModelinferError::new(codes::SSH_CONNECT_FAILED, format!("SSH 握手失败: {e}"))
    })?;
    match &target.auth {
        SshAuth::Password(password) => {
            sess.userauth_password(&target.user, password).map_err(|e| {
                ModelinferError::new(codes::SSH_CONNECT_FAILED, format!("SSH 认证失败（密码）: {e}"))
            })?;
        }
        SshAuth::PrivateKey(key) => {
            sess.userauth_pubkey_memory(&target.user, None, key, None)
                .map_err(|e| {
                    ModelinferError::new(codes::SSH_CONNECT_FAILED, format!("SSH 认证失败（私钥）: {e}"))
                })?;
        }
        SshAuth::Local => {
            return Err(local_exec_error("本机目标不应建立 SSH 会话"));
        }
    }
    if !sess.authenticated() {
        return Err(ModelinferError::new(codes::SSH_CONNECT_FAILED, "SSH 认证未通过"));
    }
    Ok(sess)
}

/// SCP 上传（阻塞）
fn upload_blocking(
    target: &SshTarget,
    local: &Path,
    remote: &str,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<(), ModelinferError> {
    let sess = connect_session(target, connect_timeout, read_timeout)?;
    let data = std::fs::read(local)
        .map_err(|e| ModelinferError::new(codes::IO_FAILED, format!("读取本地文件失败 {}: {e}", local.display())))?;
    if target.user == "root" {
        return scp_write(&sess, &data, remote, 0o644);
    }
    // 在传输任何文件前验证 sudo 权限。
    let channel = open_root_channel(&sess, target, "true")?;
    collect_output(&sess, channel, read_timeout)?;
    let mut channel = open_channel(&sess, "mktemp -d /tmp/modelinfer-upload.XXXXXXXXXX")?;
    channel.send_eof().map_err(|_| ModelinferError::new(codes::SSH_COMMAND_FAILED, "关闭上传准备通道失败"))?;
    let out = collect_output(&sess, channel, read_timeout)?;
    let staging = out.stdout.trim();
    let suffix = staging.strip_prefix("/tmp/modelinfer-upload.").unwrap_or("");
    if !out.success() || suffix.is_empty() || !suffix.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "创建上传临时目录失败"));
    }
    let result = (|| {
        let staged_file = format!("{staging}/payload");
        scp_write(&sess, &data, &staged_file, 0o600)?;
        let cmd = format!("install -m 0644 -- {} {}", crate::deploy::script::sh_quote(&staged_file), crate::deploy::script::sh_quote(remote));
        let channel = open_root_channel(&sess, target, &cmd)?;
        let out = collect_output(&sess, channel, read_timeout)?;
        if !out.success() {
            return Err(ModelinferError::new(codes::SSH_COMMAND_FAILED, "以 root 写入上传文件失败"));
        }
        Ok(())
    })();
    // 临时目录归 SSH 用户所有；即使 sudo 失败，也能以登录账号清理。
    if let Ok(mut channel) = open_channel(&sess, &format!("rm -rf -- {}", crate::deploy::script::sh_quote(staging))) {
        let _ = channel.send_eof();
        let _ = collect_output(&sess, channel, read_timeout);
    }
    result
}

fn scp_write(sess: &ssh2::Session, data: &[u8], remote: &str, mode: i32) -> Result<(), ModelinferError> {
    let mut remote_file = sess
        .scp_send(Path::new(remote), mode, data.len() as u64, None)
        .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("SCP 打开远端文件失败: {e}")))?;
    remote_file
        .write_all(data)
        .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("SCP 写远端文件失败: {e}")))?;
    let _ = remote_file.send_eof();
    let _ = remote_file.wait_eof();
    remote_file
        .close()
        .map_err(|e| ModelinferError::new(codes::SSH_COMMAND_FAILED, format!("SCP 关闭远端文件失败: {e}")))?;
    let _ = remote_file.wait_close();
    Ok(())
}

fn resolve(target: &SshTarget) -> Result<std::net::SocketAddr, ModelinferError> {
    (target.host.as_str(), target.port)
        .to_socket_addrs()
        .ok()
        .and_then(|mut it| it.next())
        .ok_or_else(|| {
            ModelinferError::new(
                codes::SSH_CONNECT_FAILED,
                format!("无法解析主机地址 {}:{}", target.host, target.port),
            )
        })
}

/// 测试替身：按命令子串匹配预设输出；未命中默认 exit 0
#[derive(Default)]
pub struct MockSshExec {
    connect_ok: AtomicBool,
    rules: Mutex<Vec<(String, SshOutput)>>,
    fail_hosts: Mutex<Vec<String>>,
    pub calls: Mutex<Vec<String>>,
    pub uploads: Mutex<Vec<String>>,
    /// 记录每次 `run_with_timeout` 传入的超时（供测试断言阶段超时语义）
    pub timeouts: Mutex<Vec<Duration>>,
    /// 流式执行时依次发送的块
    stream_chunks: Mutex<Vec<Vec<u8>>>,
    /// 记录流式执行调用
    pub stream_calls: Mutex<Vec<String>>,
    /// 记录每次调用使用的认证方式（`password` / `key`；不含凭据明文）
    pub auth_calls: Mutex<Vec<String>>,
}

impl MockSshExec {
    pub fn new() -> Self {
        Self {
            connect_ok: AtomicBool::new(true),
            rules: Mutex::new(Vec::new()),
            fail_hosts: Mutex::new(Vec::new()),
            calls: Mutex::new(Vec::new()),
            uploads: Mutex::new(Vec::new()),
            timeouts: Mutex::new(Vec::new()),
            stream_chunks: Mutex::new(Vec::new()),
            stream_calls: Mutex::new(Vec::new()),
            auth_calls: Mutex::new(Vec::new()),
        }
    }

    /// 记录的认证方式序列
    pub fn recorded_auth(&self) -> Vec<String> {
        self.auth_calls.lock().unwrap().clone()
    }

    /// 设置流式执行时依次发送的块
    pub fn with_stream_chunks(self, chunks: Vec<&str>) -> Self {
        *self.stream_chunks.lock().unwrap() = chunks.into_iter().map(|s| s.as_bytes().to_vec()).collect();
        self
    }

    pub fn with_connect_failure(mut self) -> Self {
        self.connect_ok = AtomicBool::new(false);
        self
    }

    /// 指定主机上的命令一律返回 exit=1（模拟命令失败）
    pub fn with_failing_host(self, host: impl Into<String>) -> Self {
        self.fail_hosts.lock().unwrap().push(host.into());
        self
    }

    /// 命令包含 `substr` 时返回预设结果（首个匹配生效）
    pub fn with_rule(self, substr: impl Into<String>, exit_code: i32, stdout: impl Into<String>) -> Self {
        self.rules.lock().unwrap().push((
            substr.into(),
            SshOutput {
                exit_code,
                stdout: stdout.into(),
                stderr: String::new(),
            },
        ));
        self
    }

    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    pub fn recorded_uploads(&self) -> Vec<String> {
        self.uploads.lock().unwrap().clone()
    }

    /// 记录的 `run_with_timeout` 超时序列
    pub fn recorded_timeouts(&self) -> Vec<Duration> {
        self.timeouts.lock().unwrap().clone()
    }

    /// 记录的流式执行调用
    pub fn recorded_stream_calls(&self) -> Vec<String> {
        self.stream_calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl SshExec for MockSshExec {
    async fn run(&self, target: &SshTarget, cmd: &str) -> Result<SshOutput, ModelinferError> {
        if !self.connect_ok.load(Ordering::SeqCst) {
            return Err(ModelinferError::new(
                codes::SSH_CONNECT_FAILED,
                format!("mock: {} 不可达", target.host),
            ));
        }
        self.auth_calls.lock().unwrap().push(target.auth.kind().to_string());
        self.calls.lock().unwrap().push(format!("{}:{}", target.host, cmd));
        if self.fail_hosts.lock().unwrap().iter().any(|h| h == &target.host) {
            return Ok(SshOutput {
                exit_code: 1,
                stdout: String::new(),
                stderr: "mock: command failed".to_string(),
            });
        }
        let rules = self.rules.lock().unwrap();
        for (substr, out) in rules.iter() {
            if cmd.contains(substr) {
                return Ok(out.clone());
            }
        }
        Ok(SshOutput {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }

    /// 记录超时后委托 `run`（便于测试断言阶段超时取值）
    async fn run_with_timeout(
        &self,
        target: &SshTarget,
        cmd: &str,
        timeout: Duration,
    ) -> Result<SshOutput, ModelinferError> {
        self.timeouts.lock().unwrap().push(timeout);
        self.run(target, cmd).await
    }

    async fn upload(
        &self,
        target: &SshTarget,
        _local: &Path,
        remote: &str,
    ) -> Result<(), ModelinferError> {
        if !self.connect_ok.load(Ordering::SeqCst) {
            return Err(ModelinferError::new(
                codes::SSH_CONNECT_FAILED,
                format!("mock: {} 不可达", target.host),
            ));
        }
        self.auth_calls.lock().unwrap().push(target.auth.kind().to_string());
        self.uploads
            .lock()
            .unwrap()
            .push(format!("{}:{}", target.host, remote));
        if self.fail_hosts.lock().unwrap().iter().any(|h| h == &target.host) {
            return Err(ModelinferError::new(
                codes::SSH_COMMAND_FAILED,
                format!("mock: {} 上传失败", target.host),
            ));
        }
        Ok(())
    }

    /// 流式替身：记录调用后依次发送预设块；接收端关闭即停止
    async fn stream(
        &self,
        target: &SshTarget,
        cmd: &str,
        sink: StreamSink,
        _idle_timeout: Duration,
    ) -> Result<(), ModelinferError> {
        if !self.connect_ok.load(Ordering::SeqCst) {
            return Err(ModelinferError::new(
                codes::SSH_CONNECT_FAILED,
                format!("mock: {} 不可达", target.host),
            ));
        }
        self.stream_calls
            .lock()
            .unwrap()
            .push(format!("{}:{}", target.host, cmd));
        let chunks = self.stream_chunks.lock().unwrap().clone();
        for c in chunks {
            if sink.send(c).is_err() {
                break;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sudo_commands_never_embed_secrets_and_quote_entire_script() {
        let root = SshTarget::new("h", 22, "root", "ssh-secret");
        assert_eq!(root_command(&root, "printf ok"), "printf ok");
        let user = SshTarget::new("h", 22, "admin", "ssh-secret").with_sudo_password("sudo-secret");
        let cmd = root_command(&user, "printf '%s' 'a b' | cat > '/root/a b'");
        assert!(cmd.starts_with("sudo -k -H -S -p '' -u root -- /bin/sh -c "));
        assert!(cmd.contains("exec </dev/null;"));
        assert!(!cmd.contains("secret"));
        assert!(!format!("{user:?}").contains("secret"));
        assert_eq!(user.elevation_password(), Some("sudo-secret"));
        assert_eq!(SshTarget::new("h", 22, "admin", "login").elevation_password(), Some("login"));
        let key = SshTarget::private_key("h", 22, "admin", "private-secret");
        assert!(root_command(&key, "id -u").starts_with("sudo -k -H -n "));
        assert_eq!(key.elevation_password(), None);
    }

    #[tokio::test]
    async fn test_rejects_failed_root_check() {
        let ssh = MockSshExec::new().with_rule(ROOT_CHECK, 1, "");
        let user = SshTarget::new("h", 22, "admin", "p");
        assert_eq!(ssh.test(&user).await.unwrap_err().code, codes::NODE_SUDO_FAILED);
        let root = SshTarget::new("h", 22, "root", "p");
        assert_eq!(ssh.test(&root).await.unwrap_err().code, codes::SSH_COMMAND_FAILED);
    }

    /// 仅实现 `run` 的执行器：验证 trait 默认 `run_with_timeout` 委托 `run`
    struct RunOnly;

    #[async_trait]
    impl SshExec for RunOnly {
        async fn run(&self, _target: &SshTarget, cmd: &str) -> Result<SshOutput, ModelinferError> {
            Ok(SshOutput {
                exit_code: 0,
                stdout: format!("ran:{cmd}"),
                stderr: String::new(),
            })
        }

        async fn upload(
            &self,
            _target: &SshTarget,
            _local: &Path,
            _remote: &str,
        ) -> Result<(), ModelinferError> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn default_run_with_timeout_delegates_to_run() {
        let exec = RunOnly;
        let target = SshTarget::new("10.0.0.1", 22, "root", "x");
        let out = exec
            .run_with_timeout(&target, "echo hi", Duration::from_secs(900))
            .await
            .unwrap();
        assert_eq!(out.stdout, "ran:echo hi");
    }

    /// 仅实现 `run` 的替身：默认 `stream` 退化为一次性执行并单块发送
    #[tokio::test]
    async fn default_stream_delegates_to_run() {
        let exec = RunOnly;
        let target = SshTarget::new("10.0.0.1", 22, "root", "x");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        exec.stream(&target, "echo hi", tx, Duration::from_secs(5))
            .await
            .unwrap();
        let chunk = rx.recv().await.unwrap();
        assert_eq!(String::from_utf8_lossy(&chunk), "ran:echo hi");
    }

    #[tokio::test]
    async fn mock_stream_records_and_sends_chunks() {
        let exec = MockSshExec::new().with_stream_chunks(vec!["a\n", "b\n"]);
        let target = SshTarget::new("10.0.0.1", 22, "root", "x");
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        exec.stream(&target, "docker logs -f --tail 3 ctn", tx, Duration::from_secs(5))
            .await
            .unwrap();
        let mut got = String::new();
        while let Some(c) = rx.recv().await {
            got.push_str(&String::from_utf8_lossy(&c));
        }
        assert_eq!(got, "a\nb\n");
        assert_eq!(exec.recorded_stream_calls().len(), 1);
    }

    #[tokio::test]
    async fn mock_records_timeout_and_delegates() {
        let exec = MockSshExec::new().with_rule("stage_start", 0, "ok\n");
        let target = SshTarget::new("10.0.0.1", 22, "root", "x");

        let out = exec
            .run_with_timeout(&target, "bash stage_start.sh", Duration::from_secs(900))
            .await
            .unwrap();
        assert_eq!(out.stdout, "ok\n");
        assert_eq!(exec.recorded_timeouts(), vec![Duration::from_secs(900)]);
        // 缺省 `run` 不记录超时
        exec.run(&target, "bash stage_start.sh").await.unwrap();
        assert_eq!(exec.recorded_timeouts(), vec![Duration::from_secs(900)]);
    }

    #[test]
    fn auth_constructors_and_kind() {
        let pwd = SshTarget::new("h", 22, "root", "secret");
        assert_eq!(pwd.auth.kind(), "password");
        assert!(pwd.auth.is_configured());
        let key = SshTarget::private_key("h", 22, "root", "KEY");
        assert_eq!(key.auth.kind(), "key");
        let from = SshTarget::from_credentials("h", 22, "root", "key", "pw", "priv");
        assert_eq!(from.auth.kind(), "key");
        assert_eq!(SshAuth::from_parts("", "pw", "").kind(), "password");
        assert!(!SshAuth::from_parts("password", "", "").is_configured());
    }

    /// 凭据 MUST NOT 出现在 Debug 输出中（统一日志安全）
    #[test]
    fn debug_never_leaks_credentials() {
        let t = SshTarget::private_key("10.0.0.1", 22, "root", "SUPER_SECRET_KEY");
        let dbg = format!("{t:?}");
        assert!(dbg.contains("PrivateKey") || dbg.contains("key"));
        assert!(!dbg.contains("SUPER_SECRET_KEY"), "Debug 不应包含私钥: {dbg}");
        let t2 = SshTarget::new("10.0.0.1", 22, "root", "SUPER_SECRET_PW");
        let dbg2 = format!("{t2:?}");
        assert!(!dbg2.contains("SUPER_SECRET_PW"), "Debug 不应包含密码: {dbg2}");
    }
}
