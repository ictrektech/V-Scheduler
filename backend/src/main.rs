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
            let gateway = router::start(bootstrap::waiting(), ui_dir.clone()).await?;
            let mut url = database_url.or_else(|| bootstrap::database(&config));
            if url.is_none() || !config.exists() {
                bootstrap::wait(config.clone(), &listen, ui_dir.clone(), gateway.as_ref()).await?;
                url = bootstrap::database(&config);
            }
            let url = url.ok_or_else(|| anyhow::anyhow!("database setup incomplete"))?;
            let result = loop {
                let result = tokio::select! {
                    result=controller::serve(config.clone(),url.clone(),listen.clone(),ui_dir.clone(),gateway.as_ref())=>result,
                    _=tokio::signal::ctrl_c()=>break Ok(()),
                };
                if result.is_ok() || gateway.as_ref().is_none_or(router::Runtime::is_ready) {
                    break result;
                }
                // Keep cloud routing and model management available while PGV
                // reconnects. No local execution can pass the pending router.
                if let Some(gateway) = gateway.as_ref() {
                    gateway.set_controller(bootstrap::waiting(), false);
                }
                eprintln!("Scheduler initialization unavailable; retrying in 5 seconds");
                tokio::select! {_=tokio::time::sleep(std::time::Duration::from_secs(5))=>{},_=tokio::signal::ctrl_c()=>break Ok(())};
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
