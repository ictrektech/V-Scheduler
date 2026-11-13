mod agent;
mod bootstrap;
mod contract;
mod controller;
mod gpu;
mod identity;
mod local;
mod model;
mod modelhub;
mod modelinfer;
mod router;
mod statistics;
mod voice;
mod vos;
use clap::{Parser, Subcommand};
use std::path::PathBuf;
#[derive(Parser)]
struct Cli {
    #[command(subcommand)]
    command: Mode,
}
#[derive(Subcommand)]
enum Mode {
    Controller {
        #[arg(long)]
        config: PathBuf,
        #[arg(long, env = "SCHEDULER_DATABASE_URL")]
        database_url: Option<String>,
        #[arg(long, default_value = "127.0.0.1:38120")]
        listen: String,
        #[arg(long)]
        ui_dir: Option<PathBuf>,
    },
    ModelHub {
        #[arg(long, default_value = "0.0.0.0:6005")]
        listen: String,
    },
    Agent {
        #[arg(long)]
        config: PathBuf,
    },
}
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Mode::Controller {
            config,
            database_url,
            listen,
            ui_dir,
        } => {
            let model_hub =
                if std::env::var("SCHEDULER_EMBEDDED_MODEL_HUB").as_deref() == Ok("true") {
                    let listen =
                        std::env::var("MODEL_HUB_LISTEN").unwrap_or_else(|_| "0.0.0.0:6005".into());
                    Some(modelhub::start(listen).await?)
                } else {
                    None
                };
            let gateway = router::start(bootstrap::waiting(None), ui_dir.clone()).await?;
            let managed = database_url.is_none();
            let mut url = if managed {
                bootstrap::database(&config)?
            } else {
                database_url
            };
            let has_config = config.try_exists()?;
            if managed && (url.is_none() || !has_config) {
                let can_setup = match url.as_deref() {
                    Some(url) => bootstrap::pending_matches(&config, url),
                    None => !has_config || bootstrap::pending_url(&config).is_some(),
                };
                if !can_setup {
                    if let Some(gateway) = gateway.as_ref() {
                        gateway.set_controller(
                            bootstrap::waiting(Some("SCHEDULER_DATABASE_RECOVERY_REQUIRED")),
                            false,
                        );
                    }
                    eprintln!("Scheduler initialization: SCHEDULER_DATABASE_RECOVERY_REQUIRED");
                    tokio::signal::ctrl_c().await?;
                    return Ok(());
                }
                tokio::select! {
                    result = bootstrap::wait(config.clone(), &listen, ui_dir.clone(), gateway.as_ref()) => result?,
                    _ = tokio::signal::ctrl_c() => return Ok(()),
                }
                url = bootstrap::database(&config)?;
            }
            let url = url.ok_or_else(|| anyhow::anyhow!("database setup incomplete"))?;
            let mut previous_failure = None;
            let result = loop {
                let result = tokio::select! {
                    result=controller::serve(config.clone(),url.clone(),listen.clone(),ui_dir.clone(),gateway.as_ref(),managed)=>result,
                    _=tokio::signal::ctrl_c()=>break Ok(()),
                };
                if result.is_ok() || gateway.as_ref().is_none_or(router::Runtime::is_ready) {
                    break result;
                }
                let error = result.unwrap_err();
                let credentials_failed = bootstrap::database_credentials_failed(&error);
                let failure = if credentials_failed {
                    Some("SCHEDULER_DATABASE_RECOVERY_REQUIRED")
                } else {
                    None
                };
                if managed && credentials_failed && bootstrap::pending_matches(&config, &url) {
                    // Only an unfinished setup from this release can recreate its own database.
                    tokio::select! {
                        result = bootstrap::wait(config.clone(), &listen, ui_dir.clone(), gateway.as_ref()) => result?,
                        _ = tokio::signal::ctrl_c() => break Ok(()),
                    }
                    if let Some(gateway) = gateway.as_ref() {
                        gateway.set_controller(bootstrap::waiting(None), false);
                    }
                    previous_failure = None;
                    continue;
                }
                if previous_failure != Some(failure) {
                    if let Some(gateway) = gateway.as_ref() {
                        gateway.set_controller(bootstrap::waiting(failure), false);
                    }
                    eprintln!(
                        "Scheduler initialization: {}; retrying database startup",
                        failure.unwrap_or("temporarily unavailable")
                    );
                    previous_failure = Some(failure);
                }
                let delay = if credentials_failed { 30 } else { 5 };
                tokio::select! {_=tokio::time::sleep(std::time::Duration::from_secs(delay))=>{},_=tokio::signal::ctrl_c()=>break Ok(())};
            };
            if let Some(gateway) = gateway {
                gateway.abort();
            }
            if let Some(model_hub) = model_hub {
                model_hub.abort();
            }
            result
        }
        Mode::ModelHub { listen } => modelhub::serve(listen).await,
        Mode::Agent { config } => agent::run(config).await,
    }
}
