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
        store::ChainStore,
        store_header::StoreHeader,
        validate_header::ValidateHeader,
        ChainSyncEvent, DecodedChainSyncEvent,
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
use generate::{generate_inputs_strategy, parse_json, read_chain_json};
use ledger::{populate_chain_store, FakeStakeDistribution};
use proptest::test_runner::Config;
use pure_stage::{simulation::SimulationBuilder, StageRef};
use pure_stage::{StageGraph, Void};
use simulate::{pure_stage_node_handle, simulate, Trace};
use std::{path::PathBuf, sync::Arc};
use tokio::sync::Mutex;
use tracing::Span;

pub use sync::*;

mod bytes;
pub mod generate;
mod ledger;
pub mod simulate;
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

    /// Generated "block tree" file in JSON
    #[arg(long, default_value = "./chain.json")]
    pub block_tree_file: PathBuf,

    /// Starting point for the (simulated) chain.
    /// Default to genesis hash, eg. all-zero hash.
    #[arg(long, default_value_t = Hash::from([0; 32]))]
    pub start_header: Hash<32>,
}

pub fn run(rt: tokio::runtime::Runtime, args: Args) {
    bootstrap(rt, args);
}

pub fn bootstrap(rt: tokio::runtime::Runtime, args: Args) {
    let network = NetworkName::Testnet(42);
    let global_parameters: &GlobalParameters = network.into();
    let stake_distribution: FakeStakeDistribution =
        FakeStakeDistribution::from_file(&args.stake_distribution_file, global_parameters).unwrap();
    let chain_data_path = args.block_tree_file;
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

    // TODO: wire in more stages and in particular chain selection !!
    let _chain_selector = make_chain_selector(Origin, &chain_store, &vec![]);
    let chain_ref = Arc::new(Mutex::new(chain_store));
    let mut consensus = ValidateHeader::new(Arc::new(stake_distribution), chain_ref.clone());

    run_simulator(
        rt,
        global_parameters.clone(),
        &mut consensus,
        &chain_data_path,
    );
}

fn run_simulator(
    rt: tokio::runtime::Runtime,
    global: GlobalParameters,
    validate_header: &mut ValidateHeader,
    chain_data_path: &PathBuf,
) {
    let config_without_shrink = Config {
        max_shrink_iters: 0,
        cases: 1,
        ..Config::default()
    };
    let number_of_nodes = 1;
    let spawn = move || {
        println!("*** Spawning node!");
        let mut network = SimulationBuilder::default();
        let init_st = ValidateHeader {
            ledger: validate_header.ledger.clone(),
            store: validate_header.store.clone(),
        };

        let init_store = StoreHeader {
            store: validate_header.store.clone(),
        };

        let receive_stage = network.stage(
            "receive_header",
            async |(_state, downstream, out),
                   msg: Envelope<ChainSyncMessage>,
                   eff|
                   -> Result<
                (
                    (),
                    StageRef<DecodedChainSyncEvent, Void>,
                    StageRef<Envelope<ChainSyncMessage>, Void>,
                ),
                Error,
            > {
                match msg.body {
                    ChainSyncMessage::Init { msg_id, .. } => {
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
                    ChainSyncMessage::InitOk { .. } => (),
                    ChainSyncMessage::Fwd {
                        slot, hash, header, ..
                    } => {
                        let decoded = handle_chain_sync(ChainSyncEvent::RollForward {
                            peer: Peer::new(&msg.src),
                            point: Point::Specific(slot.into(), hash.into()),
                            raw_header: header.into(),
                            span: Span::current(),
                        })?;
                        eff.send(&downstream, decoded).await
                    }
                    ChainSyncMessage::Bck { slot, hash, .. } => {
                        let decoded = handle_chain_sync(ChainSyncEvent::Rollback {
                            peer: Peer::new(&msg.src),
                            rollback_point: Point::Specific(slot.into(), hash.into()),
                            span: Span::current(),
                        })?;
                        eff.send(&downstream, decoded).await
                    }
                };
                Ok(((), downstream, out))
            },
        );

        let validate_header_stage = network.stage(
            "validate_header",
            async |(mut state, global, downstream),
                   msg: DecodedChainSyncEvent,
                   eff|
                   -> Result<
                (
                    ValidateHeader,
                    GlobalParameters,
                    StageRef<DecodedChainSyncEvent, Void>,
                ),
                Error,
            > {
                let result = state.validate_header(&eff, msg, &global).await?;
                eff.send(&downstream, result).await;
                Ok((state, global, downstream))
            },
        );

        let store_header_stage =
                network.stage(
                    "store_header",
                    async |(store, downstream),
                           msg: DecodedChainSyncEvent,
                           eff|
                           -> Result<
                        (StoreHeader, StageRef<DecodedChainSyncEvent, Void>),
                        Error,
                    > {
                        let result = store.handle_event(msg).await?;
                        eff.send(&downstream, result).await;
                        Ok((store, downstream))
                    },
                );

        let propagate_header_stage = network.stage(
            "propagate_header",
            async |(next_msg_id, downstream),
                   msg: DecodedChainSyncEvent,
                   eff|
                   -> Result<(u64, StageRef<Envelope<ChainSyncMessage>, Void>), Error> {
                let msg_id = next_msg_id;
                let (peer, encoded) = match msg {
                    DecodedChainSyncEvent::RollForward {
                        peer,
                        point,
                        header,
                        ..
                    } => (
                        peer,
                        ChainSyncMessage::Fwd {
                            msg_id,
                            slot: point.slot_or_default(),
                            hash: match point {
                                Origin => Bytes { bytes: vec![0; 32] },
                                Specific(_slot, hash) => Bytes { bytes: hash },
                            },
                            header: Bytes {
                                bytes: to_cbor(&header),
                            },
                        },
                    ),
                    DecodedChainSyncEvent::Rollback {
                        peer,
                        rollback_point,
                        ..
                    } => (
                        peer,
                        ChainSyncMessage::Bck {
                            msg_id,
                            slot: rollback_point.slot_or_default(),
                            hash: match rollback_point {
                                Origin => Bytes { bytes: vec![0; 32] },
                                Specific(_slot, hash) => Bytes { bytes: hash },
                            },
                        },
                    ),
                };
                eff.send(
                    &downstream,
                    Envelope {
                        // FIXME: do we have the name of the node stored somewhere?
                        src: "n1".to_string(),
                        // XXX: this should be broadcast to ALL followers
                        dest: peer.name,
                        body: encoded,
                    },
                )
                .await;
                Ok((msg_id + 1, downstream))
            },
        );

        let (output, rx) = network.output("output", 10);
        let receive = network.wire_up(
            receive_stage,
            ((), validate_header_stage.sender(), output.clone()),
        );
        network.wire_up(
            validate_header_stage,
            (init_st, global.clone(), store_header_stage.sender()),
        );
        network.wire_up(
            store_header_stage,
            (init_store, propagate_header_stage.sender()),
        );
        network.wire_up(propagate_header_stage, (0, output.without_state()));

        let running = network.run(rt.handle().clone());
        pure_stage_node_handle(rx, receive, running).unwrap()
    };

    simulate(
        config_without_shrink,
        number_of_nodes,
        spawn,
        generate_inputs_strategy(chain_data_path),
        chain_property(chain_data_path),
    );
}

fn chain_property(
    chain_data_path: &PathBuf,
) -> impl Fn(Trace<ChainSyncMessage>) -> Result<(), String> + use<'_> {
    move |trace| {
        match trace.0.last() {
            None => Err("impossible, no last entry in trace".to_string()),
            Some(entry) => {
                assert_eq!(entry.src, "n1");
                assert_eq!(entry.dest, "c1");
                // FIXME: the property is wrong, we should check the property
                // that the output message trace is a prefix of the read chain
                let data = read_chain_json(chain_data_path);
                let blocks = parse_json(data.as_bytes()).map_err(|err| err.to_string())?;
                match &entry.body {
                    ChainSyncMessage::Fwd { hash, slot, .. } => {
                        let actual = (hash.clone(), *slot);
                        let expected = blocks
                            .last()
                            .map(|block| (block.hash.clone(), Slot::from(block.slot)))
                            .expect("empty chain data");
                        if actual != expected {
                            panic!(
                                "tip of chains don't match, expected {:?}, got {:?}",
                                expected, actual
                            );
                        }
                        println!("Success!")
                    }
                    _ => {
                        println!("TRACE:");
                        for entry in &trace.0 {
                            println!("{:?}", entry);
                        }
                        panic!("Last entry in trace isn't a forward")
                    }
                }
                Ok(())
            }
        }
    }
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
