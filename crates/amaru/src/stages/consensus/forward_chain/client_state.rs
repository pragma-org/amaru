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

use super::{ClientOp, hash_point};
use crate::stages::AsTip;
use amaru_kernel::Header;
use amaru_ouroboros_traits::IsHeader;
use amaru_stores::chain_store::ChainStore;
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
                        |(_, op)| matches!(op, ClientOp::Forward(_, tip2) if tip2.0 == tip.0),
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
    let mut headers = vec![ClientOp::Forward(
        current_header.clone(),
        current_header.as_tip(),
    )];

    while let Some(parent_hash) = current_header.parent() {
        match store.load_header(&parent_hash) {
            Some(header) => {
                if points.iter().any(|p| hash_point(p) == parent_hash) {
                    // Found a matching point, return the collected headers
                    headers.reverse();
                    return Some((headers, header.as_tip()));
                }
                headers.push(ClientOp::Forward(header.clone(), header.as_tip()));
                current_header = header;
            }
            None => return None, // Broken chain
        }
    }

    // Reached genesis without finding any matching point
    headers.reverse();
    Some((headers, Tip(Point::Origin, 0)))
}
