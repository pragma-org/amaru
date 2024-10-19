use crate::config::NetworkName;
use amaru::sync::{Config, Point};
use clap::{builder::TypedValueParser as _, Parser};
use miette::{Diagnostic, IntoDiagnostic};
use pallas_network::facades::PeerClient;
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio_util::sync::CancellationToken;
use tracing::debug;

#[derive(Debug, Parser)]
pub struct Args {
    /// Upstream peer address to synchronize from.
    #[arg(long)]
    peer_address: String,

    /// A `slot#block_header_hash` point to start synchronizing from
    /// For example:
    ///   61841373.fddcbaddb5fce04f01d26a39d5bab6b59ed2387412e9cf7431f00f25f9ce557d
    #[arg(long, verbatim_doc_comment)]
    from: String,

    /// The target network to choose from.
    #[arg(
        long,
        default_value_t = NetworkName::Mainnet,
        value_parser = clap::builder::PossibleValuesParser::new(NetworkName::possible_values())
            .map(|s| s.parse::<NetworkName>().unwrap()),
    )]
    network: NetworkName,
}

pub async fn run(args: Args) -> miette::Result<()> {
    let config = parse_args(args)?;

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

#[derive(Debug, thiserror::Error, Diagnostic)]
enum Error<'a> {
    #[error("malformed point's block header hash: failed to decode it from hex string")]
    MalformedPointHeaderHash(hex::FromHexError),
    #[error("malformed point: {}", .0)]
    MalformedPoint(&'a str),
}

fn parse_args(args: Args) -> miette::Result<Config> {
    let point = parse_point(&args.from)?;

    Ok(Config {
        upstream_peer: args.peer_address,
        network_magic: args.network.to_network_magic(),
        intersection: vec![point],
    })
}

// NOTE: Consider moving this into a shared module if necessary.
fn parse_point(raw_str: &str) -> miette::Result<Point> {
    let mut split = raw_str.split('.');

    let slot = split
        .next()
        .ok_or(Error::MalformedPoint("missing block header hash after '.'"))
        .and_then(|s| {
            s.parse::<u64>().map_err(|_| {
                Error::MalformedPoint("failed to parse point's slot as a non-negative integer")
            })
        })
        .into_diagnostic()?;

    let block_header_hash = split
        .next()
        .ok_or(Error::MalformedPoint("missing block header hash after '.'"))
        .and_then(|s| hex::decode(s).map_err(Error::MalformedPointHeaderHash))
        .into_diagnostic()?;

    Ok(Point::Specific(slot, block_header_hash))
}
