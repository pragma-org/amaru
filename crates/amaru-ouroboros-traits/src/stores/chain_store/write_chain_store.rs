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

use amaru_kernel::{Header, HeaderHash, Point, RawBlock};

use crate::{Nonces, OpcertSequenceNumbers, StoreError};

/// Write interface for the ChainStore
pub trait WriteChainStore: Send + Sync {
    fn store_header(&self, header: &Header) -> Result<(), StoreError>;

    /// Store a header together with its evolved nonces in a single store operation.
    ///
    /// Both are written atomically so that the presence of nonces for a header always means that
    /// this header was fully validated.
    fn store_validated_header(&self, header: &Header, nonces: &Nonces) -> Result<(), StoreError>;

    fn set_anchor_point(&self, point: &Point) -> Result<(), StoreError>;

    fn set_best_chain_tip(&self, tip: &Point) -> Result<(), StoreError>;

    fn store_block(&self, hash: &HeaderHash, block: &RawBlock) -> Result<(), StoreError>;

    fn set_block_valid(&self, hash: &HeaderHash, valid: bool) -> Result<(), StoreError>;

    /// Remove the validity flag of a block.
    fn remove_block_valid(&self, hash: &HeaderHash) -> Result<(), StoreError>;

    fn put_nonces(&self, header: &HeaderHash, nonces: &Nonces) -> Result<(), StoreError>;

    fn put_opcert_seed(&self, counters: &OpcertSequenceNumbers, at: &Point) -> Result<(), StoreError>;

    /// Replace the current best chain from the given fork point with the provided
    /// forward path and set the best-chain tip in one store operation.
    /// The best-chain tip is set to the last forward point.
    fn switch_to_fork(&self, fork_point: &Point, forward_points: &[Point]) -> Result<(), StoreError>;

    /// Roll forward the best chain to the given point and set the best chain hash to that point.
    fn roll_forward_chain(&self, point: &Point) -> Result<(), StoreError>;
}
