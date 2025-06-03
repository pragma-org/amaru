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

use super::echo::Envelope;
use amaru_consensus::{
    consensus::{
        chain_selection::{ChainSelector, ChainSelectorBuilder},
        receive_header::handle_chain_sync,
        select_chain::SelectChain,
        store::ChainStore,
        store_header::StoreHeader,
        validate_header::ValidateHeader,
        ChainSyncEvent, DecodedChainSyncEvent, ValidateHeaderEvent,
    },
    peer::Peer,
};
use amaru_kernel::{
    network::NetworkName,
    protocol_parameters::GlobalParameters,
    to_cbor, Hash, Header,
    Point::{self, *},
    Slot,
};
use amaru_stores::rocksdb::consensus::RocksDBStore;
use anyhow::Error;
use bytes::Bytes;
use clap::Parser;
use gasket::framework::WorkerError;
use ledger::{populate_chain_store, FakeStakeDistribution};
use proptest::{
    prelude::{BoxedStrategy, Strategy},
    test_runner::Config,
};
use pure_stage::{simulation::SimulationBuilder, StageRef};
use pure_stage::{StageGraph, Void};
use simulate::{pure_stage_node_handle, simulate, NodeHandle, Trace};
use std::{path::PathBuf, sync::Arc};
use sync::{
    mk_message, read_peer_addresses_from_init, ChainSyncMessage, MessageReader, OutputWriter,
    StdinMessageReader,
};
use tokio::sync::Mutex;
use tracing::info;

mod bytes;
pub mod generate;
mod ledger;
mod simulate;
mod sync;

#[derive(Debug, Parser)]
#[clap(name = "Amaru Simulator")]
#[clap(bin_name = "amaru-sim")]
#[clap(author, version, about, long_about = None)]
pub struct Args {
    /// Path of JSON-formatted stake distribution file.
    #[arg(long, default_value = "./stake_distribution.json")]
    pub stake_distribution_file: PathBuf,

    /// Path of JSON-formatted consensus context file.
    #[arg(long, default_value = "./consensus_context.json")]
    pub consensus_context_file: PathBuf,

    /// Path of the chain on-disk storage.
    #[arg(long, default_value = "./chain.db")]
    pub chain_dir: PathBuf,

    /// Path to the directory containing blockchain data such as epoch nonces.
    #[arg(long, default_value = "./data")]
    pub data_dir: PathBuf,

    /// Starting point for the (simulated) chain.
    /// Default to genesis hash, eg. all-zero hash.
    #[arg(long, default_value_t = Hash::from([0; 32]))]
    pub start_header: Hash<32>,
}

pub async fn run(args: Args) {
    bootstrap(args).await;
}

pub async fn bootstrap(args: Args) {
    let global_parameters: &GlobalParameters = network.into();

    let stake_distribution: FakeStakeDistribution =
        FakeStakeDistribution::from_file(&args.stake_distribution_file, global_parameters).unwrap();
    let era_history = network.into();

    let mut chain_store = RocksDBStore::new(&args.chain_dir, era_history).unwrap_or_else(|e| {
        panic!(
            "unable to open chain store at {}: {:?}",
            args.chain_dir.display(),
            e
        )
    });

    populate_chain_store(
        &mut chain_store,
        &args.start_header,
        &args.consensus_context_file,
    )
    .unwrap();

    let chain_selector = make_chain_selector(Origin, &chain_store, &vec![]);
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let mut consensus = ValidateHeader::new(Arc::new(stake_distribution), chain_ref.clone());
    let mut store_header = StoreHeader::new(chain_ref.clone());
    let mut select_chain = SelectChain::new(chain_selector);

    run_simulator(&mut consensus, &mut store_header, &mut select_chain).await;
}

const CHAIN_PROPERTY: fn(Trace<ChainSyncMessage>) -> Result<(), String> =
    |trace: Trace<ChainSyncMessage>| Ok(());

fn arbitrary_message() -> BoxedStrategy<ChainSyncMessage> {
    use proptest::{collection::vec, prelude::*};

    prop_oneof![
        (any::<u64>(), any::<String>(), vec(any::<String>(), 0..10)).prop_map(
            |(msg_id, node_id, node_ids)| ChainSyncMessage::Init {
                msg_id,
                node_id,
                node_ids
            }
        ),
        (any::<u64>()).prop_map(|msg_id| ChainSyncMessage::InitOk {
            in_reply_to: msg_id
        }),
        (any::<u64>(), any::<u64>(), any::<[u8; 32]>()).prop_map(|(msg_id, slot, hash)| {
            ChainSyncMessage::Fwd {
                msg_id,
                slot: Slot::from(slot),
                hash: hash.to_vec().into(),
                header: Bytes { bytes: vec![] },
            }
        }),
        (any::<u64>(), any::<u64>(), any::<[u8; 32]>()).prop_map(|(msg_id, slot, hash)| {
            ChainSyncMessage::Bck {
                msg_id,
                slot: Slot::from(slot),
                hash: hash.to_vec().into(),
            }
        })
    ]
    .boxed()
}

async fn run_simulator(
    validate_header: &mut ValidateHeader,
    _store_header: &mut StoreHeader,
    _select_chain: &mut SelectChain,
) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let config = Config::default();
    let number_of_nodes = 1;
    let spawn = move || {
        println!("*** Spawning node!");
        let mut network = SimulationBuilder::default();
        let init_st = ValidateHeader {
            ledger: validate_header.ledger.clone(),
            store: validate_header.store.clone(),
        };
        let validate_header_stage = network.stage(
            "validate_header",
            async |(mut state, out),
                   msg: Envelope<ChainSyncMessage>,
                   eff|
                   -> Result<
                (ValidateHeader, StageRef<Envelope<ChainSyncMessage>, Void>),
                Error,
            > {
                match msg.body {
                    ChainSyncMessage::Init {
                        msg_id,
                        node_id,
                        node_ids,
                    } => {
                        let reply_msg = ChainSyncMessage::InitOk {
                            in_reply_to: msg_id,
                        };
                        let reply = Envelope {
                            src: msg.dest,
                            dest: msg.src,
                            body: reply_msg,
                        };
                        eff.send(&out, reply).await
                    }
                    ChainSyncMessage::InitOk { in_reply_to } => todo!(),
                    ChainSyncMessage::Fwd {
                        msg_id,
                        slot,
                        hash,
                        header,
                    } => todo!(),
                    ChainSyncMessage::Bck { msg_id, slot, hash } => todo!(),
                };
                Ok((state, out))
            },
        );

        let (output, rx) = network.output("output", 10);
        let basic = network.wire_up(validate_header_stage, (init_st, output.without_state()));
        let running = network.run(rt.handle().clone());
        pure_stage_node_handle(rx, basic, running).unwrap()
    };

    simulate(
        config,
        number_of_nodes,
        spawn,
        arbitrary_message(),
        CHAIN_PROPERTY,
    )
}

// async fn write_events(
//     output_writer: &mut OutputWriter,
//     store: &Arc<Mutex<dyn ChainStore<Header>>>,
//     events: &[ValidateHeaderEvent],
// ) {
//     let mut msgs = vec![];
//     let s = store.lock().await;
//     for e in events {
//         match e {
//             ValidateHeaderEvent::Validated { point, .. } => {
//                 let h: Hash<32> = point.into();
//                 let hdr = s.load_header(&h).unwrap();
//                 let fwd = ChainSyncMessage::Fwd {
//                     msg_id: 0, // FIXME
//                     slot: point.slot_or_default(),
//                     hash: Bytes {
//                         bytes: (*h).to_vec(),
//                     },
//                     header: Bytes {
//                         bytes: to_cbor(&hdr),
//                     },
//                 };
//                 let envelope = Envelope {
//                     src: "n1".to_string(),
//                     dest: "c1".to_string(),
//                     body: fwd,
//                 };
//                 msgs.push(envelope);
//             }
//             ValidateHeaderEvent::Rollback { rollback_point, .. } => {
//                 let h: Hash<32> = rollback_point.into();
//                 let fwd = ChainSyncMessage::Bck {
//                     msg_id: 0, // FIXME
//                     slot: rollback_point.slot_or_default(),
//                     hash: Bytes {
//                         bytes: (*h).to_vec(),
//                     },
//                 };
//                 let envelope = Envelope {
//                     src: "n1".to_string(),
//                     dest: "c1".to_string(),
//                     body: fwd,
//                 };
//                 msgs.push(envelope);
//             }
//         }
//     }

//     output_writer.write(msgs).await;
// }

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
