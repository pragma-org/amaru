// Copyright 2025 PRAGMA
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::cmd::{
    DEFAULT_NETWORK, bootstrap::import_nonces_for_network,
    import_headers::import_headers_for_network,
};
use amaru_kernel::{default_chain_dir, network::NetworkName};
use clap::{Parser, arg};
use mithril_client::{ClientBuilder, MessageBuilder};
use std::{error::Error, fs, path::PathBuf};
use tracing::info;

#[derive(Debug, Parser)]
pub struct Args {
    /// Path of the ledger on-disk storage.
    #[arg(long, value_name = "DIR")]
    ledger_dir: Option<PathBuf>,

    /// Path of the chain on-disk storage.
    #[arg(long, value_name = "DIR")]
    chain_dir: Option<PathBuf>,

    /// Network to bootstrap the node for.
    ///
    /// Should be one of 'mainnet', 'preprod', 'preview' or 'testnet:<magic>' where
    /// `magic` is a 32-bits unsigned value denoting a particular testnet.
    #[arg(
        long,
        value_name = "NETWORK",
        default_value_t = DEFAULT_NETWORK,
    )]
    network: NetworkName,

    /// Path to directory containing per-network bootstrap configuration files.
    ///
    /// This path will be used as a prefix to resolve per-network configuration files
    /// needed for bootstrapping. Given a source directory `data`, and a
    /// a network name of `preview`, the expected layout for configuration files would be:
    ///
    /// * `data/preview/snapshots.json`: a list of `Snapshot` vaalues,
    /// * `data/preview/nonces.json`: a list of `InitialNonces` values,
    /// * `data/preview/headers.json`: a list of `Point`s.
    #[arg(
        long,
        value_name = "DIRECTORY",
        default_value = "data",
        verbatim_doc_comment
    )]
    config_dir: PathBuf,
}

pub async fn run(args: Args) -> Result<(), Box<dyn Error>> {
    info!(config=?args.config_dir, ledger_dir=?args.ledger_dir, chain_dir=?args.chain_dir, network=%args.network,
          "bootstrapping from mithril",
    );

    let network = args.network;

    /*let ledger_dir = args
    .ledger_dir
    .unwrap_or_else(|| default_ledger_dir(args.network).into());*/

    let chain_dir = args
        .chain_dir
        .unwrap_or_else(|| default_chain_dir(args.network).into());

    let network_dir = args.config_dir.join(&*network.to_string());

    const AGGREGATOR_ENDPOINT: &str =
        "https://aggregator.release-preprod.api.mithril.network/aggregator";
    const GENESIS_VERIFICATION_KEY: &str = "5b3132372c37332c3132342c3136312c362c3133372c3133312c3231332c3230372c3131372c3139382c38352c3137362c3139392c3136322c3234312c36382c3132332c3131392c3134352c31332c3233322c3234332c34392c3232392c322c3234392c3230352c3230352c33392c3233352c34345d";
    const ANCILLARY_VERIFICATION_KEY: &str = "5b3138392c3139322c3231362c3135302c3131342c3231362c3233372c3231302c34352c31382c32312c3139362c3230382c3234362c3134362c322c3235322c3234332c3235312c3139372c32382c3135372c3230342c3134352c33302c31342c3232382c3136382c3132392c38332c3133362c33365d";
    let client = ClientBuilder::aggregator(AGGREGATOR_ENDPOINT, GENESIS_VERIFICATION_KEY)
        .set_ancillary_verification_key(ANCILLARY_VERIFICATION_KEY.to_string())
        .build()?;

    let database_client = client.cardano_database();
    let snapshots = database_client.list().await?;
    let snapshot_list_item = snapshots.first().ok_or("no snapshot found")?;
    let snapshot = database_client
        .get(&snapshot_list_item.digest)
        .await?
        .ok_or("no snapshot found")?;

    let certificate = client
        .certificate()
        .verify_chain(&snapshot.certificate_hash)
        .await?;

    let target_dir = PathBuf::from("mithril-snapshots");
    fs::create_dir_all(&target_dir)?;
    database_client
        .download_unpack_full(&snapshot, &target_dir)
        .await?;
    // TODO allow to provide user feedback

    info!("Snapshot unpacked to: {:?}", target_dir);

    let message = MessageBuilder::new()
        .compute_snapshot_message(&certificate, &target_dir)
        .await?;
    assert!(certificate.match_message(&message));

    info!("Snapshot verified against certificate");

    let _snapshots_dir = target_dir.join("ledger");

    // TODO Use db-server to get the expected CBOR file by leveraging `ExtLedgerState ShelleyBlock`
    // Then convert it into the format expected by  using convert_snapshot_to

    import_nonces_for_network(network, &network_dir, &chain_dir).await?;

    import_headers_for_network(network, &network_dir, &chain_dir).await?;

    Ok(())
}
