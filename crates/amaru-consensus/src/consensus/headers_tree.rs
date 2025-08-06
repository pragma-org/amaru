use crate::consensus::chain_selection::{Fork, ForwardChainSelection};
use crate::peer::Peer;
use crate::ConsensusError;
use amaru_kernel::{Point, ORIGIN_HASH};
use amaru_ouroboros_traits::IsHeader;
use indextree::{Arena, Node, NodeId};
use pallas_crypto::hash::Hash;
use std::collections::BTreeMap;
use tracing::debug;

/// This data type stores chains as a tree of headers.
/// It also keeps track of what is the latest tip for each peer.
///
/// The main function of this data type is to be able to always return the best chain for the current
/// tree of headers.
///
#[allow(dead_code)]
#[derive(Debug)]
pub struct HeadersTree<H> {
    /// The arena maintains a list of headers and their parent/child relationship.
    arena: Arena<H>,
    /// This NodeId points to the header that is at the tip of the best chain.
    best_chain: Option<NodeId>,
    /// This map maintains a node id pointing to the tip of each peer's chain.
    peers: BTreeMap<Peer, NodeId>,
}

#[allow(dead_code)]
impl<H: IsHeader + Clone + std::fmt::Debug> HeadersTree<H> {
    /// Initialize a HeadersTree from a best chain (h[n - 1] is assumed to be the parent of h[n] in the vector).
    pub fn new(headers: Vec<H>) -> HeadersTree<H> {
        // Create a new arena
        let mut arena: Arena<H> = Arena::new();
        let best_chain = HeadersTree::insert_headers_into_arena(&mut arena, headers);

        HeadersTree {
            arena,
            best_chain,
            peers: BTreeMap::new(),
        }
    }

    /// Insert headers into the tree structure to initialize the chain of a new peer
    fn insert_headers(&mut self, headers: Vec<H>) {
        _ = Self::insert_headers_into_arena(&mut self.arena, headers)
    }

    /// Insert headers into the arena and return the last created node id
    fn insert_headers_into_arena(arena: &mut Arena<H>, headers: Vec<H>) -> Option<NodeId> {
        let mut iter = headers.into_iter();
        if let Some(first) = iter.next() {
            let rest: Vec<_> = iter.collect();
            let mut last_node_id: NodeId = arena.new_node(first);

            for header in rest {
                let new_node_id = arena.new_node(header);
                last_node_id.append(new_node_id, arena);
                last_node_id = new_node_id;
            }
            Some(last_node_id)
        } else {
            None
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
            self.get_chain_from(&node_id)
        } else {
            vec![]
        }
    }

    pub fn select_roll_forward(
        &mut self,
        peer: &Peer,
        header: H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        let peer_tip = match self.peers.get(peer) {
            Some(node_id) => *node_id,
            None => return Err(ConsensusError::UnknownPeer(peer.clone())),
        };
        let peer_tip_node = self.unsafe_get_arena_node(peer_tip);
        let peer_tip_node_hash = peer_tip_node.get().hash();
        if header.hash() == peer_tip_node_hash {
            Ok(ForwardChainSelection::NoChange)
        } else if header.parent() == Some(peer_tip_node_hash) {
            let header_node_id = self.insert_header(peer, header.clone(), &peer_tip);
            Ok(self.select_new_best_chain(peer, header, &header_node_id, &peer_tip))
        } else {
            let e = ConsensusError::InvalidHeaderParent {
                peer: peer.clone(),
                forwarded: header.hash(),
                actual: header.parent().unwrap_or(ORIGIN_HASH),
                expected: peer_tip_node_hash,
            };
            debug!("{e}. The current headers tree is {:?}", &self);
            Err(e)
        }
    }

    /// Return the chain ending with the header at node_id (sorted from older to younger).
    fn get_chain_from(&self, node_id: &NodeId) -> Vec<&H> {
        let mut chain: Vec<&H> = node_id
            .ancestors(&self.arena)
            .filter_map(|n_id| self.arena.get(n_id).map(|n| n.get()))
            .collect();
        chain.reverse();
        chain
    }

    /// Return an arena node when it is expected to be found
    #[allow(clippy::panic)]
    fn unsafe_get_arena_node(&self, node_id: NodeId) -> &Node<H> {
        self.arena.get(node_id).unwrap_or_else(|| {
            panic!(
                "Node not found in the arena {}. The arena is {:?}",
                node_id, self.arena
            )
        })
    }

    /// Insert a new header in the arena and maintain the peer tip
    fn insert_header(&mut self, peer: &Peer, header: H, parent_node_id: &NodeId) -> NodeId {
        let header_node_id = self.arena.new_node(header.clone());
        parent_node_id.append(header_node_id, &mut self.arena);
        self.peers.insert(peer.clone(), header_node_id);
        header_node_id
    }

    /// Given a new header insertion for a given peer, at parent_node_id,
    /// determine if this is a NoChange, NewTip or a SwitchToFork
    fn select_new_best_chain(
        &mut self,
        peer: &Peer,
        header: H,
        header_node_id: &NodeId,
        parent_node_id: &NodeId,
    ) -> ForwardChainSelection<H> {
        match self.best_chain {
            Some(current_tip) => {
                // If we added the new node on top of the current best chain, we have a new tip
                if *parent_node_id == current_tip {
                    self.best_chain = Some(*header_node_id);
                    ForwardChainSelection::NewTip {
                        peer: peer.clone(),
                        tip: header,
                    }
                } else {
                    let current_tip_header = self.unsafe_get_arena_node(current_tip).get();
                    // If the new header does not improve the current chain height we keep the same best chain
                    if header.block_height() <= current_tip_header.block_height() {
                        ForwardChainSelection::NoChange
                    } else {
                        // Otherwise, if the new header creates a longer chain, we have a fork

                        // We set the new best chain
                        self.best_chain = Some(*header_node_id);

                        // The rollback point is the intersection of the previous best chain and the new one.
                        // The fork_fragment is the list of header that must be recreated after the rollback point.
                        let intersection_node_id: Option<NodeId> =
                            self.find_intersection_node_id(header_node_id, &current_tip);
                        let mut fork_fragment: Vec<H> = header_node_id
                            .ancestors(&self.arena)
                            .take_while(|n| Some(*n) != intersection_node_id)
                            .filter_map(|n| self.arena.get(n).map(|n| n.get().clone()))
                            .collect();
                        fork_fragment.reverse();
                        let fork = Fork {
                            peer: peer.clone(),
                            rollback_point: intersection_node_id
                                .and_then(|n| self.arena.get(n).map(|n| n.get().point()))
                                .unwrap_or(Point::Origin),
                            fork: fork_fragment,
                        };
                        ForwardChainSelection::SwitchToFork(fork)
                    }
                }
            }
            None => {
                self.best_chain = Some(*header_node_id);
                ForwardChainSelection::NewTip {
                    peer: peer.clone(),
                    tip: header,
                }
            }
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

    /// Return the node id that is the least common parent between 2 node ids
    fn find_intersection_node_id(&self, node_id1: &NodeId, node_id2: &NodeId) -> Option<NodeId> {
        let mut ancestors1: Vec<NodeId> = node_id1.ancestors(&self.arena).collect();
        let mut ancestors2: Vec<NodeId> = node_id2.ancestors(&self.arena).collect();
        ancestors1.reverse();
        ancestors2.reverse();

        ancestors1
            .into_iter()
            .zip(ancestors2)
            .take_while(|(n1, n2)| n1 == n2)
            .last()
            .map(|ns| ns.0)
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
    fn empty() {
        let tree: HeadersTree<FakeHeader> = HeadersTree::new(vec![]);
        assert_eq!(
            tree.best_chain_tip(),
            None,
            "there is not best chain for an empty tree yet"
        );
    }

    #[test]
    fn single_chain_is_best_chain() {
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
    fn initialize_peer_on_known_point() {
        let (mut tree, headers) = create_headers_tree(5);

        let peer = Peer::new("alice");

        let last = headers.last().unwrap();
        let peer_point = Point::Specific(10, last.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        assert_eq!(tree.get_tip_for(&peer).unwrap(), Some(*last).as_ref());
    }

    #[test]
    fn initialize_peer_on_unknown_point_fails() {
        let mut tree = create_headers_tree(5).0;

        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, random_bytes(HEADER_HASH_SIZE));
        assert!(tree.initialize_peer(&peer, &peer_point).is_err());
    }

    #[test]
    fn roll_forward_extends_best_chain() {
        let (mut tree, mut headers) = create_headers_tree(5);
        let tip = headers.last().unwrap();

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        assert_eq!(
            tree.select_roll_forward(&peer, new_tip).unwrap(),
            ForwardChainSelection::NewTip { peer, tip: new_tip }
        );
        assert_eq!(tree.best_chain_tip(), Some(&new_tip));

        headers.push(new_tip);
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&FakeHeader>>()
        );
    }

    #[test]
    fn roll_forward_with_incorrect_parent_fails() {
        let (mut tree, headers) = create_headers_tree(5);
        let tip = headers.last().unwrap();

        // create a new tip pointing to an incorrect parent (the first header of the chain in this example)
        let new_tip = make_header_with_parent(headers.first().unwrap());

        // initialize alice as a peer with last known header = 5
        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        // Now roll forward with the 6th block
        assert!(tree.select_roll_forward(&peer, new_tip).is_err());
    }

    #[test]
    fn roll_forward_from_unknown_peer_fails() {
        let (mut tree, headers) = create_headers_tree(5);
        let last = headers.last().unwrap();

        let peer = Peer::new("alice");
        let peer_point = Point::Specific(10, last.hash().to_vec());
        tree.initialize_peer(&peer, &peer_point).unwrap();

        // Now roll forward with an unknown peer
        let peer = Peer::new("bob");
        assert!(tree.select_roll_forward(&peer, *last).is_err());
    }

    // FIXME: a peer should not stutter when rolling forward, unless we
    // reconnect.
    #[test]
    fn roll_forward_is_idempotent() {
        let peer = Peer::new("alice");
        let (mut tree, mut headers) = initialize_with_peer(5, &peer);
        let new_tip = make_header_with_parent(headers.last().unwrap());
        // Roll forward twice with the same header
        assert_eq!(
            tree.select_roll_forward(&peer, new_tip).unwrap(),
            ForwardChainSelection::NewTip {
                peer: peer.clone(),
                tip: new_tip
            }
        );
        assert_eq!(
            tree.select_roll_forward(&peer, new_tip).unwrap(),
            ForwardChainSelection::NoChange
        );

        assert_eq!(tree.best_chain_tip(), Some(&new_tip));
        headers.push(new_tip);
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&FakeHeader>>()
        );
    }

    #[test]
    fn roll_forward_from_another_peer_at_tip_extends_best_chain() {
        let alice = Peer::new("alice");
        let (mut tree, mut headers) = initialize_with_peer(5, &alice);

        // Initialize bob with the same headers as alice
        let bob = Peer::new("bob");
        let tip = headers.last().unwrap();
        let new_tip = make_header_with_parent(tip);
        tree.initialize_peer(&bob, &tip.point()).unwrap();

        // Roll forward with a new header from bob, on the same chain
        assert_eq!(
            tree.select_roll_forward(&bob, new_tip).unwrap(),
            ForwardChainSelection::NewTip {
                peer: bob,
                tip: new_tip
            }
        );
        assert_eq!(tree.best_chain_tip(), Some(&new_tip));

        headers.push(new_tip);
        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&FakeHeader>>()
        );
    }

    #[test]
    fn roll_forward_from_another_peer_on_a_smaller_chain_is_noop() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

        // Initialize bob with the less headers than alice
        let bob = Peer::new("bob");
        let middle = headers.get(2).unwrap();
        tree.initialize_peer(&bob, &middle.point()).unwrap();

        // Roll forward with a new header from bob
        let new_tip_for_bob = make_header_with_parent(middle);
        assert_eq!(
            tree.select_roll_forward(&bob, new_tip_for_bob).unwrap(),
            ForwardChainSelection::NoChange
        );
        let tip = headers.last().unwrap();
        assert_eq!(
            tree.best_chain_tip(),
            Some(tip),
            "the current tip must not change"
        );

        assert_eq!(
            tree.best_chain_fragment(),
            headers.iter().collect::<Vec<&FakeHeader>>(),
            "the best chain hasn't changed"
        );
    }

    #[test]
    fn roll_forward_from_another_peer_on_a_fork() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

        // Initialize bob with some headers common with alice + additional headers that are different
        // so that their chains have the same length
        let bob = Peer::new("bob");
        let middle = headers.get(2).unwrap();
        tree.initialize_peer(&bob, &middle.point()).unwrap();

        // Roll forward with 2 new headers from bob
        let bob_new_header1 = make_header_with_parent(middle);
        let bob_new_header2 = make_header_with_parent(&bob_new_header1);
        tree.select_roll_forward(&bob, bob_new_header1).unwrap();
        tree.select_roll_forward(&bob, bob_new_header2).unwrap();

        // Adding a new header must create a fork
        let bob_new_header3 = make_header_with_parent(&bob_new_header2);
        let fork: Vec<FakeHeader> = vec![bob_new_header1, bob_new_header2, bob_new_header3];
        let fork = Fork {
            peer: bob.clone(),
            rollback_point: middle.point(),
            fork,
        };

        assert_eq!(
            tree.select_roll_forward(&bob, bob_new_header3).unwrap(),
            ForwardChainSelection::SwitchToFork(fork)
        );
    }

    // TODO: that's where things become insteresting: the fork should still
    // be anchored on a known header and shorter than k
    #[test]
    fn roll_forward_with_fork_to_a_disjoint_chain() {
        let alice = Peer::new("alice");
        let mut tree = initialize_with_peer(5, &alice).0;

        // Initialize bob with a completely different chain of the same size
        let bob = Peer::new("bob");
        let mut bob_headers = generate_headers_anchored_at(None, 5);
        let bob_tip = bob_headers.last().unwrap();
        tree.insert_headers(bob_headers.clone());
        tree.initialize_peer(&bob, &bob_tip.point()).unwrap();

        // Adding a new header for bob must create a fork
        let bob_new_tip = make_header_with_parent(bob_tip);
        bob_headers.push(bob_new_tip);
        let fork = Fork {
            peer: bob.clone(),
            rollback_point: Point::Origin,
            fork: bob_headers,
        };

        assert_eq!(
            tree.select_roll_forward(&bob, bob_new_tip).unwrap(),
            ForwardChainSelection::SwitchToFork(fork)
        );
    }

    /// HELPERS
    fn create_headers_tree(size: u32) -> (HeadersTree<FakeHeader>, Vec<FakeHeader>) {
        let headers = generate_headers_anchored_at(None, size);
        let tree = HeadersTree::new(headers.clone());
        (tree, headers)
    }

    fn initialize_with_peer(size: u32, peer: &Peer) -> (HeadersTree<FakeHeader>, Vec<FakeHeader>) {
        let (mut tree, headers) = create_headers_tree(size);
        let tip = headers.last().unwrap();
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(peer, &peer_point).unwrap();
        (tree, headers)
    }

    fn make_header_with_parent(parent: &FakeHeader) -> FakeHeader {
        FakeHeader {
            block_number: parent.block_number + 1,
            slot: 0,
            parent: Some(parent.hash()),
            body_hash: random_bytes(HEADER_HASH_SIZE).as_slice().into(),
        }
    }
}
