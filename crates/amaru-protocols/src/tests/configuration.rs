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
use amaru_kernel::is_header::tests::{any_headers_chain_with_root, make_header, run};
use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, RawBlock, cbor, to_cbor};
use amaru_mempool::InMemoryMempool;
use amaru_ouroboros_traits::in_memory_consensus_store::InMemConsensusStore;
use amaru_ouroboros_traits::{ChainStore, TxId};
use pallas_primitives::conway::{Block, MintedBlock, Tx};
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
    pub(super) mempool: Arc<InMemoryMempool<Tx>>,
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

    #[allow(dead_code)]
    pub(super) fn with_chain_store(
        mut self,
        chain_store: Arc<dyn ChainStore<BlockHeader>>,
    ) -> Self {
        self.chain_store = chain_store;
        self
    }

    #[allow(dead_code)]
    pub(super) fn with_mempool(mut self, mempool: Arc<InMemoryMempool<Tx>>) -> Self {
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
    let root_header = BlockHeader::from(make_header(0, 0, Some(origin_hash)));
    chain_store.set_anchor_hash(&root_header.hash())?;
    let mut headers = run(any_headers_chain_with_root(
        chain_length - 1, // -1 since we already have the root header
        root_header.hash(),
    ));
    headers.insert(0, root_header);

    for header in headers.iter() {
        chain_store.store_header(header)?;
        chain_store.roll_forward_chain(&header.point())?;
        chain_store.set_best_chain_hash(&header.hash())?;

        tracing::info!("storing block for header {}", header.point());
        let mut block = make_block();
        block.header = header.header().clone();
        chain_store.store_block(&header.hash(), &RawBlock::from(to_cbor(&block).as_slice()))?;
    }
    Ok(())
}

/// Create a block from the Conway3.block test data (see https://github.com/txpipe/pallas/blob/main/test_data/conway3.block)
fn make_block() -> Block {
    let bytes = hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex");
    let (_, block): (u16, MintedBlock<'_>) =
        cbor::decode(bytes.as_slice()).expect("Failed to parse Conway3.block bytes");
    Block::from(block)
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
