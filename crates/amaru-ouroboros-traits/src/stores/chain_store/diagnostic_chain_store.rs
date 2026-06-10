// Copyright 2026 PRAGMA
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

use std::{iter::successors, sync::Arc};

use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, RawBlock};

use crate::{Nonces, ReadChainStore};

/// A chain store interface that also exposes diagnostic methods to load raw data.
/// It should not be used by the consensus stages since it might load lots of data at once.
pub trait DiagnosticChainStore: ReadChainStore {
    /// Load all headers in the store.
    ///
    /// NOTE: This can be very expensive for large stores and is only
    /// used for diagnostics and testing purposes.
    fn load_headers(&self) -> Box<dyn Iterator<Item = BlockHeader> + '_>;

    /// Load all nonces in the store.
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_>;
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_>;
    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_>;

    fn ancestors_with_validity<'a>(
        &'a self,
        start: HeaderHash,
    ) -> Box<dyn Iterator<Item = (BlockHeader, Option<bool>)> + 'a> {
        let anchor_point = self.get_anchor_tip().point();

        let header_opt = self.load_header_with_validity(&start);

        Box::new(successors(header_opt, move |(h, _valid)| {
            if h.slot() <= anchor_point.slot_or_default() {
                None
            } else {
                h.parent().and_then(|p| self.load_header_with_validity(&p))
            }
        }))
    }

    /// Return the ancestors of the header, including the header itself.
    /// Stop if the followed chain reaches past the anchor.
    fn ancestors<'a>(&'a self, start: BlockHeader) -> Box<dyn Iterator<Item = BlockHeader> + 'a> {
        let anchor_point = self.get_anchor_tip().point();

        Box::new(successors(Some(start), move |h| {
            if h.slot() <= anchor_point.slot_or_default() {
                None
            } else {
                h.parent().and_then(|p| self.load_header(&p))
            }
        }))
    }

    /// Return the hashes of the ancestors of the header, including the header hash itself.
    fn ancestors_hashes<'a>(&'a self, hash: &HeaderHash) -> Box<dyn Iterator<Item = HeaderHash> + 'a> {
        if let Some(header) = self.load_header(hash) {
            Box::new(self.ancestors(header).map(|h| h.hash()))
        } else {
            Box::new(vec![*hash].into_iter())
        }
    }

    /// Return the hashes of the best chain fragment, starting from the anchor.
    fn retrieve_best_chain(&self) -> Vec<HeaderHash> {
        let anchor = self.get_anchor_hash();
        let mut best_chain = vec![];
        let mut current_hash = self.get_best_chain_hash();
        while let Some(header) = self.load_header(&current_hash) {
            best_chain.push(current_hash);
            if header.hash() != anchor
                && let Some(parent) = header.parent()
            {
                current_hash = parent;
            } else {
                break;
            }
        }
        best_chain.reverse();
        best_chain
    }

    /// Retrieve all blocks from the chain store starting from the anchor to the best chain tip.
    #[expect(clippy::expect_used)]
    fn get_blocks(&self) -> Vec<(HeaderHash, amaru_kernel::Block)> {
        self.retrieve_best_chain()
            .iter()
            .map(|h| {
                let b = self
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
    fn get_best_chain_block_headers(&self) -> Vec<BlockHeader> {
        self.retrieve_best_chain()
            .iter()
            .map(|h| self.load_header(h).expect("missing header for the best chain"))
            .collect()
    }
}

impl<T: DiagnosticChainStore + ?Sized> DiagnosticChainStore for Arc<T> {
    fn load_headers(&self) -> Box<dyn Iterator<Item = BlockHeader> + '_> {
        self.as_ref().load_headers()
    }

    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_> {
        self.as_ref().load_nonces()
    }

    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_> {
        self.as_ref().load_blocks()
    }

    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_> {
        self.as_ref().load_parents_children()
    }
}
