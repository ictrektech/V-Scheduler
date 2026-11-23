use std::{process::Stdio, time::Duration};

const PORT: u16 = 38121;

pub fn enabled() -> bool {
    std::env::var("SCHEDULER_NATIVE_NODES").as_deref() == Ok("true")
}

pub fn origin() -> String {
    format!("http://127.0.0.1:{PORT}")
}

pub fn start(database_url: &str) -> Option<tokio::task::JoinHandle<()>> {
    if !enabled() {
        return None;
    }
    let database_url = database_url.to_owned();
    Some(tokio::spawn(async move {
        loop {
            match tokio::process::Command::new("scheduler-node-manager")
                .env("SCHEDULER_NODE_DATABASE_URL", &database_url)
                .arg("--port")
                .arg(PORT.to_string())
                .stdin(Stdio::null())
                .kill_on_drop(true)
                .spawn()
            {
                Ok(mut child) => match child.wait().await {
                    Ok(status) => eprintln!("Node manager exited ({status})"),
                    Err(error) => eprintln!("Failed to wait for node manager: {error}"),
                },
                Err(error) => eprintln!("Failed to start node manager: {error}"),
            }
            tokio::time::sleep(Duration::from_secs(5)).await;
        }
    }))
}
