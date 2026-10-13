mod agent;
mod controller;
mod gpu;
mod model;
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
        database_url: String,
        #[arg(long, default_value = "127.0.0.1:38120")]
        listen: String,
        #[arg(long)]
        ui_dir: Option<PathBuf>,
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
        } => controller::serve(config, database_url, listen, ui_dir).await,
        Mode::Agent { config } => agent::run(config).await,
    }
}
