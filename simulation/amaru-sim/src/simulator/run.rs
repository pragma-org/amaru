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

use crate::echo::Envelope;
use crate::simulator::NodeConfig;
use crate::simulator::{
    Args, History, NodeHandle, SimulateConfig, bytes::Bytes, generate::generate_entries,
    simulate::simulate,
};
use crate::sync::ChainSyncMessage;
use acto::AcTokio;
use amaru::stages::build_stage_graph::build_stage_graph;
use amaru_consensus::consensus::effects::FetchBlockEffect;
use amaru_consensus::consensus::errors::ConsensusError;
use amaru_consensus::consensus::headers_tree::data_generation::{Chain, GeneratedActions};
use amaru_consensus::consensus::stages::track_peers::SyncTracker;
use amaru_consensus::consensus::{
    effects::{
        ForwardEvent, ForwardEventListener, ResourceBlockValidation, ResourceForwardEventListener,
        ResourceHeaderStore, ResourceHeaderValidation, ResourceParameters,
    },
    headers_tree::HeadersTreeState,
    stages::select_chain::{DEFAULT_MAXIMUM_FRAGMENT_LENGTH, SelectChain},
    tip::HeaderTip,
};
use amaru_kernel::consensus_events::{ChainSyncEvent, Tracked};
use amaru_kernel::string_utils::{ListDebug, ListToString, ListsToString};
use amaru_kernel::{BlockHeader, IsHeader};
use amaru_kernel::{
    Point, network::NetworkName, peer::Peer, protocol_parameters::GlobalParameters, to_cbor,
};
use amaru_network::NetworkResource;
use amaru_ouroboros::can_validate_blocks::mock::MockCanValidateHeaders;
use amaru_ouroboros::network_operations::ResourceNetworkOperations;
use amaru_ouroboros::{
    ChainStore, can_validate_blocks::mock::MockCanValidateBlocks,
    in_memory_consensus_store::InMemConsensusStore,
};
use async_trait::async_trait;
use pure_stage::simulation::SimulationBuilder;
use pure_stage::simulation::running::OverrideResult;
use pure_stage::{Instant, Receiver, StageGraph, StageRef, trace_buffer::TraceBuffer};
use rand::rngs::StdRng;
use std::{
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
pub fn run(rt: Runtime, args: Args) {
    let trace_buffer = Arc::new(parking_lot::Mutex::new(TraceBuffer::new(42, 1_000_000_000)));
    let node_config = NodeConfig::from(args.clone());

    let spawn = |node_id: String, rng: Arc<parking_lot::Mutex<StdRng>>| {
        let mut network = SimulationBuilder::default()
            .with_trace_buffer(trace_buffer.clone())
            .with_rng(rng);
        let (input, init_messages, output) =
            spawn_node(node_id, node_config.clone(), &mut network, &rt);
        let mut running = network.run(rt.handle().clone());
        running.override_external_effect(usize::MAX, |_eff: Box<FetchBlockEffect>| {
            OverrideResult::Handled(Box::new(Ok::<Vec<u8>, ConsensusError>(vec![])))
        });
        NodeHandle::from_pure_stage(input, init_messages, output, running).unwrap()
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
    rt: &Runtime,
) -> (
    StageRef<Envelope<ChainSyncMessage>>,
    Receiver<Envelope<ChainSyncMessage>>,
    Receiver<Envelope<ChainSyncMessage>>,
) {
    info!("Spawning node!");
    let (network_name, select_chain, sync_tracker, resource_header_store, resource_validation) =
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
                    eff.send(
                        &downstream,
                        Tracked::Wrapped(ChainSyncEvent::RollForward {
                            peer: Peer::new(&msg.src),
                            point: Point::Specific(slot.into(), hash.into()),
                            raw_header: header.into(),
                            span: Span::current(),
                        }),
                    )
                    .await
                }
                ChainSyncMessage::Bck { slot, hash, .. } => {
                    eff.send(
                        &downstream,
                        Tracked::Wrapped(ChainSyncEvent::Rollback {
                            peer: Peer::new(&msg.src),
                            rollback_point: Point::Specific(slot.into(), hash.into()),
                            span: Span::current(),
                        }),
                    )
                    .await
                }
            }
            (downstream, output)
        },
    );

    let our_tip = HeaderTip::new(Point::Origin, 0);
    let receive_header_ref = build_stage_graph(select_chain, sync_tracker, our_tip, network);

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
    network
        .resources()
        .put::<ResourceNetworkOperations>(Arc::new(NetworkResource::new(
            [],
            &AcTokio::from_handle("upstream", rt.handle().clone()),
            0,
            resource_header_store,
        )));

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
/// FIXME: at some point we should implement a deterministic tie breaker when multiple best chains exist
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
    for statistic in statistics.display_as_lines() {
        info!("{}", statistic);
    }
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
