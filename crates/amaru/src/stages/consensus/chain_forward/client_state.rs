use amaru_consensus::{consensus::store::ChainStore, IsHeader};
use amaru_kernel::{Hash, Header};
use pallas_network::miniprotocols::{chainsync::Tip, Point};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;

#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ClientOp {
    Backward(Point),
    Forward(Header),
}

/// The state we track for one client.
///
/// The `ops` list may contain up to one rollback at the front only.
pub(super) struct ClientState {
    store: Arc<Mutex<dyn ChainStore<Header>>>,
    /// The list of operations to send to the client.
    ops: VecDeque<ClientOp>,
    /// The point we presume the client is at.
    /// This is updated as soon as we send an operation to the client.
    client_at: Tip,
}

impl ClientState {
    pub fn new(
        store: Arc<Mutex<dyn ChainStore<Header>>>,
        ops: VecDeque<ClientOp>,
        client_at: Tip,
    ) -> Self {
        Self {
            store,
            ops,
            client_at,
        }
    }

    pub async fn next_op(&mut self) -> Option<(ClientOp, Tip)> {
        let op = self.ops.pop_front()?;
        let tip = self.tip().await;
        Some((op, tip))
    }

    pub async fn tip(&self) -> Tip {
        if let Some(op) = self.ops.back() {
            match op {
                ClientOp::Backward(point) => {
                    let store = self.store.lock().await;
                    #[allow(clippy::expect_used)]
                    let header = store
                        .load_header(&hash_point(point))
                        .expect("rollback point was not in store");
                    Tip(point.clone(), header.block_height())
                }
                ClientOp::Forward(header) => {
                    Tip(to_pallas_point(&header.point()), header.block_height())
                }
            }
        } else {
            self.client_at.clone()
        }
    }

    pub fn add_op(&mut self, op: ClientOp) {
        match op {
            ClientOp::Backward(point) => {
                let needle = ClientOp::Backward(point.clone());
                if let Some(index) = self.ops.iter().rposition(|op| op == &needle) {
                    self.ops.truncate(index + 1);
                } else {
                    self.ops.clear();
                    self.ops.push_back(ClientOp::Backward(point));
                }
            }
            op @ ClientOp::Forward(_) => self.ops.push_back(op),
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
        return Some((
            vec![],
            Tip(start_point.clone(), start_header.block_height()),
        ));
    }

    // Find the first point that is in the past of start_point
    let mut current_header = start_header;
    let mut headers = vec![ClientOp::Forward(current_header.clone())];

    while let Some(parent_hash) = current_header.parent() {
        match store.load_header(&parent_hash) {
            Some(header) => {
                if points.iter().any(|p| hash_point(p) == parent_hash) {
                    // Found a matching point, return the collected headers
                    return Some((
                        headers,
                        Tip(to_pallas_point(&header.point()), header.block_height()),
                    ));
                }
                headers.push(ClientOp::Forward(header.clone()));
                current_header = header;
            }
            None => return None, // Broken chain
        }
    }

    // Reached genesis without finding any matching point
    Some((headers, Tip(Point::Origin, 0)))
}

pub(super) fn hash_point(point: &Point) -> Hash<32> {
    match point {
        Point::Origin => Hash::from([0; 32]),
        Point::Specific(_slot, hash) => Hash::from(hash.as_slice()),
    }
}

pub(super) fn to_pallas_point(point: &amaru_kernel::Point) -> Point {
    match point {
        amaru_kernel::Point::Origin => Point::Origin,
        amaru_kernel::Point::Specific(slot, hash) => Point::Specific(*slot, hash.clone()),
    }
}
