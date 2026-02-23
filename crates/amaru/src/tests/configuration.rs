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

use crate::stages::config::{Config, StoreType};
use crate::tests::configuration::NodeType::{NodeUnderTest, UpstreamNode};
use crate::tests::in_memory_connection_provider::InMemoryConnectionProvider;
use crate::tests::test_data::{create_transactions, create_transactions_in_mempool};
use amaru_consensus::headers_tree::data_generation::Action;
use amaru_kernel::cardano::network_block::make_encoded_block;
use amaru_kernel::{
    BlockHeader, IsHeader, MAINNET_ERA_HISTORY, NetworkName, PREPROD_ERA_HISTORY,
    PREPROD_INITIAL_PROTOCOL_PARAMETERS, PREVIEW_ERA_HISTORY, PREVIEW_INITIAL_PROTOCOL_PARAMETERS,
    Point, ProtocolParameters, TESTNET_ERA_HISTORY, Transaction,
};
use amaru_kernel::{EraHistory, Peer};
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros::{ChainStore, ConnectionsResource, TxId};
use amaru_stores::in_memory::MemoryStore;
use parking_lot::Mutex;
use pure_stage::trace_buffer::TraceBuffer;
use std::fmt::{Debug, Formatter};
use std::sync::Arc;

/// Configuration for running a test node:
///
///  - With a specific chain store and mempool.
///  - With a specific connections resource which could be implemented in memory or via TCP.
///  - The chain length is the length of the maximum chain that has been created when generated data
///  - If this configuration is used for the initiator, it also contains the address of the upstream peer to connect to (the responder).
///
#[derive(Clone)]
pub struct NodeTestConfig {
    pub chain_store: Arc<dyn ChainStore<BlockHeader>>,
    pub mempool: Arc<InMemoryMempool<Transaction>>,
    pub connections: ConnectionsResource,
    pub chain_length: usize,
    pub upstream_peers: Vec<Peer>,
    pub listen_address: String,
    pub mailbox_size: usize,
    pub trace_buffer: Arc<Mutex<TraceBuffer>>,
    pub seed: u64,
    pub actions: Vec<Action>,
    pub node_type: NodeType,
    pub network_name: NetworkName,
}

impl Debug for NodeTestConfig {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeTestConfig")
            .field("chain_length", &self.chain_length)
            .field("upstream_peers", &self.upstream_peers)
            .field("listen_address", &self.listen_address)
            .field("mailbox_size", &self.mailbox_size)
            .field("seed", &self.seed)
            .field("actions", &self.actions)
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
            upstream_peers: vec![Peer::new("127.0.0.1:3001")],
            listen_address: "127.0.0.1:3000".to_string(),
            mailbox_size: 10000,
            trace_buffer: Arc::new(Mutex::new(TraceBuffer::default())),
            seed: 42,
            actions: Vec::new(),
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

    pub fn era_history(&self) -> EraHistory {
        match self.network_name {
            NetworkName::Preprod => PREPROD_ERA_HISTORY.clone(),
            NetworkName::Preview => PREVIEW_ERA_HISTORY.clone(),
            NetworkName::Testnet(_) => TESTNET_ERA_HISTORY.clone(),
            NetworkName::Mainnet => MAINNET_ERA_HISTORY.clone(),
        }
    }

    /// TODO: define protocol parameters for all the networks
    #[expect(clippy::panic)]
    pub fn protocol_parameters(&self) -> ProtocolParameters {
        match self.network_name {
            NetworkName::Preprod => PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone(),
            NetworkName::Preview => PREVIEW_INITIAL_PROTOCOL_PARAMETERS.clone(),
            other @ NetworkName::Mainnet | other @ NetworkName::Testnet(_) => {
                panic!("no initial protocol parameters for {other}")
            }
        }
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

    pub fn with_actions(mut self, actions: Vec<Action>) -> Self {
        self.actions = actions;
        self
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
            self.chain_store
                .store_block(
                    &header.hash(),
                    &make_encoded_block(header, &self.era_history()),
                )
                .unwrap();
            self.chain_store
                .roll_forward_chain(&header.point())
                .unwrap();
        }

        if let Some(header) = headers.first() {
            tracing::info!("set the anchor to {}", header.point());
            self.chain_store.set_anchor_hash(&header.hash()).unwrap();
            tracing::info!("set the tip to {}", header.point());
            self.chain_store
                .set_best_chain_hash(&header.hash())
                .unwrap();
        }
        self
    }

    /// Create a node configuration from the simulation configuration.
    /// This sets the ledger and chain store + the upstream peer that is
    /// eventually used to initialize the HeadersTree for chain selection.
    pub fn make_node_configuration(&self) -> Config {
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
        let ledger_store = MemoryStore::new(self.era_history(), self.protocol_parameters());
        let chain_anchor = self
            .chain_store
            .load_header(&self.chain_store.get_anchor_hash())
            .map(|h| h.point())
            .unwrap_or(Point::Origin);
        ledger_store.set_tip(chain_anchor);

        config.ledger_store = StoreType::InMem(ledger_store);
        config.chain_store = StoreType::InMem(self.chain_store.clone());
        config
    }
}

pub const RESPONDER_BLOCKS_NB: usize = 10;
pub const INITIATOR_BLOCKS_NB: usize = 4;

pub const RESPONDER_TXS_NB: usize = 10;
pub const INITIATOR_TXS_NB: usize = 10;

/// By construction we return the same tx ids as the ones created in the function above
pub fn get_tx_ids() -> Vec<TxId> {
    create_transactions(RESPONDER_TXS_NB)
        .into_iter()
        .map(|tx| TxId::from(&tx))
        .collect()
}
