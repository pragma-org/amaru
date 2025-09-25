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

use crate::stages::consensus::forward_chain::client_protocol::{ClientOp, hash_point};
use crate::stages::{AsTip, PallasPoint};
use amaru_kernel::Header;
use amaru_ouroboros_traits::{ChainStore, IsHeader};
use pallas_network::miniprotocols::{Point, chainsync::Tip};
use std::collections::VecDeque;
use std::sync::Arc;

/// The state we track for one client.
///
/// The `ops` list may contain up to one rollback at the front only.
pub(super) struct ClientState {
    /// The list of operations to send to the client.
    ops: VecDeque<ClientOp>,
}

impl ClientState {
    pub fn new(ops: VecDeque<ClientOp>) -> Self {
        Self { ops }
    }

    pub fn next_op(&mut self) -> Option<ClientOp> {
        tracing::debug!("next_op: {:?}", self.ops.front());
        self.ops.pop_front()
    }

    pub fn add_op(&mut self, op: ClientOp) {
        tracing::debug!("add_op: {:?}", op);
        match op {
            ClientOp::Backward(tip) => {
                if let Some((index, _)) =
                    self.ops.iter().enumerate().rfind(
                        |(_, op)| matches!(op, ClientOp::Forward(header2) if header2.pallas_point() == tip.0),
                    )
                {
                    tracing::debug!("found backward op at index {index} in {:?}", self.ops);
                    self.ops.truncate(index + 1);
                    tracing::debug!("last after truncate: {:?}", self.ops.back());
                } else {
                    tracing::debug!("clearing ops");
                    self.ops.clear();
                    self.ops.push_back(ClientOp::Backward(tip));
                }
            }
            op @ ClientOp::Forward(..) => {
                tracing::debug!("adding forward op");
                self.ops.push_back(op);
            }
        }
    }
}

/// Find headers between points in the chain store.
/// Returns None if the local chain is broken.
/// Otherwise returns Some(headers) where headers is a list of headers leading from
/// the tallest point from the list that lies in the past of `start_point`.
pub(super) fn find_headers_between(
    store: Arc<dyn ChainStore<Header>>,
    start_point: &Point,
    points: &[Point],
) -> Option<(Vec<ClientOp>, Tip)> {
    let start_header = store.load_header(&hash_point(start_point))?;

    if points.contains(start_point) {
        return Some((vec![], start_header.as_tip()));
    }

    // Find the first point that is in the past of start_point
    let mut current_header = start_header;
    let mut headers = vec![ClientOp::Forward(current_header.clone())];

    while let Some(parent_hash) = current_header.parent() {
        match store.load_header(&parent_hash) {
            Some(header) => {
                if points.iter().any(|p| hash_point(p) == parent_hash) {
                    // Found a matching point, return the collected headers
                    headers.reverse();
                    return Some((headers, header.as_tip()));
                }
                headers.push(ClientOp::Forward(header.clone()));
                current_header = header;
            }
            None => return None, // Broken chain
        }
    }

    // Reached genesis without finding any matching point
    headers.reverse();
    Some((headers, Tip(Point::Origin, 0)))
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::stages::PallasPoint;
    use crate::stages::consensus::forward_chain::client_state::find_headers_between;
    use crate::stages::consensus::forward_chain::test_infra::{
        BRANCH_47, CHAIN_47, LOST_47, TIP_47, WINNER_47, hash, mk_store,
    };
    use amaru_kernel::{Hash, Header};
    use amaru_ouroboros_traits::{ChainStore, IsHeader};
    use pallas_network::miniprotocols::Point;
    use pallas_network::miniprotocols::chainsync::Tip;
    use std::sync::Arc;

    #[test]
    fn test_mk_store() {
        let store = mk_store(CHAIN_47);
        assert_eq!(store.len(), 48);
        let chain = store.get_chain(TIP_47);
        assert_eq!(chain.len(), 47);
        assert_eq!(chain[0].header_body.slot, 31);
        assert_eq!(chain[0].header_body.prev_hash, None);
        assert_eq!(chain[46].header_body.slot, 990);
        assert_eq!(chain[6].block_height(), 7);
    }

    #[test]
    fn find_headers_between_tip_and_tip() {
        let store = mk_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        let points = [store.get_point(TIP_47)];

        let (ops, Tip(p, h)) = find_headers_between(store, &tip, &points).unwrap();
        assert_eq!((ops, p, h), (vec![], tip, 47));
    }

    #[test]
    fn find_headers_between_tip_and_branch() {
        let store = mk_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        let points = [store.get_point(BRANCH_47)];
        let peer = store.get_point(BRANCH_47);

        let (ops, Tip(p, h)) = find_headers_between(store.clone(), &tip, &points).unwrap();
        assert_eq!(
            (ops.len() as u64, p, h),
            (
                store.get_height(TIP_47) - store.get_height(BRANCH_47),
                peer,
                store.get_height(BRANCH_47)
            )
        );
    }

    #[test]
    fn find_headers_between_tip_and_branches() {
        let store = mk_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        // Note that the below scheme does not match the documented behaviour, which shall pick the first from
        // the list that is on the same chain. But that doesn't make sense to me at all.
        let points = [
            store.get_point(BRANCH_47), // this will lose to the (taller) winner
            store.get_point(LOST_47),   // this is not on the same chain
            store.get_point(WINNER_47), // this is the winner after the branch
        ];
        let peer = store.get_point(WINNER_47);

        let (ops, Tip(p, h)) = find_headers_between(store.clone(), &tip, &points).unwrap();
        assert_eq!(
            (ops.len() as u64, p, h),
            (
                store.get_height(TIP_47) - store.get_height(WINNER_47),
                peer,
                store.get_height(WINNER_47)
            )
        );
    }

    #[test]
    fn find_headers_between_tip_and_lost() {
        let store = mk_store(CHAIN_47);

        let tip = store.get_point(TIP_47);
        let points = [store.get_point(LOST_47)];

        let result = find_headers_between(store.clone(), &tip, &points).unwrap();
        assert_eq!(result.0.len() as u64, store.get_height(TIP_47));
        assert_eq!(result.1.0, Point::Origin);
        assert_eq!(result.1.1, 0);
    }

    // HELPERS

    /// This trait extends ChainStore with some useful methods for tests.
    pub trait ChainStoreExt {
        fn len(&self) -> usize;

        fn get_all_children(&self, hash: &Hash<32>) -> Vec<Header>;

        fn get_chain(&self, h: &str) -> Vec<Header>;

        fn get_point(&self, h: &str) -> Point;

        fn get_height(&self, h: &str) -> u64;
    }

    impl ChainStoreExt for Arc<dyn ChainStore<Header>> {
        fn len(&self) -> usize {
            self.get_all_children(&self.get_anchor_hash()).len()
        }

        fn get_all_children(&self, hash: &Hash<32>) -> Vec<Header> {
            let mut result = vec![];
            if let Some(header) = self.load_header(hash) {
                result.push(header);
            }
            for child in self.get_children(hash) {
                result.extend(self.get_all_children(&child))
            }
            result
        }

        fn get_chain(&self, h: &str) -> Vec<Header> {
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
            header.pallas_point()
        }

        fn get_height(&self, h: &str) -> u64 {
            let header = self.load_header(&hash(h)).unwrap();
            header.block_height()
        }
    }
}
