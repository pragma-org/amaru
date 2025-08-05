use crate::consensus::chain_selection::ForwardChainSelection;
use crate::peer::Peer;
use crate::ConsensusError;
use amaru_kernel::{Point, ORIGIN_HASH};
use amaru_ouroboros_traits::IsHeader;
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

impl<H: IsHeader + Clone + std::fmt::Debug> HeadersTree<H> {
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

    /// Return the tip of the best chain that currently known
    pub fn best_chain_tip(&self) -> Option<&H> {
        self.best_chain
            .and_then(|node_id| self.arena.get(node_id).map(|n| n.get()))
    }

    /// Return best chain fragment currently known
    pub fn best_chain_fragment(&self) -> Vec<&H> {
        if let Some(node_id) = self.best_chain {
            let mut chain: Vec<&H> = node_id
                .ancestors(&self.arena)
                .filter_map(|n_id| self.arena.get(n_id).map(|n| n.get()))
                .collect();
            chain.reverse();
            chain
        } else {
            vec![]
        }
    }

    pub fn select_roll_forward(
        &mut self,
        peer: &Peer,
        header: H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        match self.peers.get(peer) {
            Some(node_id) => {
                let header_node_id = self.arena.new_node(header.clone());
                let current_node = self.arena.get(*node_id).expect(&format!(
                    "Node not found in the arena {}. The arena is {:?}",
                    node_id, self.arena
                ));
                let current_node_hash = current_node.get().hash();
                if header.parent() == Some(current_node_hash) {
                    node_id.append(header_node_id, &mut self.arena);
                    self.best_chain = Some(header_node_id);
                    Ok(ForwardChainSelection::NewTip(header))
                } else {
                    Err(ConsensusError::InvalidHeaderParent {
                        peer: peer.clone(),
                        forwarded: header.hash(),
                        actual: header.parent().unwrap_or(ORIGIN_HASH),
                        expected: current_node_hash,
                    })
                }
            }
            None => Err(ConsensusError::UnknownPeer(peer.clone())),
        }
    }

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
    fn initialize_peer(&mut self, peer: &Peer, point: &Point) -> Result<(), ConsensusError> {
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
    use crate::consensus::chain_selection::ForwardChainSelection;
    use crate::peer::Peer;
    use amaru_kernel::{Point, HEADER_HASH_SIZE};
    use amaru_ouroboros_traits::fake::FakeHeader;

    #[test]
    fn test_empty() {
        let tree: HeadersTree<FakeHeader> = HeadersTree::new(vec![]);
        assert_eq!(
            tree.best_chain_tip(),
            None,
            "there is not best chain for an empty tree yet"
        );
    }

    #[test]
    fn test_single_chain_is_best_chain() {
        let headers = generate_headers_anchored_at(None, 5);
        let last = headers.last().unwrap();
        let tree = HeadersTree::new(headers.clone());

        assert_eq!(tree.best_chain_tip(), Some(last));
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&FakeHeader>>()
        );
    }

    #[test]
    fn test_initialize_peer() {
        let headers = generate_headers_anchored_at(None, 5);
        let peer = Peer::new("alice");
        let mut tree = HeadersTree::new(headers.clone());

        let last = headers.last().unwrap();
        let peer_point = Point::Specific(10, last.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        assert_eq!(tree.get_tip_for(&peer).unwrap(), Some(*last).as_ref());
    }

    #[test]
    fn test_initialize_peer_point_not_found() {
        let headers = generate_headers_anchored_at(None, 5);
        let mut tree = HeadersTree::new(headers.clone());

        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, random_bytes(HEADER_HASH_SIZE).into());
        assert!(tree.initialize_peer(&peer, &peer_point).is_err());
    }

    #[test]
    fn test_roll_forward() {
        // create 6 chained headers, the first 5 are used to create the first best chain
        let headers = generate_headers_anchored_at(None, 6);
        let tip = headers.get(4).unwrap().clone();
        let new_tip = headers.last().unwrap().clone();
        let mut tree = HeadersTree::new(
            headers
                .clone()
                .into_iter()
                .take(5)
                .collect::<Vec<FakeHeader>>(),
        );

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        // Now roll forward with the 6th block
        assert_eq!(
            tree.select_roll_forward(&peer, new_tip).unwrap(),
            ForwardChainSelection::NewTip(new_tip)
        );
        assert_eq!(tree.best_chain_tip(), Some(&new_tip));
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&FakeHeader>>()
        );
    }

    #[test]
    fn test_roll_forward_with_incorrect_parent() {
        // create 6 chained headers, the first 5 are used to create the first best chain
        let headers = generate_headers_anchored_at(None, 5);
        let tip = headers.last().unwrap().clone();

        // create a new tip pointing to an incorrect parent (the first header of the chain in this example)
        let new_tip = FakeHeader {
            block_number: 1,
            slot: 0,
            parent: Some(headers.first().unwrap().hash()),
            body_hash: random_bytes(HEADER_HASH_SIZE).as_slice().into(),
        };
        let mut tree = HeadersTree::new(headers);

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        // Now roll forward with the 6th block
        assert!(tree.select_roll_forward(&peer, new_tip).is_err(),);
    }

    #[test]
    fn test_roll_forward_with_unknown_peer() {
        let headers = generate_headers_anchored_at(None, 5);
        let last = headers.last().unwrap();
        let mut tree = HeadersTree::new(headers.clone());

        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, last.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        // Now roll forward with an unknown peer
        let peer = Peer::new("bob");
        assert!(tree.select_roll_forward(&peer, last.clone()).is_err());
    }
}
