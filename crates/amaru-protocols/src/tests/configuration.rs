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

use crate::tx_submission::{create_transactions, create_transactions_in_mempool};
use amaru_kernel::cardano::network_block::make_network_block;
use amaru_kernel::utils::tests::run_strategy;
use amaru_kernel::{
    BlockHeader, HeaderHash, IsHeader, Transaction, any_headers_chain_with_root, make_header,
};
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::{ChainStore, TxId};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

/// Configuration for running 2 test nodes, initiator and responder communicating over TCP:
///  - They both have their own chain store and mempool.
///  - They either listen to a specific address (responder) or connect to a specific address (initiator).
///  - They can have different chain lengths and number of transactions in their mempool.
///  - They can have different timeouts and processing waits to simulate slow peers.
#[derive(Clone)]
pub(super) struct Configuration {
    pub(super) chain_store: Arc<dyn ChainStore<BlockHeader>>,
    pub(super) mempool: Arc<InMemoryMempool<Transaction>>,
    pub(super) addr: SocketAddr,
    pub(super) connection_timeout: Duration,
    pub(super) processing_wait: Option<Duration>,
    pub(super) chain_length: usize,
    pub(super) slow_manager: bool,
}

impl Configuration {
    pub(super) fn initiator() -> Self {
        let initiator = Self {
            chain_store: Arc::new(InMemConsensusStore::default()),
            chain_length: 0,
            mempool: Arc::new(InMemoryMempool::default()),
            addr: SocketAddr::from(([127, 0, 0, 1], 3000)),
            connection_timeout: Duration::from_secs(1),
            processing_wait: None,
            slow_manager: false,
        };
        initiator
            .with_best_chain_of_length(INITIATOR_BLOCKS_NB)
            .with_txs(INITIATOR_TXS_NB)
    }

    pub(super) fn responder() -> Self {
        let responder = Self {
            chain_store: Arc::new(InMemConsensusStore::default()),
            chain_length: 0,
            mempool: Arc::new(InMemoryMempool::default()),
            addr: SocketAddr::from(([127, 0, 0, 1], 0)),
            connection_timeout: Duration::from_secs(1),
            processing_wait: None,
            slow_manager: false,
        };
        responder
            .with_best_chain_of_length(RESPONDER_BLOCKS_NB)
            .with_txs(RESPONDER_TXS_NB)
    }

    pub(super) fn with_best_chain_of_length(mut self, chain_length: usize) -> Self {
        initialize_chain_store(chain_length, self.chain_store.clone()).unwrap();
        self.chain_length = chain_length;
        self
    }

    #[expect(dead_code)]
    pub(super) fn with_chain_store(
        mut self,
        chain_store: Arc<dyn ChainStore<BlockHeader>>,
    ) -> Self {
        self.chain_store = chain_store;
        self
    }

    #[expect(dead_code)]
    pub(super) fn with_mempool(mut self, mempool: Arc<InMemoryMempool<Transaction>>) -> Self {
        self.mempool = mempool;
        self
    }

    pub(super) fn with_txs(self, txs_nb: u64) -> Self {
        create_transactions_in_mempool(self.mempool.clone(), txs_nb);
        self
    }

    pub(super) fn with_addr(mut self, addr: SocketAddr) -> Self {
        self.addr = addr;
        self
    }

    pub(super) fn with_slow_manager(mut self) -> Self {
        self.slow_manager = true;
        self
    }

    pub(super) fn with_connection_timeout(mut self, timeout: Duration) -> Self {
        self.connection_timeout = timeout;
        self
    }

    pub(super) fn with_processing_wait(mut self, wait: Duration) -> Self {
        self.processing_wait = Some(wait);
        self
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
pub(super) fn get_tx_ids() -> Vec<TxId> {
    create_transactions(RESPONDER_TXS_NB)
        .into_iter()
        .map(|tx| TxId::from(&tx))
        .collect()
}
