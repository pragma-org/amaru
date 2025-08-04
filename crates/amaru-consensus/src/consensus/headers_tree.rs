use crate::peer::Peer;
use crate::ConsensusError;
use amaru_kernel::Point;
use amaru_ouroboros_traits::{IsHeader, HASH_SIZE};
use indextree::{Arena, NodeId};
use pallas_crypto::hash::Hash;
use std::collections::BTreeMap;

/// This data type stores chains as a tree of headers.
/// It also keeps track of what is the latest tip for each peer.
///
/// The main function of this data type is to be able to always return the best chain for the current
/// tree of headers.
///
pub struct HeadersTree<H> {
    /// The arena maintains a list of headers and their parent/child relationship.
    arena: Arena<H>,
    /// This NodeId points to the header that is at the tip of the best chain.
    best_chain: Option<NodeId>,
    /// This map maintains a node id pointing to the tip of each peer's chain.
    peers: BTreeMap<Peer, NodeId>,
}

impl<H: IsHeader> HeadersTree<H> {
    /// Initialize a HeadersTree from a best chain (h[n - 1] is assumed to be the parent of h[n] in the vector).
    pub fn new(headers: Vec<H>) -> HeadersTree<H> {
        // Create a new arena
        let mut arena: Arena<H> = Arena::new();

        let mut iter = headers.into_iter();
        if let Some(first) = iter.next() {
            let rest: Vec<_> = iter.collect();
            let mut last_node_id: NodeId = arena.new_node(first);

            for header in rest {
                let new_node_id = arena.new_node(header);
                last_node_id.append(new_node_id, &mut arena);
                last_node_id = new_node_id;
            }
            HeadersTree {
                arena,
                best_chain: Some(last_node_id),
                peers: BTreeMap::new(),
            }
        } else {
            HeadersTree {
                arena,
                best_chain: None,
                peers: BTreeMap::new(),
            }
        }
    }

    /// Return the best chain that is currently known
    pub fn best_chain(&self) -> Option<&H> {
        self.best_chain
            .and_then(|node_id| self.arena.get(node_id).map(|n| n.get()))
    }
}

/// Implementation functions
impl<H : IsHeader> HeadersTree<H> {
    /// Return the chain tip for a given Peer
    fn get_tip_for(&self, peer: &Peer) -> Result<Option<&H>, ConsensusError> {
        let node_id = self
            .peers
            .get(peer)
            .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?;
        Ok(self.arena.get(*node_id).map(|node| node.get()))
    }

    /// Add a peer and its current chain tip.
    /// This function must be invoked after the point header has been added to the tree.
    fn intersect_peer(&mut self, peer: &Peer, point: &Point) -> Result<(), ConsensusError> {
        for node in self.arena.iter() {
            if node.get().hash() == Hash::from(point) {
                if let Some(node_id) = self.arena.get_node_id(node) {
                    self.peers.insert(peer.clone(), node_id);
                    return Ok(());
                }
            }
        }
        Err(ConsensusError::UnknownPoint(point.clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consensus::chain_selection::tests::{generate_headers_anchored_at, random_bytes};
    use crate::peer::Peer;
    use amaru_kernel::Point;
    use amaru_ouroboros_traits::fake::FakeHeader;

    #[test]
    fn test_empty() {
        let tree: HeadersTree<FakeHeader> = HeadersTree::new(vec![]);
        assert_eq!(
            tree.best_chain(),
            None,
            "there is not best chain for an empty tree yet"
        );
    }

    #[test]
    fn test_single_chain_is_best_chain() {
        let headers = generate_headers_anchored_at(None, 5);
        let last = headers.last().unwrap();
        let tree = HeadersTree::new(headers.clone());

        assert_eq!(tree.best_chain(), Some(last));
    }

    #[test]
    fn test_intersect_peer() {
        let headers = generate_headers_anchored_at(None, 5);
        let peer = Peer::new("alice");
        let last = headers.last().unwrap();
        let peer_point = Point::Specific(10, last.hash().to_vec());

        let mut tree = HeadersTree::new(headers.clone());
        tree.intersect_peer(&peer, &peer_point).unwrap();

        assert_eq!(tree.get_tip_for(&peer).unwrap(), Some(*last).as_ref());
    }

    #[test]
    fn test_intersect_peer_point_not_found() {
        let headers = generate_headers_anchored_at(None, 5);
        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, random_bytes(HASH_SIZE as u32).into());

        let mut tree = HeadersTree::new(headers.clone());
        assert!(tree.intersect_peer(&peer, &peer_point).is_err());
    }
}
