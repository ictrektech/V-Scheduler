use std::{process::Stdio, time::Duration};

pub const ORIGIN: &str = "http://127.0.0.1:38124";

pub fn enabled() -> bool {
    std::env::var("SCHEDULER_EMBEDDED_IMAGE_HUB").as_deref() == Ok("true")
}

pub fn start() -> Option<tokio::task::JoinHandle<()>> {
    if !enabled() {
        return None;
    }
    Some(tokio::spawn(async {
        loop {
            let process = tokio::process::Command::new("image-hub-entrypoint.sh")
                .args([
                    "--port", "38124",
                    "--frontend-dir", "/opt/v-scheduler/image-hub-ui",
                    "--data-dir", "/var/lib/image-hub",
                ])
                .env("VOS_IMAGE_HUB_BIND_ADDRESS", "127.0.0.1")
                .env_remove("SCHEDULER_PGV_BOOTSTRAP_PASSWORD")
                .kill_on_drop(true)
                .stdin(Stdio::null())
                .spawn();
            match process {
                Ok(mut child) => match child.wait().await {
                    Ok(status) => eprintln!("Image Hub exited ({status})"),
                    Err(error) => eprintln!("Failed to wait for Image Hub: {error}"),
                },
                Err(error) => eprintln!("Failed to start Image Hub: {error}"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }))
}
