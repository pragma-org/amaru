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

use std::{cmp::Reverse, collections::VecDeque, iter::successors, sync::Arc};

use amaru_kernel::{BlockHeader, BlockHeight, HeaderHash, IsHeader, NonEmptyVec, ORIGIN_HASH, Point, RawBlock, Tip};

use crate::{
    BaseReadChainStore, ChildTipsMode, FindAncestorOnBestChainResult, FindCommonAncestorResult, MissingBlocks,
    MissingBlocksResult,
    MissingBlocksResult::{BoundaryNotFound, Found, StartHeaderNotFound},
    NextBestChainHeader, Nonces, SampleAncestorPointsResult, StoreError,
};

/// Read interface for the ChainStore. It uses a snapshot when iterating over data for consistency.
pub trait ReadChainStore: BaseReadChainStore {
    /// Return a consistent point-in-time view of the ChainStore which can be used to safely iterate
    /// over chain data.
    fn snapshot(&self) -> Box<dyn BaseReadChainStore + '_>;

    /// Return the next best-chain header from the given pointer using a single snapshot.
    fn next_best_chain_header(&self, pointer: &Point) -> Result<NextBestChainHeader, StoreError> {
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
            if snapshot.load_from_best_chain(&point).is_some() {
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
    fn find_intersect_point(&self, mut points: Vec<Point>) -> Option<Point> {
        let snapshot = self.snapshot();
        points.sort_by_key(|p| Reverse(*p));
        points.into_iter().find(|&point| point == Point::Origin || snapshot.load_from_best_chain(&point).is_some())
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
        let best = snapshot.get_best_chain_hash();
        if best == ORIGIN_HASH {
            return Ok(SampleAncestorPointsResult::Found(vec![Point::Origin]));
        }
        let Some(best) = snapshot.load_header(&best) else {
            return Ok(SampleAncestorPointsResult::BestChainTipNotFound);
        };
        let best_point = best.point();
        let mut points = vec![best_point];
        let mut spacing = 1;
        let mut last = best_point;
        for (index, header) in ancestors_on_snapshot(best, &*snapshot).skip(1).enumerate() {
            last = header.tip().point();
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
    fn find_anchor_at_height(&self, target_height: BlockHeight) -> Option<HeaderHash> {
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
    fn child_tips(&self, hash: &HeaderHash, mode: ChildTipsMode) -> Vec<Tip> {
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
                    result.push(header.tip());
                } else {
                    to_visit.extend(children);
                }
            } else {
                return result;
            }
        }
    }
}

/// Walk ancestors of `start` on a snapshot view, stopping past the anchor.
/// Mirrors `ReadChainStore::ancestors` but takes the snapshot explicitly so it
/// can be reused inside default impls that already hold one.
fn ancestors_on_snapshot<'a>(
    start: BlockHeader,
    snapshot: &'a (dyn BaseReadChainStore + 'a),
) -> Box<dyn Iterator<Item = BlockHeader> + 'a> {
    let anchor_point = snapshot.get_anchor_tip().point();

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
) -> Box<dyn Iterator<Item = (BlockHeader, Option<bool>)> + 'a> {
    let anchor_point = snapshot.get_anchor_tip().point();

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
    fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader> {
        self.as_ref().load_header(hash)
    }

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(BlockHeader, Option<bool>)> {
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

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        self.as_ref().load_from_best_chain(point)
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

    fn has_header(&self, hash: &HeaderHash) -> bool {
        self.as_ref().has_header(hash)
    }
}

impl<T: ReadChainStore + ?Sized> ReadChainStore for Arc<T> {
    fn snapshot(&self) -> Box<dyn BaseReadChainStore + '_> {
        self.as_ref().snapshot()
    }
}
