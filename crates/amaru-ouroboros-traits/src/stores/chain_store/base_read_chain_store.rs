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

use amaru_kernel::{BlockHeader, HeaderHash, ORIGIN_HASH, Point, RawBlock, Tip};

use crate::{Nonces, StoreError};

/// Low-level chain store reads. It is used by the `ReadChainStore` trait which most code should
/// depend on.
pub trait BaseReadChainStore: Send + Sync {
    /// Try to load a header by its hash.
    fn load_header(&self, hash: &HeaderHash) -> Option<BlockHeader>;

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(BlockHeader, Option<bool>)>;

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash>;
    fn get_anchor_hash(&self) -> HeaderHash;
    fn get_anchor_tip(&self) -> Tip {
        self.load_header(&self.get_anchor_hash()).map(|h| h.tip()).unwrap_or_else(Tip::origin)
    }
    fn get_best_chain_hash(&self) -> HeaderHash;

    /// Load a `HeaderHash` from the best chain.
    /// Returns `None` if the point is not in the best chain.
    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash>;

    /// Return the next `Point` on the best chain following given
    /// `Point`, if it exists.
    fn next_best_chain(&self, point: &Point) -> Option<Point>;

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError>;
    fn has_block(&self, hash: &HeaderHash) -> Result<bool, StoreError>;
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
        // TODO: store the tip directly in the database
        self.load_tip(&self.get_best_chain_hash())
            .expect("best chain tip not found. There should always be a best chain tip")
    }
}

impl BaseReadChainStore for Box<dyn BaseReadChainStore + '_> {
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

    fn load_from_best_chain(&self, point: &Point) -> Option<HeaderHash> {
        self.as_ref().load_from_best_chain(point)
    }

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        self.as_ref().next_best_chain(point)
    }
}
