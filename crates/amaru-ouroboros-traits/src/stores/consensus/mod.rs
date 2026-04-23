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

pub mod missing_blocks;
use std::{
    cmp::Reverse,
    fmt::Display,
    iter::{self, successors},
};

use amaru_kernel::{BlockHeader, BlockHeight, HeaderHash, IsHeader, NonEmptyVec, ORIGIN_HASH, Point, RawBlock, Tip};
use thiserror::Error;

use crate::{Nonces, consensus::missing_blocks::MissingBlocks};

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

    #[expect(clippy::expect_used)]
    fn get_best_chain_tip(&self) -> Tip {
        self.load_tip(&self.get_best_chain_hash())
            .expect("best chain tip not found. There should always be a best chain tip")
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

impl<H: IsHeader> ReadOnlyChainStore<H> for Box<dyn ReadOnlyChainStore<H> + '_> {
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
    /// Return an immutable, read-only version of the chain store.
    fn snapshot(&self) -> Box<dyn ReadOnlyChainStore<H> + '_>;

    /// Return the next best-chain header from the given pointer using a single snapshot.
    fn next_best_chain_header(&self, pointer: &Point) -> Result<NextBestChainHeader<H>, StoreError> {
        let snapshot = self.snapshot();
        if *pointer != Point::Origin && snapshot.load_from_best_chain(pointer).is_none() {
            return Ok(NextBestChainHeader::NeedRollback);
        }
        let Some(point) = snapshot.next_best_chain(pointer) else {
            return Ok(NextBestChainHeader::AtTip);
        };
        let Some(header) = snapshot.load_header(&point.hash()) else {
            return Ok(NextBestChainHeader::MissingHeader { point });
        };
        if header.parent().unwrap_or(ORIGIN_HASH) != pointer.hash() {
            return Ok(NextBestChainHeader::NeedRollback);
        }
        Ok(NextBestChainHeader::RollForward { point, header })
    }

    /// Return the hashes of the ancestors of the header (inclusive of the start hash and in parent -> child order),
    /// until the first validated ancestor (exclusive) and return a bool denoting
    /// if that ancestor's block is valid or invalid.
    ///
    /// Example:
    ///
    ///   O--A--B--C
    ///            ^
    ///          start
    ///
    /// Returns `([A, B, C], true)`.
    ///
    /// If the first validated ancestor is invalid instead:
    ///   O--A--B--C
    ///            ^
    ///          start
    ///
    /// Returns `([A, B, C], false)`.
    ///
    /// Note that the anchor hash will not be returned since it is always valid.
    fn unvalidated_ancestor_hashes(&self, start: HeaderHash) -> (Vec<HeaderHash>, bool)
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let mut hashes = Vec::new();
        let mut valid = true;
        for (header, v) in snapshot.ancestors_with_validity(start) {
            match v {
                Some(is_valid) => {
                    valid = is_valid;
                    break;
                }
                None => {
                    hashes.push(header.hash());
                }
            }
        }
        hashes.reverse();
        (hashes, valid)
    }

    /// Return the fork point with the best chain (if it exists) and the list of points from
    /// that point to the new best tip (in that order, ending with `start`)
    ///
    /// Example:
    ///            D--E  current best chain
    ///           /
    /// O--A--B--C
    ///          \
    ///           F--G
    ///              ^
    ///            start = new best tip
    ///
    /// Returns `(C, [F, G])`.
    ///
    /// Returns None if the start point is already on the best chain.
    fn find_fork_point(&self, start: HeaderHash) -> Option<(Point, NonEmptyVec<Point>)>
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let header = snapshot.load_header(&start)?;
        let mut forward_points = None;
        for ancestor in snapshot.ancestors(header) {
            let point = ancestor.point();
            if snapshot.load_from_best_chain(&point).is_some() {
                // Both `?` here implement the "start was already on best chain" case from the
                // contract: if the first ancestor we visit is already on the best chain, we
                // never populated `forward_points` (outer `?`), or we only pushed `start`
                // itself and then popped it (inner `?` leaves nothing behind). Either way we
                // return `None`.
                let mut forward_points: Vec<Point> = forward_points?;
                let first = forward_points.pop()?;
                forward_points.reverse();
                return Some((point, NonEmptyVec::new(first, forward_points)));
            }
            forward_points.get_or_insert_with(Vec::new).push(point);
        }
        None
    }

    /// Return the most recent point shared by both chains if it exists.
    ///
    /// Example:
    /// O--A--B--C--D
    ///       \
    ///        E--F--G
    ///
    /// `find_common_ancestor(D, G)` returns `Some(B)`.
    fn find_common_ancestor(&self, hash1: HeaderHash, hash2: HeaderHash) -> Option<Point>
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let header1 = snapshot.load_header(&hash1)?;
        let header2 = snapshot.load_header(&hash2)?;
        let mut chain1 = snapshot.ancestors(header1).map(|h| h.point()).peekable();
        for point in snapshot.ancestors(header2).map(|h| h.point()) {
            while let Some(a_point) = chain1.peek() {
                if a_point.slot_or_default() > point.slot_or_default() {
                    chain1.next();
                } else {
                    break;
                }
            }
            if let Some(a_point) = chain1.peek() {
                if *a_point == point {
                    return Some(point);
                }
            } else {
                break;
            }
        }
        None
    }

    /// Find the first point, in the list of points, that intersects with the best chain.
    ///
    /// Return None if none of the points is on the best chain
    fn find_intersect_point(&self, mut points: Vec<Point>) -> Option<Point>
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let best_chain_is_non_empty = snapshot.get_best_chain_hash() != ORIGIN_HASH;
        points.sort_by_key(|p| Reverse(*p));
        points.into_iter().find(|&point| {
            (point == Point::Origin && best_chain_is_non_empty) || snapshot.load_from_best_chain(&point).is_some()
        })
    }

    /// Find the closest rollback point when walking ancestors from `parent_hash`.
    ///
    /// The search runs against a single store snapshot. It returns the ledger tip
    /// itself if encountered, otherwise it returns the closest ancestor that is both
    /// on the best chain and marked valid. Invalid ancestors stop the search, and
    /// ancestors below `ledger_tip` are rejected as below the rollback boundary.
    fn find_rollback_point(&self, parent_hash: HeaderHash, ledger_tip: Point) -> RollbackPointSearchResult
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let anchor_hash = snapshot.get_anchor_hash();
        let mut current_hash = parent_hash;
        let mut forward_points = Vec::new();

        loop {
            let Some((ancestor, valid)) = snapshot.load_header_with_validity(&current_hash) else {
                return RollbackPointSearchResult::NotFound;
            };
            let ancestor_point = ancestor.point();

            if valid == Some(false) {
                return RollbackPointSearchResult::DependsOnInvalid;
            }

            if ancestor_point < ledger_tip {
                return RollbackPointSearchResult::BelowImmutable;
            }

            let chosen_because_contains = ancestor_point != ledger_tip
                && valid == Some(true)
                && snapshot.load_from_best_chain(&ancestor_point).is_some();
            if ancestor_point == ledger_tip || chosen_because_contains {
                forward_points.reverse();
                return RollbackPointSearchResult::Found {
                    point: ancestor_point,
                    forward_points,
                    chosen_because_contains,
                };
            }

            forward_points.push(ancestor_point);
            if current_hash == anchor_hash {
                return RollbackPointSearchResult::NotFound;
            }

            let Some(parent_hash) = ancestor.parent() else {
                return RollbackPointSearchResult::NotFound;
            };
            current_hash = parent_hash;
        }
    }

    /// Return a sparse sample of points from the best chain, starting at the tip, with
    /// exponentially increasing spacing, always ending with the oldest reachable point.
    ///
    /// Example:
    /// O--A--B--C--D--E--F--G  tip
    ///
    /// Returns `[G, F, D, O]`.
    fn sample_ancestor_points(&self) -> Vec<Point>
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let best = snapshot.get_best_chain_hash();
        if best == ORIGIN_HASH {
            return vec![Point::Origin];
        }
        let Some(best) = snapshot.load_header(&best) else {
            return vec![Point::Origin];
        };
        let best_point = best.tip().point();
        let mut points = vec![best_point];
        let mut spacing = 1;
        let mut last = best_point;
        for (index, header) in snapshot.ancestors(best).skip(1).enumerate() {
            last = header.tip().point();
            if index + 1 == spacing {
                points.push(last);
                spacing *= 2;
            }
        }
        if points.last() != Some(&last) {
            points.push(last);
        }
        points
    }

    /// Walk forward on the best chain from the current anchor and return the hash of the first
    /// header whose block height is `>= target_height`. Returns `None` if the current anchor is
    /// already at or past that height, or if the best chain does not reach it.
    ///
    /// The entire walk runs against a single snapshot, so callers see a consistent view of the
    /// best chain even if other writers mutate it concurrently.
    fn find_anchor_at_height(&self, target_height: BlockHeight) -> Option<HeaderHash>
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let anchor_hash = snapshot.get_anchor_hash();
        let (mut point, current_height) = if anchor_hash == ORIGIN_HASH {
            (Point::Origin, BlockHeight::from(0))
        } else {
            let header = snapshot.load_header(&anchor_hash)?;
            (header.point(), header.block_height())
        };
        if target_height <= current_height {
            return None;
        }
        while let Some(next_point) = snapshot.next_best_chain(&point) {
            let next_header = snapshot.load_header(&next_point.hash())?;
            if next_header.block_height() >= target_height {
                return Some(next_header.hash());
            }
            point = next_point;
        }
        None
    }

    /// Return the range of missing blocks on the path from the nearest available block (or anchor)
    /// up to `start_hash`, in ancestor -> descendant order, truncated to the `limit` oldest entries.
    ///
    /// Example:
    /// O---A---B---C---D---E
    ///         *           ^
    ///       block     start_hash
    ///       present
    ///
    /// If blocks for `C`, `D`, and `E` are missing, returns
    /// `Some(MissingBlocks { boundary: B, missing: [C, D, E] })`.
    ///
    /// Return `None` if the last_hash header does not exist in the database.
    ///
    /// Note: the anchor point is not returned because that will confuse block validation.
    ///
    fn find_missing_blocks(&self, start_hash: HeaderHash, limit: usize) -> Result<Option<MissingBlocks>, StoreError>
    where
        H: 'static,
    {
        let snapshot = self.snapshot();
        let Some(start) = snapshot.load_header(&start_hash) else {
            return Ok(None);
        };
        let anchor = snapshot.get_anchor_hash();
        let mut missing = Vec::new();
        for header in snapshot.ancestors(start) {
            let block = snapshot.load_block(&header.hash())?;
            if block.is_some() || header.hash() == anchor {
                missing.reverse();
                missing.truncate(limit);
                return Ok(Some(MissingBlocks::new(header.point(), missing)));
            } else {
                missing.push(header.point());
            }
        }
        Ok(None)
    }

    fn store_header(&self, header: &H) -> Result<(), StoreError>;

    fn set_anchor_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;

    fn set_best_chain_hash(&self, hash: &HeaderHash) -> Result<(), StoreError>;

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError>;

    fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> Result<(), StoreError>;

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError>;

    /// Replace the current best chain from the given fork point with the provided
    /// forward path and set the best chain hash in one store operation.
    /// The best chain hash is set to the hash of the last forward point.
    fn switch_to_fork(&self, fork_point: &Point, forward_points: &NonEmptyVec<Point>) -> Result<(), StoreError>;

    /// Roll forward the best chain to the given point and set the best chain hash to that point.
    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError>;
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum RollbackPointSearchResult {
    Found { point: Point, forward_points: Vec<Point>, chosen_because_contains: bool },
    DependsOnInvalid,
    BelowImmutable,
    NotFound,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NextBestChainHeader<H> {
    NeedRollback,
    AtTip,
    MissingHeader { point: Point },
    RollForward { point: Point, header: H },
}
