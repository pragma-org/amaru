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

use std::{collections::VecDeque, fmt::Display, num::NonZeroUsize, path::PathBuf};

use amaru_kernel::{Header, HeaderHash, IsHeader, NetworkPoint, NonEmptyVec, Point, RawBlock};
use thiserror::Error;

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

/// Contiguous blocks committed from a verified archive, including both endpoints.
/// `None` coverage means unknown availability, rather than an empty or complete archive.
/// This is a snapshot of availability; pruning may invalidate it before the next batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockHistoryCoverage {
    pub first: NetworkPoint,
    pub through: NetworkPoint,
}

/// Resolved exclusive start and inclusive immutable endpoint, including for an empty range.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResolvedBlockRange {
    pub after: NetworkPoint,
    pub through: NetworkPoint,
    /// Verified archive coverage captured with the immutable boundary, independently of the target.
    pub coverage: Option<BlockHistoryCoverage>,
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

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MissingBlocksResult {
    Found(MissingBlocks),
    BoundaryNotFound,
    StartHeaderNotFound,
}

#[derive(PartialEq, Eq, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FindAncestorOnBestChainResult {
    StartHeaderNotFound,
    NotFound,
    Found { fork_point: Point, forward_points: NonEmptyVec<Point> },
}

#[derive(PartialEq, Eq, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum FindCommonAncestorResult {
    HeaderNotFound(HeaderHash),
    NotFound,
    Found(Point),
}

#[derive(PartialEq, Eq, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum SampleAncestorPointsResult {
    BestChainTipNotFound,
    Found(Vec<Point>),
}

impl SampleAncestorPointsResult {
    pub fn as_slice(&self) -> Option<&[Point]> {
        match self {
            SampleAncestorPointsResult::Found(points) => Some(points.as_slice()),
            _ => None,
        }
    }
}

#[derive(Error, PartialEq, Debug, Clone, serde::Serialize, serde::Deserialize)]
pub enum StoreError {
    WriteError { error: String },
    ReadError { error: String },
    OpenError { error: String },
    Locked { path: PathBuf },
    IncompatibleChainStoreVersions { stored: u16, current: u16 },
}

impl Display for StoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            StoreError::WriteError { error } => write!(f, "WriteError: {}", error),
            StoreError::ReadError { error } => write!(f, "ReadError: {}", error),
            StoreError::OpenError { error } => write!(f, "OpenError: {}", error),
            StoreError::Locked { path } => write!(f, "Chain store at '{}' is locked", path.display()),
            StoreError::IncompatibleChainStoreVersions { stored, current } => {
                write!(
                    f,
                    "Incompatible chain DB versions: found {stored}, expected {current}. \
Pass `--db-chain-automatic-migration` (or set `AMARU_DB_CHAIN_AUTOMATIC_MIGRATION=true`) when starting the node with `amaru node run`."
                )
            }
        }
    }
}

/// Abort if `header` is not the content addressed by `hash`.
///
/// A content-addressed store that yields the wrong blob is corrupt; the node cannot continue.
#[expect(clippy::panic)]
pub fn header_addressed_by(header: Header, hash: &HeaderHash) -> Header {
    let actual = header.hash();
    if actual != *hash {
        panic!(
            "chain-store integrity failure: header stored at key {hash} hashes to {actual}; the database is corrupt and Amaru cannot continue"
        );
    }
    header
}

/// Abort if `block` is not stored under its header hash `hash`, or if the body does not match
/// the header's `block_body_hash`.
///
/// Decode failure or a hash mismatch means the database is corrupt.
#[expect(clippy::panic)]
pub fn block_addressed_by(block: RawBlock, hash: &HeaderHash) -> RawBlock {
    let header = match block.decode_header() {
        Ok(header) => header,
        Err(error) => panic!(
            "chain-store integrity failure: block stored at key {hash} has an undecodable header ({error}); the database is corrupt and Amaru cannot continue"
        ),
    };
    let actual = header.hash();
    if actual != *hash {
        panic!(
            "chain-store integrity failure: block stored at key {hash} has header hash {actual}; the database is corrupt and Amaru cannot continue"
        );
    }
    let expected_body = header.body().block_body_hash;
    let actual_body = match block.body_hash() {
        Ok(body_hash) => body_hash,
        Err(error) => panic!(
            "chain-store integrity failure: block stored at key {hash} has an undecodable body ({error}); the database is corrupt and Amaru cannot continue"
        ),
    };
    if expected_body != actual_body {
        panic!(
            "chain-store integrity failure: block stored at key {hash} has body hash {actual_body} but header claims {expected_body}; the database is corrupt and Amaru cannot continue"
        );
    }
    block
}

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum ChildTipsMode {
    All,
    SkipInvalid,
}

/// List of ordered, consecutive points for which we haven't received any blocks yet.
/// `boundary` is the parent of the first missing point.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MissingBlocks {
    boundary: Point,
    missing: VecDeque<Point>,
}

impl MissingBlocks {
    pub fn new(boundary: Point, missing: Vec<Point>) -> Self {
        Self { boundary, missing: VecDeque::from(missing) }
    }

    pub fn boundary(&self) -> Point {
        self.boundary
    }

    pub fn first(&self) -> Option<Point> {
        self.missing.front().copied()
    }

    pub fn last(&self) -> Option<Point> {
        self.missing.back().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.missing.is_empty()
    }

    pub fn from_to(&self) -> Option<(&Point, &Point)> {
        Some((self.missing.front()?, self.missing.back()?))
    }

    pub fn missing_points(&self) -> Vec<Point> {
        self.missing.iter().copied().collect()
    }

    pub fn nb_missing_blocks(&self) -> usize {
        self.missing.len()
    }

    /// This method is called when the first missing block has been fetched.
    /// It is then removed from the list of missing blocks and becomes the boundary.
    pub fn shift_one_block(&mut self) {
        if let Some(removed) = self.missing.pop_front() {
            self.boundary = removed
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum NextBestChainHeader {
    NeedRollback,
    AtTip,
    MissingHeader { point: Point },
    RollForward { point: Point, header: Header },
}
