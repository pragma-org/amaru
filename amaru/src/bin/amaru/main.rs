use clap::{Parser, Subcommand};
use tracing_subscriber::prelude::*;

mod cmd;
mod config;
mod exit;

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the node in all its glory.
    Daemon(cmd::daemon::Args),

    /// Import the ledger state from a CBOR export produced by the Haskell node.
    Import(cmd::import::Args),
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
        Command::Import(args) => cmd::import::run(args).await,
    }
}

pub fn setup_tracing() {
    use is_terminal::IsTerminal;
    use tracing_subscriber::*;

    let filter = EnvFilter::builder()
        .with_default_directive(filter::LevelFilter::INFO.into())
        .from_env_lossy();

    if std::io::stdout().is_terminal() {
        registry()
            .with(
                fmt::layer()
                    .event_format(fmt::format().with_ansi(true).pretty())
                    .with_filter(filter),
            )
            .init();
    } else {
        registry()
            .with(
                fmt::layer()
                    .event_format(fmt::format().json())
                    .with_filter(filter),
            )
            .init();
    }
}
