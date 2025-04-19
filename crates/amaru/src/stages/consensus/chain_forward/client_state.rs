use super::ClientOp;
use crate::stages::AsTip;
use amaru_consensus::{consensus::store::ChainStore, IsHeader};
use amaru_kernel::{Hash, Header};
use pallas_network::miniprotocols::{chainsync::Tip, Point};
use std::collections::VecDeque;

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
        self.ops.pop_front()
    }

    pub fn add_op(&mut self, op: ClientOp) {
        match op {
            ClientOp::Backward(tip) => {
                if let Some(index) = self
                    .ops
                    .iter()
                    .rposition(|op| matches!(op, ClientOp::Forward(_, tip) if &tip.0 == &tip.0))
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

/// Find headers between points in the chain store.
/// Returns None if none of the points in `points` lies in the past of `start_point`.
/// Otherwise returns Some(headers) where headers is a list of headers leading from
/// the first found point in the past of `start_point` matching a point from `points`
/// up to `start_point`.
pub(super) fn find_headers_between(
    store: &dyn ChainStore<Header>,
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

pub(super) fn hash_point(point: &Point) -> Hash<32> {
    match point {
        Point::Origin => Hash::from([0; 32]),
        Point::Specific(_slot, hash) => Hash::from(hash.as_slice()),
    }
}
