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

pub mod in_memory_consensus_store;
pub mod overriding_consensus_store;

use std::{
    fmt::Display,
    iter::{self, successors},
};

use amaru_kernel::{BlockHeader, HeaderHash, IsHeader, ORIGIN_HASH, Point, RawBlock, Tip};
use thiserror::Error;

use crate::Nonces;

pub trait ReadOnlyChainStore<H>
where
    H: IsHeader,
{
    /// Try to load a header by its hash.
    fn load_header(&self, hash: &HeaderHash) -> Option<H>;

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(H, Option<bool>)>;

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash>;
    fn get_anchor_hash(&self) -> HeaderHash;
    fn get_best_chain_hash(&self) -> HeaderHash;

    /// Load a `HeaderHash` from the best chain.
    /// Returns `None` if the point is not in the best chain.
    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash>;

    /// Return the next `Point` on the best chain following given
    /// `Point`, if it exists.
    fn next_best_chain(&self, point: &Point) -> Option<Point>;

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError>;
    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces>;
    fn has_header(&self, hash: &HeaderHash) -> bool;

    /// Retrieve the tip of a block header given its hash.
    fn load_tip(&self, hash: &HeaderHash) -> Option<Tip> {
        if hash == &ORIGIN_HASH {
            return Some(Tip::origin());
        }
        self.load_header(hash).map(|h| h.tip())
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

    /// Return the ancestors of the header, including the header itself.
    /// Stop if the followed chain reaches past the anchor.
    fn ancestors<'a>(&'a self, start: H) -> Box<dyn Iterator<Item = H> + 'a>
    where
        H: 'a,
    {
        let anchor = self.get_anchor_hash();
        let anchor_point = match self.load_header(&anchor) {
            Some(header) => header.point(),
            None => Point::Origin,
        };

        Box::new(successors(Some(start), move |h| {
            if h.slot() <= anchor_point.slot_or_default() {
                None
            } else {
                h.parent().and_then(|p| self.load_header(&p))
            }
        }))
    }

    fn ancestors_with_validity<'a>(&'a self, start: HeaderHash) -> Box<dyn Iterator<Item = (H, Option<bool>)> + 'a>
    where
        H: 'a,
    {
        let anchor = self.get_anchor_hash();
        let anchor_point = match self.load_header(&anchor) {
            Some(header) => header.point(),
            None => Point::Origin,
        };

        let header_opt = self.load_header_with_validity(&start);

        Box::new(successors(header_opt, move |(h, _valid)| {
            if h.slot() <= anchor_point.slot_or_default() {
                None
            } else {
                h.parent().and_then(|p| self.load_header_with_validity(&p))
            }
        }))
    }

    /// Return the hashes of the ancestors of the header, including the header hash itself.
    fn ancestors_hashes<'a>(&'a self, hash: &HeaderHash) -> Box<dyn Iterator<Item = HeaderHash> + 'a>
    where
        H: 'a,
    {
        if let Some(header) = self.load_header(hash) {
            Box::new(self.ancestors(header).map(|h| h.hash()))
        } else {
            Box::new(vec![*hash].into_iter())
        }
    }

    fn child_tips<'a>(&'a self, hash: &HeaderHash) -> Box<dyn Iterator<Item = Tip> + 'a>
    where
        H: 'a,
    {
        let mut to_visit = if hash == &ORIGIN_HASH { self.get_children(hash) } else { vec![*hash] };
        Box::new(iter::from_fn(move || {
            loop {
                let hash = to_visit.pop()?;
                tracing::debug!(hash = %hash, "visiting child");
                let (header, validity) = self.load_header_with_validity(&hash)?;
                if validity == Some(false) {
                    continue;
                }
                let children = self.get_children(&hash);
                to_visit.extend(children);
                return Some(header.tip());
            }
        }))
    }
}

/// A chain store interface that exposes diagnostic methods to load raw data.
pub trait DiagnosticChainStore {
    /// Load all headers in the store.
    ///
    /// NOTE: This can be very expensive for large stores and is only
    /// used for diagnostics and testing purposes.
    fn load_headers(&self) -> Box<dyn Iterator<Item = BlockHeader> + '_>;

    /// Load all nonces in the store.
    fn load_nonces(&self) -> Box<dyn Iterator<Item = (HeaderHash, Nonces)> + '_>;
    fn load_blocks(&self) -> Box<dyn Iterator<Item = (HeaderHash, RawBlock)> + '_>;
    fn load_parents_children(&self) -> Box<dyn Iterator<Item = (HeaderHash, Vec<HeaderHash>)> + '_>;
}

impl<H: IsHeader> ReadOnlyChainStore<H> for Box<dyn ChainStore<H>> {
    fn load_header(&self, hash: &HeaderHash) -> Option<H> {
        self.as_ref().load_header(hash)
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(H, Option<bool>)> {
        self.as_ref().load_header_with_validity(hash)
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        self.as_ref().get_children(hash)
    }

    fn get_anchor_hash(&self) -> HeaderHash {
        self.as_ref().get_anchor_hash()
    }

    fn get_best_chain_hash(&self) -> HeaderHash {
        self.as_ref().get_best_chain_hash()
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        self.as_ref().load_block(hash)
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        self.as_ref().get_nonces(header)
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        self.as_ref().has_header(hash)
    }

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        self.as_ref().load_from_best_chain(point)
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        self.as_ref().next_best_chain(point)
    }
}

/// A simple chain store interface that can store and retrieve headers indexed by their hash.
pub trait ChainStore<H>: ReadOnlyChainStore<H> + Send + Sync
where
    H: IsHeader,
{
    fn store_header(&self, header: &H) -> Result<(), StoreError>;

    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError>;

    fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> Result<(), StoreError>;

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError>;

    /// Roll forward the best chain to the given point.
    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError>;

    /// Rollback the best chain tip at the given point.
    /// The point must exist on the best chain, and all points on chain after that
    /// point will be deleted.
    /// Returns the number of headers that were rolled back.
    fn rollback_chain(&self, point: &Point) -> Result<usize, StoreError>;
}

#[derive(Error, PartialEq, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum StoreError {
    WriteError { error: String },
    ReadError { error: String },
    OpenError { error: String },
    IncompatibleChainStoreVersions { stored: u16, current: u16 },
}

impl Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::WriteError { error } => write!(f, "WriteError: {}", error),
            StoreError::ReadError { error } => write!(f, "ReadError: {}", error),
            StoreError::OpenError { error } => write!(f, "OpenError: {}", error),
            StoreError::IncompatibleChainStoreVersions { stored, current } => {
                write!(f, "Incompatible DB Versions: found {}, expected {}", stored, current)
            }
        }
    }
}

/// Retrieve all blocks from the chain store starting from the anchor to the best chain tip.
#[cfg(feature = "test-utils")]
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
#[cfg(feature = "test-utils")]
#[expect(clippy::expect_used)]
pub fn get_best_chain_block_headers(store: std::sync::Arc<dyn ChainStore<BlockHeader>>) -> Vec<BlockHeader> {
    store
        .retrieve_best_chain()
        .iter()
        .map(|h| store.load_header(h).expect("missing header for the best chain"))
        .collect()
}
