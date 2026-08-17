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

use amaru_kernel::{Header, HeaderHash, IsHeader, NetworkPoint, ORIGIN_HASH, Point, PoolId, RawBlock};

use crate::{Nonces, StoreError};

/// Low-level chain store reads. It is used by the `ReadChainStore` trait which most code should
/// depend on.
pub trait BaseReadChainStore: Send + Sync {
    /// Try to load a header by its hash.
    fn load_header(&self, hash: &HeaderHash) -> Option<Header>;

    fn load_header_with_validity(&self, hash: &HeaderHash) -> Option<(Header, Option<bool>)>;

    fn get_children(&self, hash: &HeaderHash) -> Vec<HeaderHash>;

    /// The immutable-horizon point, stored as a full [`Point`].
    ///
    /// An empty store reports [`Point::Origin`].
    fn get_anchor_point(&self) -> Point;

    /// Hash of the immutable-horizon point.
    fn get_anchor_hash(&self) -> HeaderHash {
        self.get_anchor_point().hash()
    }

    /// The locally adopted best-chain tip, stored as a full [`Point`].
    ///
    /// An empty store reports [`Point::Origin`].
    fn get_best_chain_tip(&self) -> Point;

    /// Hash of the locally adopted best-chain tip.
    fn get_best_chain_hash(&self) -> HeaderHash {
        self.get_best_chain_tip().hash()
    }

    /// Whether `point` identifies a block on the locally adopted best chain.
    ///
    /// [`NetworkPoint::Origin`] is always on the best chain. A specific point matches by slot and
    /// header hash; block height is not part of the identity.
    fn is_on_best_chain(&self, point: NetworkPoint) -> bool;

    /// Return the next `Point` on the best chain following given
    /// `Point`, if it exists.
    fn next_best_chain(&self, point: &Point) -> Option<Point>;

    fn load_block(&self, hash: &HeaderHash) -> Result<Option<RawBlock>, StoreError>;
    fn has_block(&self, hash: &HeaderHash) -> Result<bool, StoreError>;
    fn get_nonces(&self, header: &HeaderHash) -> Option<Nonces>;

    /// Latest opcert sequence number of this header's issuer, as specified in one of the ancestors
    /// of that header. A parentless header yields `None`.
    fn get_latest_opcert_sequence_number(&self, pool_id: &PoolId, header: &Header) -> Result<Option<u64>, StoreError>;

    fn has_header(&self, hash: &HeaderHash) -> bool;

    /// Load the chain point of a stored header. Prefer this over [`Self::load_header`] when the
    /// rest of the header is not needed.
    fn load_point(&self, hash: &HeaderHash) -> Option<Point> {
        if hash == &ORIGIN_HASH {
            return Some(Point::Origin);
        }
        self.load_header(hash).map(|h| h.point())
    }
}

impl BaseReadChainStore for Box<dyn BaseReadChainStore + '_> {
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

    fn next_best_chain(&self, point: &Point) -> Option<Point> {
        self.as_ref().next_best_chain(point)
    }
}
