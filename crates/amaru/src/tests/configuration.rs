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

use std::{
    fmt::{Debug, Formatter},
    sync::Arc,
};

use amaru_consensus::headers_tree::data_generation::Action;
use amaru_kernel::{
    BlockHeader, EraHistory, IsHeader, NetworkName, Peer, Point, ProtocolParameters, Transaction,
    cardano::network_block::make_encoded_block,
};
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros::{ChainStore, ConnectionsResource, TxId, in_memory_consensus_store::InMemConsensusStore};
use amaru_stores::in_memory::MemoryStore;
use anyhow::anyhow;
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;

use crate::{
    stages::config::{Config, StoreType},
    tests::{
        configuration::NodeType::{NodeUnderTest, UpstreamNode},
        in_memory_connection_provider::InMemoryConnectionProvider,
        test_data::{create_transactions, create_transactions_in_mempool},
    },
};

/// Configuration for running a test node:
///
///  - With a specific chain store and mempool.
///  - With a specific connections resource which could be implemented in memory or via TCP.
///  - The chain length is the length of the maximum chain that has been created when generated data
///  - With some transactions already present in the node mempool
///  - If this configuration is used for the initiator, it also contains the address of the upstream peer to connect to (the responder).
///
#[derive(Clone)]
pub struct NodeTestConfig {
    pub chain_store: Arc<dyn ChainStore<BlockHeader>>,
    pub mempool: Arc<InMemoryMempool<Transaction>>,
    pub connections: ConnectionsResource,
    pub chain_length: usize,
    pub number_of_transactions: usize,
    pub upstream_peers: Vec<Peer>,
    pub listen_address: String,
    pub mailbox_size: usize,
    pub trace_buffer: Arc<Mutex<TraceBuffer>>,
    pub seed: u64,
    pub events: Vec<SimulationEvent>,
    pub node_type: NodeType,
    pub network_name: NetworkName,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SimulationEvent {
    PeerAction(Action),
    InjectTx(Transaction),
}

impl Debug for NodeTestConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeTestConfig")
            .field("chain_length", &self.chain_length)
            .field("number_of_transactions", &self.number_of_transactions)
            .field("upstream_peers", &self.upstream_peers)
            .field("listen_address", &self.listen_address)
            .field("mailbox_size", &self.mailbox_size)
            .field("seed", &self.seed)
            .field("events", &self.events)
            .field("node_type", &self.node_type)
            .finish()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeType {
    UpstreamNode,
    NodeUnderTest,
    DownstreamNode,
}

impl Default for NodeTestConfig {
    fn default() -> Self {
        Self {
            chain_store: Arc::new(InMemConsensusStore::default()),
            mempool: Arc::new(InMemoryMempool::default()),
            connections: Arc::new(InMemoryConnectionProvider::default()),
            chain_length: 10,
            number_of_transactions: 10,
            upstream_peers: vec![Peer::new("127.0.0.1:3001")],
            listen_address: "127.0.0.1:3000".to_string(),
            mailbox_size: 10000,
            trace_buffer: Arc::new(Mutex::new(TraceBuffer::default())),
            seed: 42,
            events: Vec::new(),
            network_name: NetworkName::Preprod,
            node_type: NodeUnderTest,
        }
    }
}

impl NodeTestConfig {
    /// Enter a tracing span with this node's identifier for logging purposes.
    pub fn enter_span(&self) -> tracing::span::EnteredSpan {
        tracing::info_span!("node", id = %self.listen_address).entered()
    }

    pub fn initiator() -> Self {
        Self::default()
            .with_chain_length(INITIATOR_BLOCKS_NB)
            .with_txs(INITIATOR_TXS_NB)
            .with_upstream_peer(Peer::new("127.0.0.1:3001"))
            .with_listen_address("127.0.0.1:3000")
            .with_node_type(NodeUnderTest)
    }

    pub fn responder() -> Self {
        Self::default()
            .with_chain_length(RESPONDER_BLOCKS_NB)
            .with_txs(RESPONDER_TXS_NB)
            .with_no_upstream_peers()
            .with_listen_address("127.0.0.1:3001")
            .with_node_type(UpstreamNode)
    }

    pub fn era_history(&self) -> &EraHistory {
        self.network_name.into()
    }

    pub fn protocol_parameters(&self) -> anyhow::Result<&ProtocolParameters> {
        self.network_name.try_into().map_err(|e: String| anyhow!(e))
    }

    pub fn with_no_upstream_peers(mut self) -> Self {
        self.upstream_peers = vec![];
        self
    }

    pub fn with_listen_address(mut self, listen_address: &str) -> Self {
        self.listen_address = listen_address.to_string();
        self
    }

    pub fn with_chain_length(mut self, chain_length: usize) -> Self {
        self.chain_length = chain_length;
        self
    }

    pub fn with_chain_store(mut self, chain_store: Arc<dyn ChainStore<BlockHeader>>) -> Self {
        self.chain_store = chain_store;
        self
    }

    pub fn with_mempool(mut self, mempool: Arc<InMemoryMempool<Transaction>>) -> Self {
        self.mempool = mempool;
        self
    }

    pub fn with_connections(mut self, connections: ConnectionsResource) -> Self {
        self.connections = connections;
        self
    }

    pub fn with_mailbox_size(mut self, size: usize) -> Self {
        self.mailbox_size = size;
        self
    }

    pub fn with_trace_buffer(mut self, trace_buffer: Arc<Mutex<TraceBuffer>>) -> Self {
        self.trace_buffer = trace_buffer;
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.seed = seed;
        self
    }

    pub fn with_network_name(mut self, network_name: NetworkName) -> Self {
        self.network_name = network_name;
        self
    }

    pub fn with_txs(self, txs_nb: usize) -> Self {
        create_transactions_in_mempool(self.mempool.clone(), txs_nb);
        self
    }

    pub fn with_upstream_peer(mut self, upstream_peer: Peer) -> Self {
        self.upstream_peers = vec![upstream_peer];
        self
    }

    pub fn with_upstream_peers(mut self, upstream_peers: Vec<Peer>) -> Self {
        self.upstream_peers = upstream_peers;
        self
    }

    pub fn with_events(mut self, events: Vec<SimulationEvent>) -> Self {
        self.events = events;
        self
    }

    pub fn with_actions(self, actions: Vec<Action>) -> Self {
        self.with_peer_actions(actions)
    }

    pub fn with_peer_actions(self, actions: Vec<Action>) -> Self {
        self.with_events(actions.into_iter().map(SimulationEvent::PeerAction).collect())
    }

    pub fn upstream_peers(&self) -> Vec<Peer> {
        self.upstream_peers.clone()
    }

    pub fn with_node_type(mut self, node_type: NodeType) -> Self {
        self.node_type = node_type;
        self
    }

    /// Given a list of block headers:
    ///
    /// - Store them in the chain store.
    /// - Create and store blocks for them.
    /// - Declare that list of header as the best chain.
    /// - Set the chain anchor and best tip to the first header of the chain.
    ///
    #[expect(clippy::unwrap_used)]
    pub fn with_validated_blocks(self, headers: Vec<BlockHeader>) -> Self {
        let _span = self.enter_span();
        for header in headers.iter() {
            tracing::info!(
                "storing block for header {} (parent hash: {})",
                header.point(),
                header.parent_hash().unwrap_or(Point::Origin.hash())
            );
            self.chain_store.store_header(header).unwrap();
            self.chain_store.store_block(&header.hash(), &make_encoded_block(header, self.era_history())).unwrap();
            self.chain_store.roll_forward_chain(&header.point()).unwrap();
        }

        if let Some(header) = headers.first() {
            tracing::info!("set the anchor to {}", header.point());
            self.chain_store.set_anchor_hash(&header.hash()).unwrap();
            tracing::info!("set the tip to {}", header.point());
            self.chain_store.set_best_chain_hash(&header.hash()).unwrap();
        }
        self
    }

    /// Create a node configuration from the simulation configuration.
    /// This sets the ledger and chain store + the upstream peer that is
    /// eventually used to initialize the HeadersTree for chain selection.
    pub fn make_node_configuration(&self) -> anyhow::Result<Config> {
        let mut config = Config {
            upstream_peers: self.upstream_peers.iter().map(|p| p.name.clone()).collect(),
            network: self.network_name,
            network_magic: self.network_name.to_network_magic(),
            ..Default::default()
        };

        config.listen_address = self.listen_address.clone();

        // Create the ledger store and set its tip to match the chain store's anchor.
        // This ensures that build_node's initialize_chain_store won't reset the
        // chain store's best_chain_hash (only the anchor will be set, which is already
        // the same as the ledger tip).
        let ledger_store = MemoryStore::new(self.era_history().clone(), self.protocol_parameters()?.clone());
        let chain_anchor = self
            .chain_store
            .load_header(&self.chain_store.get_anchor_hash())
            .map(|h| h.point())
            .unwrap_or(Point::Origin);
        ledger_store.set_tip(chain_anchor);

        config.ledger_store = StoreType::InMem(ledger_store);
        config.chain_store = StoreType::InMem(self.chain_store.clone());
        Ok(config)
    }

    /// Return the list of transaction ids for the transations held by the node at the beginning of
    /// the run
    pub fn transaction_ids(&self) -> Vec<TxId> {
        let mut result = vec![];
        for event in self.events.iter() {
            if let SimulationEvent::InjectTx(tx) = event {
                result.push(TxId::from(&tx))
            }
        }
        result
    }
}

pub const RESPONDER_BLOCKS_NB: usize = 10;
pub const INITIATOR_BLOCKS_NB: usize = 4;

pub const RESPONDER_TXS_NB: usize = 10;
pub const INITIATOR_TXS_NB: usize = 10;

/// By construction we return the same tx ids as the ones created in the function above
pub fn get_tx_ids() -> Vec<TxId> {
    create_transactions(RESPONDER_TXS_NB).into_iter().map(|tx| TxId::from(&tx)).collect()
}
