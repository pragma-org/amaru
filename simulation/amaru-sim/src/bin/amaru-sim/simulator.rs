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

use std::{path::PathBuf, sync::Arc};

use crate::ledger::{populate_chain_store, FakeStakeDistribution};
use crate::sync::{
    mk_message, read_peer_addresses_from_init, MessageReader, OutputWriter, StdinMessageReader,
};
use amaru_consensus::consensus::wiring::PullEvent;
use amaru_consensus::consensus::{
    chain_selection::{ChainSelector, ChainSelectorBuilder},
    header_validation::Consensus,
    store::{rocksdb::RocksDBStore, ChainStore},
};
use amaru_consensus::peer::Peer;
use amaru_kernel::{
    Header,
    Point::{self, *},
};
pub use pallas_crypto::hash::Hash;
use amaru_sim::echo::Envelope;
use clap::Parser;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Debug, Parser)]
#[clap(name = "Amaru Simulator")]
#[clap(bin_name = "amaru-sim")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Path of JSON-formatted stake distribution file.
    #[arg(long, default_value = "./stake_distribution.json")]
    stake_distribution_file: PathBuf,

    /// Path of JSON-formatted consensus context file.
    #[arg(long, default_value = "./consensus_context.json")]
    consensus_context_file: PathBuf,

    /// Path of the chain on-disk storage.
    #[arg(long, default_value = "./chain.db")]
    chain_dir: PathBuf,

    /// Path to the directory containing blockchain data such as epoch nonces.
    #[arg(long, default_value = "./data")]
    data_dir: PathBuf,

    /// Starting point for the (simulated) chain.
    /// Default to genesis hash, eg. all-zero hash.
    #[arg(long, default_value_t = Hash::from([0; 32]))]
    start_header: Hash<32>,
    
}

pub async fn run(args: Args) {
    let consensus = bootstrap(args);

    consensus.await;
}

pub async fn bootstrap(args: Args) {
    let mut input_reader = StdinMessageReader::new();

    let stake_distribution: FakeStakeDistribution =
        FakeStakeDistribution::from_file(&args.stake_distribution_file).unwrap();

    let mut chain_store = RocksDBStore::new(args.chain_dir.clone())
        .unwrap_or_else(|e| panic!("unable to open chain store at {}: {:?}", args.chain_dir.display(), e));

    populate_chain_store(&mut chain_store, &args.start_header, &args.consensus_context_file).unwrap();

    let peer_addresses = read_peer_addresses_from_init(&mut input_reader)
        .await
        .unwrap();

    info!("using upstream peer addresses: {:?}", peer_addresses);

    let chain_selector = make_chain_selector(
        Origin,
        &chain_store,
        &peer_addresses
            .iter()
            .map(|a| Peer::new(&a.clone()))
            .collect::<Vec<_>>(),
    );
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let mut consensus = Consensus::new(
        Box::new(stake_distribution),
        chain_ref.clone(),
        chain_selector,
    );

    run_simulator(&mut input_reader, chain_ref, &mut consensus).await;
}

async fn run_simulator(
    input_reader: &mut impl MessageReader,
    _store: Arc<Mutex<dyn ChainStore<Header>>>,
    consensus: &mut Consensus,
) {
    let mut output_writer = OutputWriter::new();
    loop {
        let span = tracing::info_span!("simulator");
        match input_reader.read().await {
            Err(err) => {
                tracing::error!("Error reading message: {:?}", err);
                break;
            }
            Ok(msg) => {
                let events = match mk_message(msg, span) {
                    Ok(event) => match event {
                        PullEvent::RollForward(peer, point, raw_header, _span) => {
                            consensus
                                .handle_roll_forward(&peer, &point, &raw_header)
                                .await
                        }
                        PullEvent::Rollback(peer, rollback) => {
                            consensus.handle_roll_back(&peer, &rollback).await
                        }
                    },
                    Err(_) => todo!(),
                };

                match events {
                    Ok(events) => {
                        output_writer
                            .write(
                                events
                                    .iter()
                                    .map(|e| Envelope {
                                        src: "n1".to_string(),
                                        dest: "".to_string(),
                                        body: e.into(),
                                    })
                                    .collect(),
                            )
                            .await;
                    }
                    Err(e) => {
                        tracing::error!("Error processing event: {:?}", e);
                        return;
                    }
                }
            }
        }
    }
    info!("no more messages to process, exiting");
}

fn make_chain_selector(
    tip: Point,
    chain_store: &impl ChainStore<Header>,
    peers: &Vec<Peer>,
) -> Arc<Mutex<ChainSelector<Header>>> {
    let mut builder = ChainSelectorBuilder::new();

    load_tip_from_store(chain_store, tip, &mut builder);

    for peer in peers {
        builder.add_peer(peer);
    }

    match builder.build() {
        Ok(chain_selector) => Arc::new(Mutex::new(chain_selector)),
        Err(e) => panic!("unable to build chain selector: {:?}", e),
    }
}

fn load_tip_from_store<'a>(
    chain_store: &impl ChainStore<Header>,
    tip: Point,
    builder: &'a mut ChainSelectorBuilder<Header>,
) -> &'a mut ChainSelectorBuilder<Header> {
    match tip {
        Origin => builder,
        Specific(..) => match chain_store.load_header(&From::from(&tip)) {
            None => panic!("Tip {:?} not found in chain store", tip),
            Some(header) => builder.set_tip(&header),
        },
    }
}
