//! 通过 tests/ssh-sudo.sh 在隔离容器中运行，不依赖现场设备。
use std::{path::Path, time::Duration};

use modelinfer::{
    error::codes,
    ssh::{RealSshExec, SshExec, SshTarget},
};

#[tokio::test]
#[ignore = "需要 tests/ssh-sudo.sh 启动隔离 SSH 容器"]
async fn root_and_sudo_ssh_matrix() {
    let port = std::env::var("MODELINFER_TEST_SSH_PORT")
        .unwrap()
        .parse()
        .unwrap();
    let key = std::fs::read_to_string(std::env::var("MODELINFER_TEST_SSH_KEY").unwrap()).unwrap();
    let ssh = RealSshExec::new();
    let root = SshTarget::new("127.0.0.1", port, "root", "root-test-password");
    let targets = [
        root.clone(),
        SshTarget::private_key("127.0.0.1", port, "root", &key),
        SshTarget::new("127.0.0.1", port, "admin", "sudo-test-password"),
        SshTarget::private_key("127.0.0.1", port, "admin", &key)
            .with_sudo_password("sudo-test-password"),
        SshTarget::new("127.0.0.1", port, "nopass", "nopass-test-password"),
        SshTarget::private_key("127.0.0.1", port, "nopass", &key),
        SshTarget::private_key("127.0.0.1", port, "nopass", &key)
            .with_sudo_password("ignored-'$()-secret"),
    ];
    let data = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(data.path(), b"uploaded\0bytes\n").unwrap();
    for (i, target) in targets.iter().enumerate() {
        ssh.test(target).await.unwrap();
        let out = ssh
            .run(
                target,
                "id -u; printf '%s' \"quoted ' text\" | cat; printf '\\n%s\\n' \"$HOME\"",
            )
            .await
            .unwrap();
        assert!(out.success(), "{out:?}");
        assert_eq!(out.stdout, "0\nquoted ' text\n/root\n");
        if target.user != "root" {
            let out = ssh
                .run(target, "if read -r secret; then exit 99; fi; printf clean")
                .await
                .unwrap();
            assert_eq!(out.stdout, "clean");
            assert_eq!(out.exit_code, 0);
        }
        let path = format!("/root/upload-{i}");
        ssh.upload(target, data.path(), &path).await.unwrap();
        let out = ssh
            .run(target, &format!("stat -c '%U:%a' {path}; cat {path}"))
            .await
            .unwrap();
        assert_eq!(out.stdout.as_bytes(), b"root:644\nuploaded\0bytes\n");
        assert_eq!(
            ssh.run_with_timeout(target, "exit 17", Duration::from_secs(5))
                .await
                .unwrap()
                .exit_code,
            17
        );
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        ssh.stream(target, "id -u; printf streamed", tx, Duration::from_secs(5))
            .await
            .unwrap();
        let mut stream = Vec::new();
        while let Some(chunk) = rx.recv().await {
            stream.extend(chunk);
        }
        assert_eq!(stream, b"0\nstreamed");
        println!(
            "PASS {} / {} / sudo={} (run, upload, timeout, stream)",
            target.user,
            target.auth.kind(),
            !target.sudo_password.is_empty()
        );
    }
    let admin = &targets[2];
    // 取消日志订阅后立即释放通道，不等待远端持续命令自行结束。
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let stream_target = admin.clone();
    let following = tokio::spawn(async move {
        RealSshExec::new()
            .stream(
                &stream_target,
                "printf ready; while :; do sleep 1; done",
                tx,
                Duration::from_secs(5),
            )
            .await
    });
    assert_eq!(rx.recv().await.unwrap(), b"ready");
    drop(rx);
    tokio::time::timeout(Duration::from_secs(3), following)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    // shell 路径引用及大 stderr 输出不会破坏命令或造成窗口死锁。
    ssh.upload(admin, data.path(), "/root/with ' quotes and spaces")
        .await
        .unwrap();
    let out = ssh
        .run(admin, "head -c 262144 /dev/zero >&2; printf done")
        .await
        .unwrap();
    assert_eq!(out.stdout, "done");
    assert_eq!(out.stderr.len(), 262144);
    assert!(
        ssh.upload(admin, data.path(), "/missing-directory/file")
            .await
            .is_err()
    );
    assert_eq!(
        ssh.run(&root, "find /tmp -maxdepth 1 -name 'modelinfer-upload.*'")
            .await
            .unwrap()
            .stdout,
        ""
    );

    for target in [
        SshTarget::private_key("127.0.0.1", port, "admin", &key),
        SshTarget::new("127.0.0.1", port, "admin", "sudo-test-password")
            .with_sudo_password("wrong"),
        SshTarget::new("127.0.0.1", port, "denied", "denied-test-password"),
        SshTarget::private_key("127.0.0.1", port, "denied", &key),
        SshTarget::new("127.0.0.1", port, "ttyuser", "tty-test-password"),
    ] {
        assert_eq!(
            ssh.test(&target).await.unwrap_err().code,
            codes::NODE_SUDO_FAILED
        );
        assert_eq!(
            ssh.upload(&target, data.path(), "/root/rejected")
                .await
                .unwrap_err()
                .code,
            codes::NODE_SUDO_FAILED
        );
        let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
        assert_eq!(
            ssh.stream(&target, "id -u", tx, Duration::from_secs(5))
                .await
                .unwrap_err()
                .code,
            codes::NODE_SUDO_FAILED
        );
        println!("PASS denied {} / {}", target.user, target.auth.kind());
    }
    // 添加后权限撤销、sudo 缺失均即时失败。
    assert!(
        ssh.run(&root, "rm /etc/sudoers.d/modelinfer-test")
            .await
            .unwrap()
            .success()
    );
    assert_eq!(
        ssh.test(&targets[5]).await.unwrap_err().code,
        codes::NODE_SUDO_FAILED
    );
    assert!(
        ssh.run(&root, "mv /usr/bin/sudo /usr/bin/sudo.disabled")
            .await
            .unwrap()
            .success()
    );
    assert_eq!(
        ssh.test(admin).await.unwrap_err().code,
        codes::NODE_SUDO_FAILED
    );
    assert_eq!(
        ssh.upload(&root, Path::new("/nonexistent-local-file"), "/root/no-file")
            .await
            .unwrap_err()
            .code,
        codes::IO_FAILED
    );
}
