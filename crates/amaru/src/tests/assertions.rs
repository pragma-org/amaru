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

use crate::tests::configuration::get_tx_ids;
use amaru_kernel::Transaction;
use amaru_kernel::utils::string::ListToString;
use amaru_ouroboros::{ResourceMempool, get_blocks};
use amaru_protocols::store_effects::ResourceHeaderStore;
use pure_stage::Resources;

/// Verify that both nodes have the same state: same best chain, same blocks, same transactions.
pub fn check_state(
    initiator_resources: &Resources,
    responder_resources: &Resources,
) -> anyhow::Result<()> {
    let responder_chain_store = responder_resources.get::<ResourceHeaderStore>()?.clone();
    let responder_mempool = responder_resources
        .get::<ResourceMempool<Transaction>>()?
        .clone();

    let initiator_chain_store = initiator_resources.get::<ResourceHeaderStore>()?.clone();
    let initiator_mempool = initiator_resources
        .get::<ResourceMempool<Transaction>>()?
        .clone();

    // Verify that the 2 nodes have the same best chain
    let initiator_best_chain = initiator_chain_store
        .ancestors_hashes(&initiator_chain_store.get_best_chain_hash())
        .collect::<Vec<_>>();
    let responder_best_chain = responder_chain_store
        .ancestors_hashes(&responder_chain_store.get_best_chain_hash())
        .collect::<Vec<_>>();
    assert_eq!(initiator_best_chain, responder_best_chain);

    // Verify that the 2 nodes have the same blocks headers
    // This makes it easier to spot differences in case of failure, before comparing full blocks.
    let initiator_block_headers = initiator_chain_store.retrieve_best_chain();
    let responder_block_headers = responder_chain_store.retrieve_best_chain();
    assert_eq!(initiator_block_headers, responder_block_headers);

    let initiator_blocks = get_blocks(initiator_chain_store);
    let responder_blocks = get_blocks(responder_chain_store);
    assert!(
        !initiator_blocks.is_empty(),
        "the initiator should have blocks"
    );

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
