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

use crate::store_effects::ResourceHeaderStore;
use crate::tests::configuration::get_tx_ids;
use amaru_kernel::cardano::network_block::NetworkBlock;
use amaru_kernel::utils::string::ListToString;
use amaru_kernel::{Block, BlockHeader, HeaderHash, Transaction};
use amaru_ouroboros_traits::{ChainStore, ResourceMempool};
use pure_stage::tokio::TokioRunning;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Notify;

/// Wait until both nodes signal that they are done.
/// We timeout after a bit to avoid hanging tests. In that case the test will fail later when checking the state.
pub(super) async fn wait_for_termination(
    responder_done: Arc<Notify>,
    initiator_done: Arc<Notify>,
) -> anyhow::Result<()> {
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        tokio::join!(responder_done.notified(), initiator_done.notified());
    })
    .await;
    Ok(())
}

/// Verify that both nodes have the same state: same best chain, same blocks, same transactions.
pub(super) fn check_state(initiator: TokioRunning, responder: TokioRunning) -> anyhow::Result<()> {
    let responder_chain_store = responder.resources().get::<ResourceHeaderStore>()?.clone();
    let responder_mempool = responder
        .resources()
        .get::<ResourceMempool<Transaction>>()?
        .clone();

    let initiator_chain_store = initiator.resources().get::<ResourceHeaderStore>()?.clone();
    let initiator_mempool = initiator
        .resources()
        .get::<ResourceMempool<Transaction>>()?
        .clone();

    // Verify that the 2 nodes have the same best chain
    let initiator_best_chain = initiator_chain_store
        .ancestors_hashes(&initiator_chain_store.get_best_chain_hash())
        .collect::<Vec<_>>();
    let responder_best_chain = responder_chain_store
        .ancestors_hashes(&responder_chain_store.get_best_chain_hash())
        .collect::<Vec<_>>();
    assert_eq!(initiator_best_chain, responder_best_chain,);

    // Verify that the 2 nodes have the same blocks headers
    // This makes it easier to spot differences in case of failure, before comparing full blocks.
    let initiator_block_headers = initiator_chain_store.retrieve_best_chain();
    let responder_block_headers = responder_chain_store.retrieve_best_chain();
    assert_eq!(initiator_block_headers, responder_block_headers);

    let initiator_blocks = get_blocks(initiator_chain_store);
    let responder_blocks = get_blocks(responder_chain_store);

    assert_eq!(
        initiator_blocks,
        responder_blocks,
        "initiator blocks {:?}\nresponder blocks {:?}",
        initiator_blocks
            .iter()
            .map(|b| b.0)
            .collect::<Vec<_>>()
            .list_to_string(","),
        responder_blocks
            .iter()
            .map(|b| b.0)
            .collect::<Vec<_>>()
            .list_to_string(",")
    );

    // Verify that the 2 nodes have the same transactions
    let tx_ids = get_tx_ids();
    assert_eq!(
        responder_mempool.get_txs_for_ids(tx_ids.as_slice()),
        initiator_mempool.get_txs_for_ids(tx_ids.as_slice())
    );
    Ok(())
}

/// Retrieve all blocks from the chain store starting from the best chain tip down to the root.
pub(super) fn get_blocks(store: Arc<dyn ChainStore<BlockHeader>>) -> Vec<(HeaderHash, Block)> {
    store
        .retrieve_best_chain()
        .iter()
        .map(|h| {
            let b = store
                .load_block(h)
                .unwrap()
                .expect("missing block for best-chain header");
            (
                *h,
                NetworkBlock::try_from(b)
                    .expect("failed to decode raw block")
                    .decode_block()
                    .expect("failed to decode block"),
            )
        })
        .collect()
}
