use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

mod cmd;
mod config;
mod exit;

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the node in all its glory.
    Daemon(cmd::daemon::Args),
}

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
    setup_tracing();

    let args = Cli::parse();

    match args.command {
        Command::Daemon(args) => cmd::daemon::run(args).await,
    }
}

pub fn setup_tracing() {
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_filter(tracing_subscriber::EnvFilter::from_default_env()),
        )
        .init();
}
