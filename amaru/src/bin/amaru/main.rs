use clap::{Parser, Subcommand};

mod common;
mod daemon;

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the node in all its glory
    Daemon(daemon::Args),
}

pub struct Config {
    sync: amaru::sync::Config,
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
    let args = Cli::parse();

    // TODO: once we agree on a config file format and structure, load it instead of using this hardcoded value
    let config = Config {
        sync: amaru::sync::Config {
            upstream_peer: "preview-node.world.dev.cardano.org:30002".into(),
            network_magic: 2,
            intersection: vec![pallas_network::miniprotocols::Point::Specific(
                61841373,
                hex::decode("fddcbaddb5fce04f01d26a39d5bab6b59ed2387412e9cf7431f00f25f9ce557d")
                    .unwrap(),
            )],
        },
    };

    match args.command {
        Command::Daemon(args) => daemon::run(config, args).await,
    }
}
