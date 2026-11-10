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

use std::{
    cmp::Reverse,
    collections::VecDeque,
    iter::{from_fn, successors},
    num::NonZeroUsize,
    ops::ControlFlow,
    sync::Arc,
};

use amaru_kernel::{
    BlockHeight, Header, HeaderHash, IsHeader, NetworkPoint, NonEmptyVec, ORIGIN_HASH, Point, PoolId, RawBlock, Slot,
};
use thiserror::Error;

use crate::{
    BaseReadChainStore, ChildTipsMode, FindAncestorOnBestChainResult, FindCommonAncestorResult, MissingBlocks,
    MissingBlocksResult,
    MissingBlocksResult::{BoundaryNotFound, Found, StartHeaderNotFound},
    NextBestChainHeader, Nonces, SampleAncestorPointsResult, StoreError,
};

/// A range on the captured immutable chain, with an exclusive start and inclusive end.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BlockRange {
    /// Last successfully processed block. `None` starts at the beginning of the chain.
    /// Missing ancestry is an error; a retained suffix is never presented as complete history.
    pub after: Option<NetworkPoint>,
    /// Last block to return. `None` captures the store's immutable anchor when the range opens.
    pub through: Option<NetworkPoint>,
}

/// An owned block and its slot/hash position.
#[derive(Debug, Clone)]
pub struct BlockEntry {
    pub point: NetworkPoint,
    pub raw_block: RawBlock,
}

/// A forward iterator over one consistent chain-store snapshot.
pub type BlockIterator<'a> = Box<dyn Iterator<Item = Result<BlockEntry, BlockReadError>> + Send + 'a>;

/// Resolved exclusive start and inclusive immutable endpoint, including for an empty range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBlockRange {
    pub after: NetworkPoint,
    pub through: NetworkPoint,
}

/// Bounds on one blocking read. Storage steps include endpoint validation and same-slot ancestry.
/// Cancellation is checked before every step; an individual storage call cannot be interrupted.
#[derive(Debug, Clone, Copy)]
pub struct BlockReadLimits {
    pub max_blocks: NonZeroUsize,
    pub max_bytes: NonZeroUsize,
    pub max_steps: NonZeroUsize,
}

impl Default for BlockReadLimits {
    fn default() -> Self {
        Self {
            max_blocks: NonZeroUsize::new(100).unwrap_or(NonZeroUsize::MIN),
            max_bytes: NonZeroUsize::new(16 * 1024 * 1024).unwrap_or(NonZeroUsize::MIN),
            max_steps: NonZeroUsize::new(4096).unwrap_or(NonZeroUsize::MIN),
        }
    }
}

/// Owned results; the snapshot has already been released when this value is returned.
#[derive(Debug)]
pub struct BlockBatch {
    pub range: ResolvedBlockRange,
    pub blocks: Vec<BlockEntry>,
    /// Last returned point, or the exclusive start if empty. Persist with index changes atomically.
    pub last_point: NetworkPoint,
    /// Every block through `range.through` has been read successfully.
    pub complete: bool,
}

/// Failures opening or consuming an immutable block range.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum BlockReadError {
    #[error("block read cancelled")]
    Cancelled,
    #[error("the node's block reader is closed")]
    Closed,
    #[error("block read exceeded its storage-step budget")]
    WorkLimit,
    #[error("block {point} requires {size} bytes, exceeding the batch limit {limit}")]
    BlockTooLarge { point: NetworkPoint, size: usize, limit: usize },
    #[error("point {point} is not on the requested immutable chain")]
    InvalidPosition { point: NetworkPoint },
    #[error("start {after} follows end {through} on the immutable chain")]
    InvalidRange { after: NetworkPoint, through: NetworkPoint },
    #[error("historical block or ancestry metadata is unavailable for {hash}")]
    MissingHistory { hash: HeaderHash },
    #[error("historical successor after {after} is unavailable before end {through}")]
    MissingSuccessor { after: NetworkPoint, through: NetworkPoint },
    #[error("invalid historical parent links at {hash}")]
    InvalidAncestry { hash: HeaderHash },
    #[error("failed to read the chain store: {0}")]
    Store(#[source] StoreError),
}

/// Read interface for the ChainStore. It uses a snapshot when iterating over data for consistency.
pub trait ReadChainStore: BaseReadChainStore {
    /// Return a consistent point-in-time view of the ChainStore which can be used to safely iterate
    /// over chain data.
    fn snapshot(&self) -> Box<dyn BaseReadChainStore + '_>;

    /// Stream immutable blocks in canonical order from one snapshot, with an exclusive start
    /// and inclusive end. An omitted end captures the immutable anchor when this method is called.
    ///
    /// Opening validates endpoints using the canonical slot index. Iteration loads one body at a
    /// time, with bounded same-slot ancestry checks and no range-sized buffer. Unknown, orphaned,
    /// and volatile endpoints are rejected; missing history is an error. Errors fuse the iterator.
    /// This low-level iterator holds its snapshot until dropped and performs blocking reads.
    /// Prefer [`Self::read_block_batch`] to release snapshots between application transactions.
    fn blocks(&self, range: BlockRange) -> Result<BlockIterator<'_>, BlockReadError> {
        let max_steps = BlockReadLimits::default().max_steps.get();
        let mut reader = RangeReader { snapshot: self.snapshot(), cancelled: || false, remaining: max_steps };
        let range = reader.resolve(range)?;
        let mut after = range.after;
        Ok(Box::new(from_fn(move || {
            if after == range.through {
                return None;
            }
            reader.remaining = max_steps;
            let entry = reader.next_entry(after, range.through);
            after = entry.as_ref().map_or(range.through, |block| block.point);
            Some(entry)
        })))
    }

    /// Resolve endpoints using the canonical slot index, with bounded, cancellable same-slot checks.
    /// No snapshot escapes this call.
    fn open_block_range(
        &self,
        range: BlockRange,
        limits: BlockReadLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ResolvedBlockRange, BlockReadError> {
        check_cancelled(cancelled)?;
        let mut reader = RangeReader { snapshot: self.snapshot(), cancelled, remaining: limits.max_steps.get() };
        let range = reader.resolve(range)?;
        check_cancelled(cancelled)?;
        Ok(range)
    }

    /// Read a bounded batch from a fresh snapshot, releasing it before returning owned blocks.
    /// Reuse the resolved explicit `through` on every call to preserve the captured target.
    /// A removed endpoint, missing body, or broken link is an error, never successful completion.
    /// On cancellation or work-budget exhaustion no partial batch is returned; retry from the
    /// application's last atomically committed point. Endpoint checks do not walk older history.
    fn read_block_batch(
        &self,
        range: BlockRange,
        limits: BlockReadLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<BlockBatch, BlockReadError> {
        check_cancelled(cancelled)?;
        let mut reader = RangeReader { snapshot: self.snapshot(), cancelled, remaining: limits.max_steps.get() };
        let range = reader.resolve(range)?;
        let blocks = (0..limits.max_blocks.get())
            .scan((range.after, limits.max_bytes.get()), |(after, remaining_bytes), index| {
                if *after == range.through {
                    return None;
                }
                reader
                    .next_entry(*after, range.through)
                    .and_then(|entry| match remaining_bytes.checked_sub(entry.raw_block.len()) {
                        Some(remaining) => {
                            *after = entry.point;
                            *remaining_bytes = remaining;
                            Ok(Some(entry))
                        }
                        None if index == 0 => Err(BlockReadError::BlockTooLarge {
                            point: entry.point,
                            size: entry.raw_block.len(),
                            limit: limits.max_bytes.get(),
                        }),
                        None => Ok(None),
                    })
                    .transpose()
            })
            .collect::<Result<Vec<_>, _>>()?;
        check_cancelled(cancelled)?;
        let last_point = blocks.last().map_or(range.after, |block| block.point);
        Ok(BlockBatch { range, blocks, last_point, complete: last_point == range.through })
    }

    /// Return the next best-chain header from the given pointer using a single snapshot.
    fn next_best_chain_header(&self, pointer: &Point) -> Result<NextBestChainHeader, StoreError> {
        let snapshot = self.snapshot();
        if *pointer != Point::Origin && !snapshot.is_on_best_chain((*pointer).into()) {
            return Ok(NextBestChainHeader::NeedRollback);
        }
        let Some(point) = snapshot.next_best_chain(pointer) else {
            return Ok(NextBestChainHeader::AtTip);
        };
        let Some(header) = snapshot.load_header(&point.hash()) else {
            return Ok(NextBestChainHeader::MissingHeader { point });
        };
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
    fn unvalidated_ancestor_hashes(&self, start: HeaderHash) -> (Vec<HeaderHash>, bool) {
        let snapshot = self.snapshot();
        let mut hashes = Vec::new();
        let mut valid = true;
        for (header, v) in ancestors_with_validity_on_snapshot(start, &*snapshot) {
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

    /// Return the tips of the headers leading from `from` (exclusive) to `to` (inclusive), in
    /// parent -> child order, walking a single snapshot.
    ///
    /// Example:
    ///
    ///   O--A--B--C--D
    ///         ^     ^
    ///        from   to
    ///
    /// Returns `Some([C, D])`, and `Some([])` when `to` is `from` itself.
    ///
    /// Unlike [`Self::unvalidated_ancestor_hashes`] the walk is bounded by `from` rather than by the
    /// recorded validity of the blocks, so the caller gets a path anchored exactly where it asked.
    ///
    /// Returns `None` when a header along the way is missing from the store, or when `to` is not a
    /// descendant of `from`.
    fn ancestors_between(&self, from: &Point, to: HeaderHash) -> Option<Vec<Point>> {
        let snapshot = self.snapshot();
        let from_slot = from.slot_or_default();
        let from_hash = from.hash();
        let mut tips = Vec::new();
        let mut current = to;

        while current != from_hash {
            let header = snapshot.load_header(&current)?;

            // Slots strictly increase along a chain, so a header at or below `from`'s slot that is
            // not `from` itself proves `to` hangs off a branch that never passes through it. Giving
            // up here keeps a mismatched pair from reading every header down to the genesis block.
            if header.slot() <= from_slot {
                return None;
            }

            tips.push(header.point());
            current = header.parent()?;
        }

        tips.reverse();
        Some(tips)
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
    fn find_ancestor_on_best_chain(&self, start: HeaderHash) -> Result<FindAncestorOnBestChainResult, StoreError> {
        let snapshot = self.snapshot();
        let Some(header) = snapshot.load_header(&start) else {
            return Ok(FindAncestorOnBestChainResult::StartHeaderNotFound);
        };
        let mut forward_points = Vec::new();
        for ancestor in ancestors_on_snapshot(header, &*snapshot) {
            let point = ancestor.point();
            if snapshot.is_on_best_chain(point.into()) {
                forward_points.reverse();
                if let Ok(forward_points) = NonEmptyVec::try_from(forward_points) {
                    return Ok(FindAncestorOnBestChainResult::Found { fork_point: point, forward_points });
                } else {
                    break;
                }
            }
            forward_points.push(point);
        }
        Ok(FindAncestorOnBestChainResult::NotFound)
    }

    /// Return the most recent point shared by both chains if it exists.
    ///
    /// Example:
    /// O--A--B--C--D
    ///       \
    ///        E--F--G
    ///
    /// `find_common_ancestor(D, G)` returns `Some(B)`.
    fn find_common_ancestor(
        &self,
        hash1: HeaderHash,
        hash2: HeaderHash,
    ) -> Result<FindCommonAncestorResult, StoreError> {
        let snapshot = self.snapshot();
        let Some(header1) = snapshot.load_header(&hash1) else {
            return Ok(FindCommonAncestorResult::HeaderNotFound(hash1));
        };
        let Some(header2) = snapshot.load_header(&hash2) else {
            return Ok(FindCommonAncestorResult::HeaderNotFound(hash2));
        };
        let mut chain1 = ancestors_on_snapshot(header1, &*snapshot).map(|h| h.point()).peekable();
        'outer: for point in ancestors_on_snapshot(header2, &*snapshot).map(|h| h.point()) {
            while let Some(a_point) = chain1.peek() {
                if *a_point > point {
                    chain1.next();
                } else if *a_point == point {
                    return Ok(FindCommonAncestorResult::Found(point));
                } else {
                    continue 'outer;
                }
            }
            break;
        }
        Ok(FindCommonAncestorResult::NotFound)
    }

    /// Find the first point, in the list of points, that intersects with the best chain.
    /// The origin point is always considered to be an intersection point.
    ///
    /// Return None if none of the points is on the best chain
    fn find_intersect_point(&self, mut points: Vec<NetworkPoint>) -> Option<Point> {
        let snapshot = self.snapshot();
        points.sort_by_key(|p| Reverse(*p));
        points.into_iter().find_map(|point| match point {
            NetworkPoint::Origin => Some(Point::Origin),
            NetworkPoint::Specific(_, hash) => snapshot.is_on_best_chain(point).then(|| snapshot.load_point(&hash))?,
        })
    }

    /// Return a sparse sample of points from the best chain, starting at the tip, with
    /// exponentially increasing spacing, always ending with the oldest reachable point.
    ///
    /// Example:
    /// O--A--B--C--D--E--F--G  tip
    ///
    /// Returns `[G, F, D, O]`.
    fn sample_ancestor_points(&self) -> Result<SampleAncestorPointsResult, StoreError> {
        let snapshot = self.snapshot();
        let best_point = snapshot.get_best_chain_tip();
        if best_point == Point::Origin {
            return Ok(SampleAncestorPointsResult::Found(vec![Point::Origin]));
        }
        let Some(best) = snapshot.load_header(&best_point.hash()) else {
            return Ok(SampleAncestorPointsResult::BestChainTipNotFound);
        };
        let mut points = vec![best_point];
        let mut spacing = 1;
        let mut last = best_point;
        for (index, header) in ancestors_on_snapshot(best, &*snapshot).skip(1).enumerate() {
            last = header.point();
            if index + 1 == spacing {
                points.push(last);
                spacing *= 2;
            }
        }
        if points.last() != Some(&last) {
            points.push(last);
        }
        Ok(SampleAncestorPointsResult::Found(points))
    }

    /// Walk forward on the best chain from the current anchor and return the hash of the first
    /// header whose block height is `>= target_height`. Returns `None` if the current anchor is
    /// already at or past that height, or if the best chain does not reach it.
    ///
    /// The entire walk runs against a single snapshot, so callers see a consistent view of the
    /// best chain even if other writers mutate it concurrently.
    fn find_anchor_at_height(&self, target_height: BlockHeight) -> Option<Point> {
        let snapshot = self.snapshot();
        let mut point = snapshot.get_anchor_point();
        if target_height <= point.block_height() {
            return None;
        }
        while let Some(next_point) = snapshot.next_best_chain(&point) {
            if next_point.block_height() >= target_height {
                return Some(next_point);
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
    /// `Some(Found(MissingBlocks { boundary: B, missing: [C, D, E] }))`.
    ///
    /// Return `StartHeaderNotFound` if the start_hash header does not exist in the database.
    /// Return `BoundaryNotFound` if we could not find an ancestor with a valid block
    ///
    /// Note: the anchor point is not returned because that will confuse block validation.
    ///
    fn find_missing_blocks(&self, start_hash: HeaderHash, limit: usize) -> Result<MissingBlocksResult, StoreError> {
        let snapshot = self.snapshot();
        let Some(start) = snapshot.load_header(&start_hash) else {
            return Ok(StartHeaderNotFound);
        };
        let anchor = snapshot.get_anchor_hash();
        let mut missing = Vec::new();
        for header in ancestors_on_snapshot(start, &*snapshot) {
            if snapshot.has_block(&header.hash())? || header.hash() == anchor {
                missing.reverse();
                missing.truncate(limit);
                return Ok(Found(MissingBlocks::new(header.point(), missing)));
            } else {
                missing.push(header.point());
            }
        }
        Ok(BoundaryNotFound)
    }

    /// Return the tips of a tree of headers starting from a root hash.
    /// All the branches of the tree are explored if mode == ChildTipsMode::All,
    /// otherwise only branches having non-invalid blocks are explored if mode == ChildTipsMode::SkipInvalid
    ///
    /// For example:
    ///
    /// O--A(ok)--B(ok)--C(ok)--D
    ///       \
    ///        E(ok)--F(ko)--G
    ///
    /// child_tips(A, ChildTipsMode::All) returns D and G.
    /// child_tips(A, ChildTipsMode::SkipInvalid) returns D only.
    ///
    fn child_tips(&self, hash: &HeaderHash, mode: ChildTipsMode) -> Vec<Point> {
        let snapshot = self.snapshot();
        let mut result = vec![];
        let mut to_visit: VecDeque<HeaderHash> =
            if hash == &ORIGIN_HASH { snapshot.get_children(hash).into() } else { vec![*hash].into() };
        loop {
            if let Some(hash) = to_visit.pop_front() {
                #[expect(clippy::panic)]
                let Some((header, validity)) = snapshot.load_header_with_validity(&hash) else {
                    panic!("child header not found: {}", hash);
                };
                if mode == ChildTipsMode::SkipInvalid && validity == Some(false) {
                    continue;
                }
                let children = snapshot.get_children(&hash);
                if children.is_empty() {
                    result.push(header.point());
                } else {
                    to_visit.extend(children);
                }
            } else {
                return result;
            }
        }
    }
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), BlockReadError> {
    if cancelled() { Err(BlockReadError::Cancelled) } else { Ok(()) }
}

fn walk_until<T>(
    mut point: NetworkPoint,
    mut step: impl FnMut(NetworkPoint) -> Result<ControlFlow<T, NetworkPoint>, BlockReadError>,
) -> Result<T, BlockReadError> {
    loop {
        match step(point)? {
            ControlFlow::Continue(next) => point = next,
            ControlFlow::Break(result) => return Ok(result),
        }
    }
}

struct RangeReader<'a, C> {
    snapshot: Box<dyn BaseReadChainStore + 'a>,
    cancelled: C,
    remaining: usize,
}

impl<C: Fn() -> bool> RangeReader<'_, C> {
    /// Validate the next canonical link before loading its authenticated body.
    fn next_entry(&mut self, after: NetworkPoint, through: NetworkPoint) -> Result<BlockEntry, BlockReadError> {
        let point = self.successor(after, through)?;
        self.step()?;
        let raw_block = self
            .snapshot
            .load_block(&point.hash())
            .map_err(BlockReadError::Store)?
            .ok_or(BlockReadError::MissingHistory { hash: point.hash() })?;
        Ok(BlockEntry { point, raw_block })
    }

    fn step(&mut self) -> Result<(), BlockReadError> {
        check_cancelled(&self.cancelled)?;
        self.remaining = self.remaining.checked_sub(1).ok_or(BlockReadError::WorkLimit)?;
        Ok(())
    }

    fn header(&mut self, hash: HeaderHash) -> Result<Header, BlockReadError> {
        self.step()?;
        if let Some(header) = self.snapshot.load_header(&hash) {
            return Ok(header);
        }
        self.step()?;
        self.snapshot
            .load_block(&hash)
            .map_err(BlockReadError::Store)?
            .and_then(|block| block.decode_header().ok())
            .ok_or(BlockReadError::MissingHistory { hash })
    }

    fn parent_hash(&mut self, point: NetworkPoint) -> Result<Option<HeaderHash>, BlockReadError> {
        let header = self.header(point.hash())?;
        if header.point().to_network_point() != point {
            return Err(BlockReadError::InvalidAncestry { hash: point.hash() });
        }
        Ok(header.parent())
    }

    fn seek(&mut self, slot: Slot) -> Result<Option<NetworkPoint>, BlockReadError> {
        self.step()?;
        self.snapshot.block_at_or_after(slot).map_err(BlockReadError::Store)
    }

    fn parent(&mut self, point: NetworkPoint, parent: Option<HeaderHash>) -> Result<NetworkPoint, BlockReadError> {
        let Some(hash) = parent.filter(|hash| *hash != ORIGIN_HASH) else {
            return Ok(NetworkPoint::Origin);
        };
        let parent = self.header(hash)?.point().to_network_point();
        if parent.slot_or_default() > point.slot_or_default() || parent.hash() == point.hash() {
            return Err(BlockReadError::InvalidAncestry { hash });
        }
        Ok(parent)
    }

    /// Only blocks sharing a slot need ancestry checks: the slot index orders all other blocks.
    fn at_or_before(&mut self, lower: NetworkPoint, upper: NetworkPoint) -> Result<bool, BlockReadError> {
        walk_until(upper, |upper| {
            if lower == NetworkPoint::Origin || lower == upper {
                Ok(ControlFlow::Break(true))
            } else if upper == NetworkPoint::Origin || lower.slot_or_default() != upper.slot_or_default() {
                Ok(ControlFlow::Break(
                    upper != NetworkPoint::Origin && lower.slot_or_default() < upper.slot_or_default(),
                ))
            } else {
                self.parent_hash(upper).and_then(|parent| self.parent(upper, parent)).map(ControlFlow::Continue)
            }
        })
    }

    fn validate_position(&mut self, point: NetworkPoint) -> Result<(), BlockReadError> {
        if point == NetworkPoint::Origin {
            return Ok(());
        }
        let canonical = self.seek(point.slot_or_default())?;
        if let Some(canonical) = canonical
            && canonical.slot_or_default() == point.slot_or_default()
            && self.at_or_before(point, canonical)?
        {
            self.parent_hash(point)?;
            return Ok(());
        }
        Err(BlockReadError::InvalidPosition { point })
    }

    fn resolve(&mut self, range: BlockRange) -> Result<ResolvedBlockRange, BlockReadError> {
        self.step()?;
        let anchor = self.snapshot.read_anchor_point().map_err(BlockReadError::Store)?.to_network_point();
        let through = range.through.unwrap_or(anchor);
        let after = range.after.unwrap_or(NetworkPoint::Origin);
        [through, after].into_iter().try_for_each(|point| {
            self.validate_position(point)?;
            self.at_or_before(point, anchor)?.then_some(()).ok_or(BlockReadError::InvalidPosition { point })
        })?;
        self.at_or_before(after, through)?
            .then_some(ResolvedBlockRange { after, through })
            .ok_or(BlockReadError::InvalidRange { after, through })
    }

    fn successor(&mut self, previous: NetworkPoint, through: NetworkPoint) -> Result<NetworkPoint, BlockReadError> {
        let missing = || BlockReadError::MissingSuccessor { after: previous, through };
        let candidate = self.seek(previous.slot_or_default())?.ok_or_else(missing)?;
        let candidate = if candidate == previous {
            let next_slot = u64::from(previous.slot_or_default()).checked_add(1).ok_or_else(missing)?;
            self.seek(next_slot.into())?.ok_or_else(missing)?
        } else {
            candidate
        };
        if candidate.slot_or_default() > through.slot_or_default() {
            return Err(missing());
        }
        walk_until(candidate, |candidate| {
            let parent = self.parent_hash(candidate)?;
            if parent.unwrap_or(ORIGIN_HASH) == previous.hash() {
                self.at_or_before(candidate, through)?.then_some(ControlFlow::Break(candidate)).ok_or_else(missing)
            } else {
                let parent = self.parent(candidate, parent)?;
                (parent != NetworkPoint::Origin && parent.slot_or_default() == candidate.slot_or_default())
                    .then_some(ControlFlow::Continue(parent))
                    .ok_or_else(missing)
            }
        })
    }
}

/// Walk ancestors of `start` on a snapshot view, stopping past the anchor.
/// Mirrors `ReadChainStore::ancestors` but takes the snapshot explicitly so it
/// can be reused inside default impls that already hold one.
fn ancestors_on_snapshot<'a>(
    start: Header,
    snapshot: &'a (dyn BaseReadChainStore + 'a),
) -> Box<dyn Iterator<Item = Header> + 'a> {
    let anchor_point = snapshot.get_anchor_point();

    Box::new(successors(Some(start), move |h| {
        if h.slot() <= anchor_point.slot_or_default() {
            None
        } else {
            h.parent().and_then(|p| snapshot.load_header(&p))
        }
    }))
}

fn ancestors_with_validity_on_snapshot<'a>(
    start: HeaderHash,
    snapshot: &'a (dyn BaseReadChainStore + 'a),
) -> Box<dyn Iterator<Item = (Header, Option<bool>)> + 'a> {
    let anchor_point = snapshot.get_anchor_point();

    let header_opt = snapshot.load_header_with_validity(&start);

    Box::new(successors(header_opt, move |(h, _valid)| {
        if h.slot() <= anchor_point.slot_or_default() {
            None
        } else {
            h.parent().and_then(|p| snapshot.load_header_with_validity(&p))
        }
    }))
}

impl<T: BaseReadChainStore + ?Sized> BaseReadChainStore for Arc<T> {
    fn read_anchor_point(&self) -> Result<Point, StoreError> {
        self.as_ref().read_anchor_point()
    }

    fn block_at_or_after(&self, slot: Slot) -> Result<Option<NetworkPoint>, StoreError> {
        self.as_ref().block_at_or_after(slot)
    }

    fn load_header(&self, hash: &HeaderHash) -> Option<Header> {
        self.as_ref().load_header(hash)
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(Header, Option<bool>)> {
        self.as_ref().load_header_with_validity(hash)
    }

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash> {
        self.as_ref().get_children(hash)
    }

    fn get_anchor_point(&self) -> Point {
        self.as_ref().get_anchor_point()
    }

    fn get_best_chain_tip(&self) -> Point {
        self.as_ref().get_best_chain_tip()
    }

    fn is_on_best_chain(&self, point: NetworkPoint) -> bool {
        self.as_ref().is_on_best_chain(point)
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        self.as_ref().next_best_chain(point)
    }

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError> {
        self.as_ref().load_block(hash)
    }

    fn has_block(&self, hash: &HeaderHash) -> Result<bool, StoreError> {
        self.as_ref().has_block(hash)
    }

    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces> {
        self.as_ref().get_nonces(header)
    }

    fn get_latest_opcert_sequence_number(&self, pool_id: &PoolId, header: &Header) -> Result<Option<u64>, StoreError> {
        self.as_ref().get_latest_opcert_sequence_number(pool_id, header)
    }

    fn has_header(&self, hash: &HeaderHash) -> bool {
        self.as_ref().has_header(hash)
    }
}

impl<T: ReadChainStore + ?Sized> ReadChainStore for Arc<T> {
    fn snapshot(&self) -> Box<dyn BaseReadChainStore + '_> {
        self.as_ref().snapshot()
    }
}
