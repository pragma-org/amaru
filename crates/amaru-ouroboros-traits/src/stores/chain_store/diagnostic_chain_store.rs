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

use amaru_kernel::{Epoch, Header, HeaderHash, IsHeader, Nonce, PoolId, RawBlock, Slot};

use crate::{BaseReadChainStore, Nonces, ReadChainStore};

/// A chain store interface that also exposes diagnostic methods to load raw data.
/// It should not be used by the consensus stages since it might load lots of data at once.
pub trait DiagnosticChainStore: ReadChainStore {
    /// Load all headers in the store.
    ///
    /// NOTE: This can be very expensive for large stores and is only
    /// used for diagnostics and testing purposes.
    fn load_headers(&self) -> Box<dyn Iterator<Item = Header> + '_>;

    /// Load all nonces in the store.
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_>;
    fn load_opcert_sequence_numbers(&self) -> Box<dyn Iterator<Item = (PoolId, Slot, HeaderHash, u64)> + '_>;
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_>;
    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_>;

    fn ancestors_with_validity<'a>(
        &'a self,
        start: HeaderHash,
    ) -> Box<dyn Iterator<Item = (Header, Option<bool>)> + 'a> {
        let anchor_point = self.get_anchor_point();

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
    fn ancestors<'a>(&'a self, start: Header) -> Box<dyn Iterator<Item = Header> + 'a> {
        let anchor_point = self.get_anchor_point();

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
    fn get_best_chain_block_headers(&self) -> Vec<Header> {
        self.retrieve_best_chain()
            .iter()
            .map(|h| self.load_header(h).expect("missing header for the best chain"))
            .collect()
    }
}

impl<T: DiagnosticChainStore + ?Sized> DiagnosticChainStore for Arc<T> {
    fn load_headers(&self) -> Box<dyn Iterator<Item = Header> + '_> {
        self.as_ref().load_headers()
    }

    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_> {
        self.as_ref().load_nonces()
    }

    fn load_opcert_sequence_numbers(&self) -> Box<dyn Iterator<Item = (PoolId, Slot, HeaderHash, u64)> + '_> {
        self.as_ref().load_opcert_sequence_numbers()
    }

    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_> {
        self.as_ref().load_blocks()
    }

    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_> {
        self.as_ref().load_parents_children()
    }
}

/// Result of looking up the nonce used to compute leadership for an epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceForEpoch {
    /// No nonce is available for the requested epoch.
    None,
    /// The nonce was recorded by the chain store and is authoritative.
    Some(Nonce),
    /// The nonce was derived for the next epoch before that epoch was observed on-chain.
    Speculative(Nonce),
}

/// Find the nonce used to compute leadership for an epoch from the best chain in a store.
///
/// A stored nonce is authoritative for epochs already represented by the chain. The following
/// epoch can be derived from the stored tail and candidate nonce when the chain has not yet
/// stored a header in that epoch.
pub fn nonce_for_epoch<T>(chain: &T, epoch: Epoch) -> NonceForEpoch
where
    T: BaseReadChainStore,
{
    let tip = chain.get_best_chain_tip();
    let mut current = tip.hash();
    while let Some(header) = chain.load_header(&current) {
        if let Some(nonces) = chain.get_nonces(&current) {
            if nonces.epoch == epoch {
                return NonceForEpoch::Some(nonces.active);
            }

            if nonces.epoch < epoch {
                break;
            }
        }

        let Some(parent) = header.parent() else { break };
        current = parent;
    }

    // Nonces are only fixed at the epoch boundary, since we need the parent of the last epoch's
    // block. There's one case where the loop above may exit, but where we may still be able to
    // figure out the nonce: it is when the last block of an epoch has been processed, but we
    // haven't yet seen the first block of the next epoch.
    //
    // That case is interesting because, one may want to know the leader schedule as soon as it is
    // available, and not only after a first block has been diffused! We therefore return the result
    // as speculative here.

    let Some(nonces) = chain.get_nonces(&tip.hash()) else {
        return NonceForEpoch::None;
    };

    if epoch != nonces.epoch + 1 {
        // Asking for an epoch that is too far in the future.
        return NonceForEpoch::None;
    }

    match chain.load_header(&nonces.tail).and_then(|header| header.parent()) {
        Some(grandparent) => NonceForEpoch::Speculative(nonces.next_active(grandparent)),
        None => NonceForEpoch::None,
    }
}
