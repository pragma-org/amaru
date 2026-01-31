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

use crate::{
    simulator::{
        Args, Envelope, History, NodeConfig, NodeHandle, SimulateConfig, bytes::Bytes,
        generate::generate_entries, simulate::simulate,
    },
    sync::ChainSyncMessage,
};
use amaru::stages::build_stage_graph::build_stage_graph;
use amaru_consensus::{
    effects::{
        ForwardEvent, ForwardEventListener, ResourceBlockValidation, ResourceForwardEventListener,
        ResourceHeaderStore, ResourceHeaderValidation, ResourceParameters,
    },
    events::{ChainSyncEvent, default_block},
    headers_tree::{
        HeadersTreeState,
        data_generation::{Chain, GeneratedActions},
    },
    stages::{
        pull::SyncTracker,
        select_chain::{DEFAULT_MAXIMUM_FRAGMENT_LENGTH, SelectChain},
    },
};
use amaru_kernel::{
    BlockHeader, GlobalParameters, Hash, IsHeader, NetworkName, Peer, Point, Tip, to_cbor,
    utils::string::{ListDebug, ListToString, ListsToString},
};
use amaru_ouroboros::{
    ChainStore,
    can_validate_blocks::mock::{MockCanValidateBlocks, MockCanValidateHeaders},
    in_memory_consensus_store::InMemConsensusStore,
};
use amaru_protocols::{blockfetch::Blocks, manager::ManagerMessage};
use async_trait::async_trait;
use pure_stage::{
    Instant, Receiver, StageGraph, StageRef,
    simulation::{RandStdRng, SimulationBuilder},
    trace_buffer::{TraceBuffer, TraceEntry},
};
use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::{runtime::Runtime, sync::mpsc};
use tracing::{Span, info};

/// Run the full simulation:
///
/// * Create a simulation environment.
/// * Run the simulation.
pub fn run(args: Args) {
    let rt = Runtime::new().unwrap();
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));
    let node_config = NodeConfig::from(args.clone());

    let spawn = |node_id: String, rng: RandStdRng| {
        let mut network = SimulationBuilder::default()
            .with_trace_buffer(trace_buffer.clone())
            .with_mailbox_size(10000)
            .with_eval_strategy(rng);
        let (input, init_messages, output) = spawn_node(node_id, node_config.clone(), &mut network);
        let running = network.run();
        NodeHandle::from_pure_stage(input, init_messages, output, running, rt.handle().clone())
            .unwrap()
    };

    let simulate_config = SimulateConfig::from(args.clone());
    simulate(
        &simulate_config,
        &node_config,
        spawn,
        generate_entries(
            &node_config,
            Instant::at_offset(Duration::from_secs(0)),
            200.0,
        ),
        chain_property(),
        display_entries_statistics,
        trace_buffer.clone(),
        args.persist_on_success,
    )
    .unwrap_or_else(|e| panic!("{e}"));
}

/// Create and start a node
/// Return:
///
///  * A handle to send messages to the node
///  * A handle to receive init messages executed on the node
///  * A handle to receive output messages from the node
///
pub fn spawn_node(
    node_id: String,
    node_config: NodeConfig,
    network: &mut SimulationBuilder,
) -> (
    StageRef<Envelope<ChainSyncMessage>>,
    Receiver<Envelope<ChainSyncMessage>>,
    Receiver<Envelope<ChainSyncMessage>>,
) {
    info!(node_id, "node.spawn");
    let (network_name, select_chain, _sync_tracker, resource_header_store, resource_validation) =
        init_node(&node_config);
    let global_parameters: &GlobalParameters = network_name.into();

    // The receiver replies ok to init messages from the sender (via 'output', the only output of the graph)
    // and forwards chain sync messages to the rest of the processing graph
    let receiver = network.stage(
        "receiver",
        async |(downstream, output), msg: Envelope<ChainSyncMessage>, eff| {
            match msg.body {
                ChainSyncMessage::Init { msg_id, .. } => {
                    eff.send(
                        &output,
                        // Reply with InitOk
                        Envelope {
                            src: msg.dest,
                            dest: msg.src,
                            body: ChainSyncMessage::InitOk {
                                in_reply_to: msg_id,
                            },
                        },
                    )
                    .await
                }
                ChainSyncMessage::InitOk { .. } => (),
                ChainSyncMessage::Fwd {
                    slot, hash, header, ..
                } => {
                    let point = Point::Specific(slot, Hash::from(&*hash.bytes));
                    let tip = Tip::new(point, 0.into());
                    eff.send(
                        &downstream,
                        ChainSyncEvent::RollForward {
                            peer: Peer::new(&msg.src),
                            tip,
                            raw_header: header.into(),
                            span: Span::current(),
                        },
                    )
                    .await
                }
                ChainSyncMessage::Bck { slot, hash, .. } => {
                    let point = Point::Specific(slot, Hash::from(&*hash.bytes));
                    let tip = Tip::new(point, 0.into());
                    eff.send(
                        &downstream,
                        ChainSyncEvent::Rollback {
                            peer: Peer::new(&msg.src),
                            rollback_point: point,
                            tip,
                            span: Span::current(),
                        },
                    )
                    .await
                }
            }
            (downstream, output)
        },
    );

    // TODO: switch simulation to tracking network bytes
    let manager = network.stage("manager", async |_, msg: ManagerMessage, eff| match msg {
        ManagerMessage::AddPeer(_) => {}
        ManagerMessage::RemovePeer(_) => {}
        ManagerMessage::Connect(_) => {}
        ManagerMessage::ConnectionDied(_, _, _) => {}
        ManagerMessage::FetchBlocks { cr, .. } => {
            // We need to return a non-empty block to proceed with the simulation.
            // That block needs to be the same that is deserialized by default with the default_block() function
            // used with the ValidateBlockEvent deserializer, otherwise the replay test will fail.
            eff.send(
                &cr,
                Blocks {
                    blocks: vec![default_block().deref().to_vec()],
                },
            )
            .await;
        }
        ManagerMessage::Accepted(_, _) => {}
    });
    let manager = network.wire_up(manager, ());

    let our_tip = Tip::origin();
    let receive_header_ref =
        build_stage_graph(select_chain, our_tip, manager.without_state(), network);

    let (output, rx1) = network.output("output", 10);

    // The number of received messages sent by the forward event listener is proportional
    // to the number of downstream peers, as each event is duplicated to each downstream peer.
    let (sender, rx2) = mpsc::channel(1_000_000);
    let listener =
        MockForwardEventListener::new(node_id, node_config.number_of_downstream_peers, sender);

    let receiver = network.wire_up(receiver, (receive_header_ref, output.clone()));

    network
        .resources()
        .put::<ResourceHeaderStore>(resource_header_store.clone());
    network
        .resources()
        .put::<ResourceHeaderValidation>(resource_validation);
    network
        .resources()
        .put::<ResourceParameters>(global_parameters.clone());
    network
        .resources()
        .put::<ResourceBlockValidation>(Arc::new(MockCanValidateBlocks));
    network
        .resources()
        .put::<ResourceForwardEventListener>(Arc::new(listener));

    (receiver.without_state(), rx1, Receiver::new(rx2))
}

fn init_node(
    node_config: &NodeConfig,
) -> (
    NetworkName,
    SelectChain,
    SyncTracker,
    ResourceHeaderStore,
    ResourceHeaderValidation,
) {
    let network_name = NetworkName::Testnet(42);
    let chain_store = Arc::new(InMemConsensusStore::new());
    let header_validation = Arc::new(MockCanValidateHeaders);

    let peers = (1..=node_config.number_of_upstream_peers)
        .map(|i| Peer::new(&format!("c{}", i)))
        .collect::<Vec<_>>();
    let select_chain = make_chain_selector(
        chain_store.clone(),
        node_config.generated_chain_depth,
        &peers,
    );
    let sync_tracker = SyncTracker::new(&peers);

    (
        network_name,
        select_chain,
        sync_tracker,
        chain_store,
        header_validation,
    )
}

fn make_chain_selector(
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
    generated_chain_depth: u64,
    peers: &Vec<Peer>,
) -> SelectChain {
    // Set the maximum length for the best fragment based on the generated chain depth
    // so we generated roll forwards that move the best chain anchor.
    let max_length = if generated_chain_depth > DEFAULT_MAXIMUM_FRAGMENT_LENGTH as u64 {
        DEFAULT_MAXIMUM_FRAGMENT_LENGTH
    } else {
        generated_chain_depth as usize - 1
    };
    let mut tree_state = HeadersTreeState::new(max_length);
    let anchor = chain_store.get_anchor_hash();
    for peer in peers {
        tree_state
            .initialize_peer(chain_store.clone(), peer, &anchor)
            .expect("the root node is guaranteed to already be in the tree")
    }
    SelectChain::new(tree_state)
}

/// Property: at the end of the simulation, the chain built from the history of messages received
/// downstream must match one of the best chains built directly from messages coming from upstream peers.
///
/// TODO: at some point we should implement a deterministic tie breaker when multiple best chains exist
/// based on the VRF key of the received headers.
fn chain_property() -> impl Fn(&History<ChainSyncMessage>, &GeneratedActions) -> Result<(), String>
{
    move |history, generated_actions| {
        let actual = make_best_chain_from_downstream_messages(history)?;
        let generated_tree = generated_actions.generated_tree();
        let best_chains = generated_tree.best_chains();

        if !best_chains.contains(&actual) {
            let actions_as_string: String = generated_actions
                .actions()
                .iter()
                .map(|action| action.pretty_print())
                .collect::<Vec<_>>()
                .list_to_string(",\n");

            Err(format!(
                r#"
The actual chain

{}

is not in the best chains

{}

The headers tree is
{}

The actions are

{}
"#,
                actual.list_to_string(",\n  "),
                best_chains.lists_to_string(",\n  ", ",\n  "),
                generated_actions.generated_tree().tree(),
                actions_as_string
            ))
        } else {
            Ok(())
        }
    }
}

/// Generate statistics from actions and log them.
fn display_entries_statistics(generated_actions: &GeneratedActions) {
    let statistics = generated_actions.statistics();
    info!(tree_depth=%statistics.tree_depth,
          tree_nodes=%statistics.number_of_nodes,
          tree_forks=%statistics.number_of_fork_nodes,
          "simulate.generate_test_data.statistics");
}

/// Build the best chain from messages sent to downstream peers.
fn make_best_chain_from_downstream_messages(
    history: &History<ChainSyncMessage>,
) -> Result<Chain, String> {
    let mut best_chain = vec![];
    for (i, message) in history.0.iter().enumerate() {
        // only consider messages sent to the first peer
        if !message.dest.starts_with("c1") {
            continue;
        };
        match &message.body {
            msg @ ChainSyncMessage::Fwd { .. } => {
                if let Some(header) = msg.decode_block_header() {
                    best_chain.push(header)
                }
            }
            msg @ ChainSyncMessage::Bck { .. } => {
                if let Some(header_hash) = msg.header_hash() {
                    let rollback_position = best_chain.iter().position(|h| h.hash() == header_hash);
                    if let Some(rollback_position) = rollback_position {
                        best_chain.truncate(rollback_position + 1);
                    } else {
                        Err(format!(
                            "after the action {}, we have a rollback position that does not exist with hash {}.\nThe best chain is:\n{}. The history is:\n{}",
                            i + 1,
                            header_hash,
                            best_chain.list_to_string(",\n"),
                            history.0.iter().collect::<Vec<_>>().list_debug("\n")
                        ))?;
                    }
                }
            }
            _ => (),
        }
    }
    Ok(best_chain)
}

/// Replay a previous simulation run:
pub fn replay(args: Args, traces: Vec<TraceEntry>) -> anyhow::Result<()> {
    let mut network = SimulationBuilder::default();
    let node_config = NodeConfig::from(args);
    let _ = spawn_node("n1".to_string(), node_config, &mut network);
    let mut replay = network.replay();
    replay.run_trace(traces)
}

/// This implementation of ForwardEventListener sends the received events a Sender that can collect them
/// to check them later.
///
/// A message id is maintained for each received event and each message is duplicated to the number of downstream peers.
#[derive(Clone, Debug)]
pub struct MockForwardEventListener {
    node_id: String,
    number_of_downstream_peers: u8,
    sender: mpsc::Sender<Envelope<ChainSyncMessage>>,
    msg_id: Arc<AtomicU64>,
}

impl MockForwardEventListener {
    pub fn new(
        node_id: String,
        number_of_downstream_peers: u8,
        sender: mpsc::Sender<Envelope<ChainSyncMessage>>,
    ) -> Self {
        Self {
            node_id,
            number_of_downstream_peers,
            msg_id: Arc::new(AtomicU64::new(0)),
            sender,
        }
    }
}

#[async_trait]
impl ForwardEventListener for MockForwardEventListener {
    async fn send(&self, event: ForwardEvent) -> anyhow::Result<()> {
        fn message(event: &ForwardEvent, msg_id: u64) -> ChainSyncMessage {
            match event {
                ForwardEvent::Forward(header) => ChainSyncMessage::Fwd {
                    msg_id,
                    slot: header.point().slot_or_default(),
                    hash: Bytes {
                        bytes: header.hash().as_slice().to_vec(),
                    },
                    header: Bytes {
                        bytes: to_cbor(&header),
                    },
                },
                ForwardEvent::Backward(tip) => ChainSyncMessage::Bck {
                    msg_id,
                    slot: tip.point().slot_or_default(),
                    hash: Bytes {
                        bytes: tip.hash().as_slice().to_vec(),
                    },
                },
            }
        }

        // This allocates a range of message ids from
        // self.msg_id to self.msg_id + number_of_downstream_peers
        let base_msg_id = self
            .msg_id
            .fetch_add(self.number_of_downstream_peers as u64, Ordering::Relaxed);

        for i in 1..=self.number_of_downstream_peers {
            let dest = format!("c{}", i);
            let msg_id = base_msg_id + i as u64;
            let envelope = Envelope {
                src: self.node_id.clone(),
                dest,
                body: message(&event, msg_id),
            };
            self.sender.send(envelope).await?;
        }
        Ok(())
    }
}
