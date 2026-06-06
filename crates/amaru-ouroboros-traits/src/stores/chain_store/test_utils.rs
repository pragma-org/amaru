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
use amaru_kernel::{BlockHeader, HeaderHash};

use crate::ChainStore;

/// Retrieve all blocks from the chain store starting from the anchor to the best chain tip.
#[expect(clippy::expect_used)]
pub fn get_blocks(store: std::sync::Arc<dyn ChainStore<BlockHeader>>) -> Vec<(HeaderHash, amaru_kernel::Block)> {
    store
        .retrieve_best_chain()
        .iter()
        .map(|h| {
            let b = store
                .load_block(h)
                .expect("load_block should not raise an error")
                .expect("missing block for a header on the best chain");
            (
                *h,
                amaru_kernel::cardano::network_block::NetworkBlock::try_from(b)
                    .expect("failed to decode raw block")
                    .decode_block()
                    .expect("failed to decode block"),
            )
        })
        .collect()
}

/// Retrieve all blocks headers from the chain store starting from anchor to the best chain tip.
#[expect(clippy::expect_used)]
pub fn get_best_chain_block_headers(store: std::sync::Arc<dyn ChainStore<BlockHeader>>) -> Vec<BlockHeader> {
    store
        .retrieve_best_chain()
        .iter()
        .map(|h| store.load_header(h).expect("missing header for the best chain"))
        .collect()
}
