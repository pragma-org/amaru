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
    sync::Arc,
};

use amaru_kernel::{
    BlockHeight, Header, HeaderHash, IsHeader, NetworkPoint, NonEmptyVec, ORIGIN_HASH, Point, PoolId, RawBlock, Slot,
};

use crate::{
    BaseReadChainStore, BlockBatch, BlockEntry, BlockHistoryCoverage, BlockIterator, BlockRange, BlockReadError,
    BlockReadLimits, ChildTipsMode, FindAncestorOnBestChainResult, FindCommonAncestorResult, MissingBlocks,
    MissingBlocksResult,
    MissingBlocksResult::{BoundaryNotFound, Found, StartHeaderNotFound},
    NextBestChainHeader, Nonces, ResolvedBlockRange, SampleAncestorPointsResult, StoreError,
};

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
        blocks_on_snapshot(self.snapshot(), range)
    }

    /// Resolve endpoints using the canonical slot index, with bounded, cancellable same-slot checks.
    /// No snapshot escapes this call. Sources with stored metadata need not decode block bodies.
    fn open_block_range(
        &self,
        range: BlockRange,
        limits: BlockReadLimits,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<ResolvedBlockRange, BlockReadError> {
        check_cancelled(cancelled)?;
        let snapshot = self.snapshot();
        let mut reader = RangeReader { snapshot: snapshot.as_ref(), cancelled, remaining: limits.max_steps.get() };
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
        let snapshot = self.snapshot();
        let mut reader = RangeReader { snapshot: snapshot.as_ref(), cancelled, remaining: limits.max_steps.get() };
        let range = reader.resolve(range)?;
        let mut batch =
            BlockBatch { range, blocks: Vec::new(), last_point: range.after, complete: range.after == range.through };
        let mut bytes = 0;
        while !batch.complete && batch.blocks.len() < limits.max_blocks.get() {
            let point = reader.successor(batch.last_point, range.through)?;
            reader.step()?;
            let raw_block = snapshot
                .load_block(&point.hash())
                .map_err(BlockReadError::Store)?
                .ok_or(BlockReadError::MissingHistory { hash: point.hash() })?;
            let size = raw_block.as_ref().len();
            if size > limits.max_bytes.get() - bytes {
                if batch.blocks.is_empty() {
                    return Err(BlockReadError::BlockTooLarge { point, size, limit: limits.max_bytes.get() });
                }
                break;
            }
            bytes += size;
            batch.blocks.push(BlockEntry { point, raw_block });
            batch.last_point = point;
            batch.complete = point == range.through;
        }
        check_cancelled(cancelled)?;
        Ok(batch)
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

fn blocks_on_snapshot(
    snapshot: Box<dyn BaseReadChainStore + '_>,
    range: BlockRange,
) -> Result<BlockIterator<'_>, BlockReadError> {
    let limits = BlockReadLimits::default();
    let mut reader =
        RangeReader { snapshot: snapshot.as_ref(), cancelled: &|| false, remaining: limits.max_steps.get() };
    let range = reader.resolve(range)?;
    let mut after = range.after;
    let mut finished = after == range.through;
    Ok(Box::new(from_fn(move || {
        if finished {
            return None;
        }
        let mut reader =
            RangeReader { snapshot: snapshot.as_ref(), cancelled: &|| false, remaining: limits.max_steps.get() };
        let entry = reader.successor(after, range.through).and_then(|point| {
            snapshot
                .load_block(&point.hash())
                .map_err(BlockReadError::Store)?
                .map(|raw_block| BlockEntry { point, raw_block })
                .ok_or(BlockReadError::MissingHistory { hash: point.hash() })
        });
        match &entry {
            Ok(block) => {
                after = block.point;
                finished = after == range.through;
            }
            Err(_) => finished = true,
        }
        Some(entry)
    })))
}

fn check_cancelled(cancelled: &dyn Fn() -> bool) -> Result<(), BlockReadError> {
    if cancelled() { Err(BlockReadError::Cancelled) } else { Ok(()) }
}

struct RangeReader<'a> {
    snapshot: &'a dyn BaseReadChainStore,
    cancelled: &'a dyn Fn() -> bool,
    remaining: usize,
}

impl RangeReader<'_> {
    fn step(&mut self) -> Result<(), BlockReadError> {
        check_cancelled(self.cancelled)?;
        self.remaining = self.remaining.checked_sub(1).ok_or(BlockReadError::WorkLimit)?;
        Ok(())
    }

    fn metadata(&mut self, hash: HeaderHash) -> Result<(NetworkPoint, Option<HeaderHash>), BlockReadError> {
        self.step()?;
        let metadata = match self.snapshot.load_header(&hash) {
            Some(header) => Some((header.point().to_network_point(), header.parent())),
            None => {
                self.step()?;
                self.snapshot.load_block_metadata(&hash).map_err(BlockReadError::Store)?
            }
        };
        let (point, parent) = metadata.ok_or(BlockReadError::MissingHistory { hash })?;
        if point.hash() != hash {
            return Err(BlockReadError::InvalidAncestry { hash });
        }
        Ok((point, parent))
    }

    fn links(&mut self, point: NetworkPoint) -> Result<Option<HeaderHash>, BlockReadError> {
        let (stored, parent) = self.metadata(point.hash())?;
        if stored != point {
            return Err(BlockReadError::InvalidAncestry { hash: point.hash() });
        }
        Ok(parent)
    }

    fn seek(&mut self, slot: Slot) -> Result<Option<NetworkPoint>, BlockReadError> {
        self.step()?;
        self.snapshot.block_at_or_after(slot).map_err(BlockReadError::Store)
    }

    fn parent(&mut self, point: NetworkPoint, parent: Option<HeaderHash>) -> Result<NetworkPoint, BlockReadError> {
        match parent {
            None => Ok(NetworkPoint::Origin),
            Some(hash) if hash == ORIGIN_HASH => Ok(NetworkPoint::Origin),
            Some(hash) => {
                let (parent, _) = self.metadata(hash)?;
                if parent.slot_or_default() > point.slot_or_default() || parent.hash() == point.hash() {
                    return Err(BlockReadError::InvalidAncestry { hash });
                }
                Ok(parent)
            }
        }
    }

    /// Only blocks sharing a slot need ancestry checks: the slot index orders all other blocks.
    fn at_or_before(&mut self, lower: NetworkPoint, mut upper: NetworkPoint) -> Result<bool, BlockReadError> {
        if lower == NetworkPoint::Origin || lower == upper {
            return Ok(true);
        }
        if upper == NetworkPoint::Origin || lower.slot_or_default() > upper.slot_or_default() {
            return Ok(false);
        }
        while lower.slot_or_default() == upper.slot_or_default() {
            let parent = self.links(upper)?;
            upper = self.parent(upper, parent)?;
            if upper == lower {
                return Ok(true);
            }
            if upper == NetworkPoint::Origin {
                return Ok(false);
            }
        }
        Ok(lower.slot_or_default() < upper.slot_or_default())
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
            self.links(point)?;
            return Ok(());
        }
        Err(BlockReadError::InvalidPosition { point })
    }

    fn resolve(&mut self, range: BlockRange) -> Result<ResolvedBlockRange, BlockReadError> {
        self.step()?;
        let anchor = self.snapshot.read_anchor_point().map_err(BlockReadError::Store)?.to_network_point();
        let through = range.through.unwrap_or(anchor);
        let after = range.after.unwrap_or(NetworkPoint::Origin);
        self.validate_position(through)?;
        if !self.at_or_before(through, anchor)? {
            return Err(BlockReadError::InvalidPosition { point: through });
        }
        self.validate_position(after)?;
        if !self.at_or_before(after, anchor)? {
            return Err(BlockReadError::InvalidPosition { point: after });
        }
        if !self.at_or_before(after, through)? {
            return Err(BlockReadError::InvalidRange { after, through });
        }
        self.step()?;
        let coverage = self.snapshot.block_history_coverage().map_err(BlockReadError::Store)?;
        Ok(ResolvedBlockRange { after, through, coverage })
    }

    fn successor(&mut self, previous: NetworkPoint, through: NetworkPoint) -> Result<NetworkPoint, BlockReadError> {
        let missing = || BlockReadError::MissingSuccessor { after: previous, through };
        let mut candidate = self.seek(previous.slot_or_default())?.ok_or_else(missing)?;
        if candidate == previous {
            let next_slot = u64::from(previous.slot_or_default()).checked_add(1).ok_or_else(missing)?;
            candidate = self.seek(next_slot.into())?.ok_or_else(missing)?;
        }
        if candidate.slot_or_default() > through.slot_or_default() {
            return Err(missing());
        }
        loop {
            let parent = self.links(candidate)?;
            if parent.unwrap_or(ORIGIN_HASH) == previous.hash() {
                if !self.at_or_before(candidate, through)? {
                    return Err(missing());
                }
                return Ok(candidate);
            }
            let parent = self.parent(candidate, parent)?;
            if parent == NetworkPoint::Origin || parent.slot_or_default() != candidate.slot_or_default() {
                return Err(missing());
            }
            candidate = parent;
        }
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
    fn block_history_coverage(&self) -> Result<Option<BlockHistoryCoverage>, StoreError> {
        self.as_ref().block_history_coverage()
    }

    fn load_block_metadata(&self, hash: &HeaderHash) -> Result<Option<(NetworkPoint, Option<HeaderHash>)>, StoreError> {
        self.as_ref().load_block_metadata(hash)
    }

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
