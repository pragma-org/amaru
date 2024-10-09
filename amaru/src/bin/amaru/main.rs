use clap::{Parser, Subcommand};

mod daemon;

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the node in all its glory
    Daemon(daemon::Args),
}

pub struct Config {
    upstream_peer: String,
}

#[derive(Debug, Parser)]
#[clap(name = "Amaru")]
#[clap(bin_name = "amaru")]
#[clap(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

fn main() -> miette::Result<()> {
    let args = Cli::parse();

    // TODO: once we agree on a config file format and structure, load it instead of using this hardcoded value
    let config = Config {
        upstream_peer: "preview-node.world.dev.cardano.org:30002".into(),
    };

    match args.command {
        Command::Daemon(args) => daemon::run(config, args),
    }
}
