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

use std::{collections::VecDeque, fmt::Display};

use amaru_kernel::{BlockHeader, HeaderHash, NonEmptyVec, Point};
use thiserror::Error;

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
    RollForward { point: Point, header: BlockHeader },
}
