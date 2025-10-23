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

use crate::stages::AsTip;
use crate::stages::consensus::forward_chain::client_protocol::{ClientOp, hash_point};
use amaru_kernel::{HeaderHash, IsHeader, Slot};
use amaru_network::point::{from_network_point, to_network_point};
use amaru_ouroboros_traits::ChainStore;
use pallas_network::miniprotocols::{Point, chainsync::Tip};
use std::collections::VecDeque;
use std::iter;
use std::sync::Arc;
use tracing::trace;

/// A structure that maintains state to follow the best chain for a given client.
///
/// The `ops` list may contain up to one rollback at the front only.
pub(super) struct ChainFollower<H> {
    /// The buffer of _operations_ to send to the client.
    ops: VecDeque<ClientOp<H>>,

    /// An iterator the part of the chain the follower should be served
    /// that comes from the immutable (stored) headers
    iterator: Box<dyn Iterator<Item = (u64, HeaderHash)>>,

    /// The initial intersection `Tip` for this follower.
    pub(super) intersection: Point,
}

impl<H: IsHeader + Clone> ChainFollower<H> {
    pub fn new(
        store: Arc<dyn ChainStore<H>>,
        current_tip: &Point,
        points: &[Point],
    ) -> Option<Self> {
        let start_header = store.load_header(&hash_point(current_tip))?;

        // the client is at least as up-to-date as we are
        if points.contains(current_tip) {
            return Some(Self {
                ops: vec![ClientOp::Backward(start_header.as_tip())].into(),
                iterator: Box::new(iter::empty()),
                intersection: to_network_point(start_header.point()),
            });
        }

        // find the anchor
        let anchor_hash = store.get_anchor_hash();

        // the best intersection point from the requested points
        let best_intersection = points
            .iter()
            .filter_map(|p| {
                store
                    .load_from_best_chain(&from_network_point(p))
                    .map(|h| (p.slot_or_default(), h))
            })
            .max_by_key(|(slot, _)| *slot);

        trace!(%anchor_hash, ?current_tip, ?best_intersection, "best_intersection");

        let mut current_header = start_header;
        let mut headers = vec![];

        // walk backwards until either:
        // 1. we find our intersection
        // 2. or we find the anchor
        while let Some(parent_hash) = current_header.parent() {
            trace!(%parent_hash, "walking back");
            match store.load_header(&parent_hash) {
                Some(header) => {
                    let is_intersection = best_intersection
                        .as_ref()
                        .map(|(_, h)| *h == parent_hash)
                        .unwrap_or(false);

                    if parent_hash == anchor_hash || is_intersection {
                        break;
                    }

                    headers.push(ClientOp::Forward(header.clone()));
                    current_header = header;
                }
                None => return None, // Broken chain?
            }
        }

        let store_iterator = needs_immutable(&best_intersection, &headers, &anchor_hash);

        // headers contains a list of Fwd operations in reverse order
        // add backward operation to intersection point
        headers.push(ClientOp::Backward(
            best_intersection
                .and_then(|(_, h)| store.load_header(&h))
                .map(|h| h.as_tip())
                .unwrap_or(Tip(Point::Origin, 0)),
        ));
        headers.reverse();

        Some(Self {
            ops: headers.into(),
            iterator: store_iterator,
            intersection: best_intersection
                .map(|(s, h)| Point::Specific(s, h.to_vec()))
                .unwrap_or(Point::Origin),
        })
    }

    pub fn next_op(&mut self) -> Option<ClientOp<H>> {
        self.ops.pop_front()
    }

    pub fn add_op(&mut self, op: ClientOp<H>) {
        match op {
            ClientOp::Backward(tip) => {
                if let Some((index, _)) =
                    self.ops.iter().enumerate().rfind(
                        |(_, op)| matches!(op, ClientOp::Forward(header2) if to_network_point(header2.point()) == tip.0),
                    )
                {
                    self.ops.truncate(index + 1);
                } else {
                    self.ops.clear();
                    self.ops.push_back(ClientOp::Backward(tip));
                }
            }
            op @ ClientOp::Forward(..) => {
                self.ops.push_back(op);
            }
        }
    }
}

fn needs_immutable<H: IsHeader + Clone>(
    best_intersection: &Option<(u64, HeaderHash)>,
    headers: &[ClientOp<H>],
    anchor_hash: &HeaderHash,
) -> Box<dyn Iterator<Item = (u64, HeaderHash)>> {
    Box::new(iter::empty())
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::stages::AsTip;
    use crate::stages::consensus::forward_chain::chain_follower::ChainFollower;
    use crate::stages::consensus::forward_chain::client_protocol::ClientOp;
    use crate::stages::consensus::forward_chain::test_infra::{
        CHAIN_47, FIRST_HEADER, FORK_47, LOST_47, TIP_47, WINNER_47, hash, mk_in_memory_store,
    };
    use amaru_kernel::{BlockHeader, IsHeader};
    use amaru_kernel::{Hash, HeaderHash};
    use amaru_network::point::from_network_point;
    use amaru_network::point::to_network_point;
    use amaru_ouroboros_traits::ChainStore;
    use pallas_network::miniprotocols::Point;
    use pallas_network::miniprotocols::chainsync::Tip;
    use std::sync::Arc;

    #[test]
    fn test_mk_store() {
        let store = mk_in_memory_store(CHAIN_47);
        assert_eq!(store.len(), 48);
        let chain = store.get_chain(TIP_47);
        assert_eq!(chain.len(), 47);
        assert_eq!(chain[0].header_body().slot, 31);
        assert_eq!(chain[0].header_body().prev_hash, None);
        assert_eq!(chain[46].header_body().slot, 990);
        assert_eq!(chain[6].block_height(), 7);
        assert_eq!(
            Some(chain[6].hash()),
            store.load_from_best_chain(&from_network_point(&store.get_point(FORK_47)))
        )
    }

    #[test]
    fn find_headers_starting_at_tip() {
        let store = mk_in_memory_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        let points = [store.get_point(TIP_47)];
        let start = Tip(tip.clone(), store.get_height(TIP_47));

        let mut chain_follower = ChainFollower::new(store, &tip, &points).unwrap();

        assert_eq!(chain_follower.next_op(), Some(ClientOp::Backward(start)));
    }

    #[test]
    fn find_headers_starting_from_fork_point() {
        let store = mk_in_memory_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        let points = [store.get_point(FORK_47)];
        let expected = store
            .load_header(&Hash::from(hex::decode(FORK_47).unwrap().as_slice()))
            .unwrap();

        let mut chain_follower = ChainFollower::new(store.clone(), &tip, &points).unwrap();

        assert_eq!(
            chain_follower.next_op(),
            Some(ClientOp::Backward(expected.as_tip()))
        );
    }

    #[test]
    fn starts_from_earliest_point_on_chain() {
        let store = mk_in_memory_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        // Note that the below scheme does not match the documented behaviour, which shall pick the first from
        // the list that is on the same chain. But that doesn't make sense to me at all.
        let points = [
            store.get_point(FORK_47),   // this will lose to the (taller) winner
            store.get_point(WINNER_47), // this is the winner after the branch
        ];
        let expected = store.get_point(WINNER_47);

        let mut chain_follower = ChainFollower::new(store.clone(), &tip, &points).unwrap();
        assert_eq!(
            chain_follower.next_op(),
            Some(ClientOp::Backward(Tip(expected, 8)))
        );
    }

    #[test]
    fn starts_from_origin_given_intersection_requested_is_not_on_best_chain() {
        let store = mk_in_memory_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        let points = [store.get_point(LOST_47)];
        let first = store
            .load_header(&Hash::from(hex::decode(FIRST_HEADER).unwrap().as_slice()))
            .expect("could not load header");

        let mut chain_follower = ChainFollower::new(store.clone(), &tip, &points).unwrap();

        assert_eq!(
            chain_follower.next_op(),
            Some(ClientOp::Backward(Tip(Point::Origin, 0)))
        );
        assert_eq!(chain_follower.next_op(), Some(ClientOp::Forward(first)));
    }

    // HELPERS

    /// This trait extends ChainStore with some useful methods for tests.
    pub trait ChainStoreExt {
        fn len(&self) -> usize;

        fn get_all_children(&self, hash: &HeaderHash) -> Vec<BlockHeader>;

        fn get_chain(&self, h: &str) -> Vec<BlockHeader>;

        fn get_point(&self, h: &str) -> Point;

        fn get_height(&self, h: &str) -> u64;
    }

    impl ChainStoreExt for Arc<dyn ChainStore<BlockHeader>> {
        fn len(&self) -> usize {
            self.get_all_children(&self.get_anchor_hash()).len()
        }

        fn get_all_children(&self, hash: &HeaderHash) -> Vec<BlockHeader> {
            let mut result = vec![];
            if let Some(header) = self.load_header(hash) {
                result.push(header);
            }
            for child in self.get_children(hash) {
                result.extend(self.get_all_children(&child))
            }
            result
        }

        fn get_chain(&self, h: &str) -> Vec<BlockHeader> {
            let mut chain = Vec::new();
            let mut current = hash(h);
            while let Some(header) = self.load_header(&current) {
                chain.push(header.clone());
                let Some(parent) = header.parent() else {
                    break;
                };
                current = parent;
            }
            chain.reverse();
            chain
        }

        fn get_point(&self, h: &str) -> Point {
            let header = self.load_header(&hash(h)).unwrap();
            to_network_point(header.point())
        }

        fn get_height(&self, h: &str) -> u64 {
            let header = self.load_header(&hash(h)).unwrap();
            header.block_height()
        }
    }
}
