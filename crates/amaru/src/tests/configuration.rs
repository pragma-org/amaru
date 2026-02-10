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
use crate::tests::in_memory_connection_provider::InMemoryConnectionProvider;
use crate::tests::test_data::{create_transactions, create_transactions_in_mempool};
use amaru_kernel::cardano::network_block::make_network_block;
use amaru_kernel::utils::tests::run_strategy;
use amaru_kernel::{
    BlockHeader, HeaderHash, IsHeader, PREPROD_INITIAL_PROTOCOL_PARAMETERS, Point, Transaction,
    any_headers_chain_with_root, make_header,
};
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::{ChainStore, ConnectionsResource, TxId};
use amaru_slot_arithmetic::EraHistory;
use amaru_stores::in_memory::MemoryStore;
use parking_lot::Mutex;
use pure_stage::simulation::{EvalStrategy, Fifo, RandStdRng};
use pure_stage::trace_buffer::TraceBuffer;
use rand::SeedableRng;
use rand::prelude::StdRng;
use std::str::FromStr;
use std::sync::Arc;

/// Configuration for running a test node:
///  - With a specific chain store and mempool.
///  - With a specific connections resource which could be implemented in memory or via TCP.
///  - The chain length is the length of the maximum chain that has been created when generated data
///  - If this configuration is used for the initiator, it also contains the address of the upstream peer to connect to (the responder).
#[derive(Clone)]
pub struct NodeConfig {
    pub chain_store: Arc<dyn ChainStore<BlockHeader>>,
    pub mempool: Arc<InMemoryMempool<Transaction>>,
    pub connections: ConnectionsResource,
    pub chain_length: usize,
    pub upstream_peers: Vec<String>,
    pub mailbox_size: usize,
    pub trace_buffer: Arc<Mutex<TraceBuffer>>,
    pub eval_strategy: Arc<dyn EvalStrategy>,
}

impl Default for NodeConfig {
    fn default() -> Self {
        Self {
            chain_store: Arc::new(InMemConsensusStore::default()),
            mempool: Arc::new(InMemoryMempool::default()),
            connections: Arc::new(InMemoryConnectionProvider::default()),
            chain_length: 10,
            upstream_peers: vec!["127.0.0.1:3001".to_string()],
            mailbox_size: 10000,
            trace_buffer: Arc::new(Mutex::new(TraceBuffer::new(100, 1000))),
            eval_strategy: Arc::new(Fifo),
        }
    }
}

impl NodeConfig {
    pub fn initiator() -> Self {
        Self::default()
            .with_chain_length(INITIATOR_BLOCKS_NB)
            .with_txs(INITIATOR_TXS_NB)
            .with_upstream_peer("127.0.0.1:3000")
    }

    pub fn responder() -> Self {
        Self::default()
            .with_chain_length(RESPONDER_BLOCKS_NB)
            .with_txs(RESPONDER_TXS_NB)
            .with_no_upstream_peers()
    }

    pub fn with_no_upstream_peers(mut self) -> Self {
        self.upstream_peers = vec![];
        self
    }

    pub fn with_chain_length(mut self, chain_length: usize) -> Self {
        #[expect(clippy::unwrap_used)]
        initialize_chain_store(chain_length, self.chain_store.clone()).unwrap();
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

    pub fn with_eval_strategy(mut self, eval_strategy: impl EvalStrategy + 'static) -> Self {
        self.eval_strategy = Arc::new(eval_strategy);
        self
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.eval_strategy = Arc::new(RandStdRng(StdRng::seed_from_u64(seed)));
        self
    }

    pub fn with_txs(self, txs_nb: u64) -> Self {
        create_transactions_in_mempool(self.mempool.clone(), txs_nb);
        self
    }

    pub fn with_upstream_peer(mut self, upstream_peer: &str) -> Self {
        self.upstream_peers = vec![upstream_peer.to_string()];
        self
    }

    /// Create a node configuration from the simulation configuration.
    /// This sets the ledger and chain store + the upstream peer that is
    /// eventually used to initialize the HeadersTree for chain selection
    pub fn make_node_configuration(&self) -> Config {
        let mut config = Config {
            upstream_peers: self.upstream_peers.clone(),
            ..Default::default()
        };

        // Use 127.0.0.1 for in-memory tests so InMemoryConnectionProvider can match addresses.
        // The responder (no upstream peers) listens on port 3000.
        // The initiator (has upstream peers) listens on a different port to avoid conflict.
        let listen_port = if self.upstream_peers.is_empty() {
            3000
        } else {
            3001
        };
        config.listen_address = format!("127.0.0.1:{listen_port}");
        let era_history: &EraHistory = config.network.into();

        // Create the ledger store and set its tip to match the chain store's anchor.
        // This ensures that build_node's initialize_chain_store won't reset the
        // chain store's best_chain_hash (only the anchor will be set, which is already
        // the same as the ledger tip).
        let ledger_store = MemoryStore::new(
            era_history.clone(),
            PREPROD_INITIAL_PROTOCOL_PARAMETERS.clone(),
        );
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

/// Initialize the chain store with a chain of headers.
/// The responder chain is longer than the initiator chain to force the initiator to catch up.
fn initialize_chain_store(
    chain_length: usize,
    chain_store: Arc<dyn ChainStore<BlockHeader>>,
) -> anyhow::Result<()> {
    // Use the same root header for both initiator and responder
    let origin_hash: HeaderHash = amaru_kernel::Hash::from_str(
        "4df4505d862586f9e2c533c5fbb659f04402664db1b095aba969728abfb77301",
    )?;
    let root_header = BlockHeader::from(make_header(100_000_000, 100_000_000, Some(origin_hash)));
    chain_store.set_anchor_hash(&root_header.hash())?;
    let mut headers = run_strategy(any_headers_chain_with_root(
        chain_length - 1, // -1 since we already have the root header
        root_header.point(),
    ));
    headers.insert(0, root_header);

    for header in headers.iter() {
        chain_store.store_header(header)?;
        chain_store.roll_forward_chain(&header.point())?;
        chain_store.set_best_chain_hash(&header.hash())?;

        tracing::info!("storing block for header {}", header.point());
        let network_block = make_network_block(header);
        chain_store.store_block(&header.hash(), &network_block.raw_block())?;
    }
    Ok(())
}

pub const RESPONDER_TXS_NB: u64 = 10;
pub const INITIATOR_TXS_NB: u64 = 10;

/// By construction we return the same tx ids as the ones created in the function above
pub fn get_tx_ids() -> Vec<TxId> {
    create_transactions(RESPONDER_TXS_NB)
        .into_iter()
        .map(|tx| TxId::from(&tx))
        .collect()
}
