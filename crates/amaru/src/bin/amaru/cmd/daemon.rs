use crate::config::NetworkName;
use amaru::{consensus::nonce, sync::Config};
use clap::{builder::TypedValueParser as _, Parser};
use miette::IntoDiagnostic;
use opentelemetry::metrics::Counter;
use pallas_network::facades::PeerClient;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error};

#[derive(Debug, Parser)]
pub struct Args {
    /// Upstream peer address to synchronize from.
    #[arg(long)]
    peer_address: String,

    /// The target network to choose from.
    #[arg(
        long,
        default_value_t = NetworkName::Mainnet,
        value_parser = clap::builder::PossibleValuesParser::new(NetworkName::possible_values())
            .map(|s| s.parse::<NetworkName>().unwrap()),
    )]
    network: NetworkName,

    /// Path to the directory containing blockchain data such as epoch nonces.
    #[arg(long, default_value = "./data")]
    data_dir: String,
}

pub async fn run(args: Args, counter: Counter<u64>) -> miette::Result<()> {
    let config = parse_args(args, counter)?;

    let client = Arc::new(Mutex::new(
        PeerClient::connect(config.upstream_peer.clone(), config.network_magic as u64)
            .await
            .into_diagnostic()?,
    ));

    let sync = amaru::sync::bootstrap(config, &client)?;

    let exit = crate::exit::hook_exit_token();

    run_pipeline(gasket::daemon::Daemon::new(sync), exit.clone()).await;

    Ok(())
}

pub async fn run_pipeline(pipeline: gasket::daemon::Daemon, exit: CancellationToken) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(5000)) => {
                if pipeline.should_stop() {
                    break;
                }
            }
            _ = exit.cancelled() => {
                debug!("exit requested");
                break;
            }
        }
    }

    debug!("shutting down pipeline");

    pipeline.teardown();
}

fn parse_args(args: Args, counter: Counter<u64>) -> miette::Result<Config> {
    // TODO: Figure out from ledger + consensus store
    let root = Path::new(&args.data_dir).join(args.network.to_string());

    let nonces = read_csv(&root.join("nonces.csv"), nonce::from_csv)?;

    Ok(Config {
        upstream_peer: args.peer_address,
        network_magic: args.network.to_network_magic(),
        nonces,
        counter,
    })
}

fn read_csv<F, T>(filepath: &PathBuf, with: F) -> miette::Result<T>
where
    F: FnOnce(&str) -> T,
{
    Ok(with(
        std::str::from_utf8(
            std::fs::read(filepath)
                .inspect_err(|e| {
                    error!(
                        "failed to read csv data file at {}: {e}",
                        filepath.as_path().to_str().unwrap_or_default(),
                    );
                })
                .into_diagnostic()?
                .as_slice(),
        )
        .into_diagnostic()?,
    ))
}
