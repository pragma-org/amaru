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

use std::iter::{from_fn, successors};

use amaru_kernel::{HeaderHash, IsHeader, ORIGIN_HASH, Point, RawBlock, Tip};

use crate::{ChildTipsMode, Nonces, StoreError};

/// Low-level chain store reads. It is used by the [`ReadChainStore`] trait which most code should
/// depend on.
pub trait BaseReadChainStore<H>: Send + Sync
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

    fn child_tips<'a>(&'a self, hash: &HeaderHash, mode: ChildTipsMode) -> Box<dyn Iterator<Item = Tip> + 'a>
    where
        H: 'a,
    {
        // FIXME operate on a snapshot
        let mut to_visit = if hash == &ORIGIN_HASH { self.get_children(hash) } else { vec![*hash] };
        Box::new(from_fn(move || {
            loop {
                let hash = to_visit.pop()?;
                tracing::debug!(hash = %hash, "visiting child");
                #[expect(clippy::panic)]
                let Some((header, validity)) = self.load_header_with_validity(&hash) else {
                    panic!("child header not found: {}", hash);
                };
                if mode == ChildTipsMode::SkipInvalid && validity == Some(false) {
                    continue;
                }
                let children = self.get_children(&hash);
                to_visit.extend(children);
                return Some(header.tip());
            }
        }))
    }
}

impl<H: IsHeader> BaseReadChainStore<H> for Box<dyn BaseReadChainStore<H> + '_> {
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
