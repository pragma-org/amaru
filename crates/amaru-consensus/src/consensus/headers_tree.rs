use crate::consensus::chain_selection::{Fork, ForwardChainSelection};
use crate::consensus::headers_tree::ChainTracker::{Me, Other};
use crate::consensus::tip::Tip;
use crate::peer::Peer;
use crate::ConsensusError;
use amaru_kernel::{Point, HEADER_HASH_SIZE};
use amaru_ouroboros_traits::IsHeader;
use indextree::{Arena, Node, NodeId};
use pallas_crypto::hash::Hash;
use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use tracing::debug;

const ORIGIN_HASH : Hash<HEADER_HASH_SIZE> = Hash::new([0; HEADER_HASH_SIZE]);

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
    arena: Arena<Tip<H>>,
    /// maximum size allowed for a given chain
    max_length: usize,
    /// This map maintains the chain tracking data for each tracker
    trackers: BTreeMap<ChainTracker, TrackingData>,
}

/// A ChainTracker is a node (generally upstream) which tracks the state of the best chain.
#[derive(Debug, Clone, PartialOrd, PartialEq, Eq, Ord)]
enum ChainTracker {
    Me,
    Other(Peer),
}

impl Display for ChainTracker {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Me => write!(f, "me"),
            Other(p) => write!(f, "{}", p),
        }
    }
}

/// This data type tracks the chain of a tracker inside the arena:
///
///  - Where it stops: tip
///  - The chain length
#[derive(Debug, Clone)]
struct TrackingData {
    tip: NodeId,
    length: usize,
}

#[allow(dead_code)]
impl<H: IsHeader + Clone + std::fmt::Debug> HeadersTree<H> {
    /// Initialize a HeadersTree as a tree rooted in the Genesis header
    pub fn new(max_length: usize, capacity: usize) -> HeadersTree<H> {
        assert!(
            max_length >= 2,
            "Cannot create a headers tree with maximum chain length lower than 2"
        );

        // Create a new arena
        let mut arena: Arena<Tip<H>> = Arena::with_capacity(capacity);
        let genesis_node_id = arena.new_node(Tip::Genesis);
        let mut trackers = BTreeMap::new();
        let tracker_data = TrackingData {
            tip: genesis_node_id,
            length: 0,
        };
        trackers.insert(Me, tracker_data);

        HeadersTree {
            arena,
            max_length,
            trackers,
        }
    }

    /// Return the headers tree size in terms of how many headers are being tracked.
    /// This is used to check the garbage collection aspect of this data structure.
    #[cfg(test)]
    fn size(&self) -> usize {
        self.arena.count()
    }

    /// Pretty-print the headers present in the arena as a tree
    ///
    /// Shows a graphical representation of the internal structure of
    /// the tree.
    /// ```text
    /// Hdr(FakeHeader { block_number: 1, slot: 7, parent: None, body_hash: Hash<32>("2cabe6ea") })
    /// |-- Hdr(FakeHeader { block_number: 2, slot: 22, parent: Some(Hash<32>("d4f3cf2e")), body_hash: Hash<32>("cd932b1e") })
    /// |   `-- Hdr(FakeHeader { block_number: 3, slot: 23, parent: Some(Hash<32>("f72dbcd2")), body_hash: Hash<32>("5b466114") })
    /// |       `-- Hdr(FakeHeader { block_number: 4, slot: 26, parent: Some(Hash<32>("a0326a71")), body_hash: Hash<32>("993e8517") })
    /// |           `-- Hdr(FakeHeader { block_number: 5, slot: 28, parent: Some(Hash<32>("b452d00f")), body_hash: Hash<32>("9d341c29") })
    /// |               `-- Hdr(FakeHeader { block_number: 6, slot: 93, parent: Some(Hash<32>("143b3c68")), body_hash: Hash<32>("35894500") })
    /// |                   `-- Hdr(FakeHeader { block_number: 7, slot: 111, parent: Some(Hash<32>("448fa5c3")), body_hash: Hash<32>("b18964dc") })
    /// |                       `-- Hdr(FakeHeader { block_number: 8, slot: 115, parent: Some(Hash<32>("c3f7d827")), body_hash: Hash<32>("db0a9b64") })
    /// |                           `-- Hdr(FakeHeader { block_number: 9, slot: 124, parent: Some(Hash<32>("f1a8d8ce")), body_hash: Hash<32>("25a75c75") })
    /// |                               `-- Hdr(FakeHeader { block_number: 10, slot: 151, parent: Some(Hash<32>("17254ace")), body_hash: Hash<32>("9f3acd52") })
    /// `-- Hdr(FakeHeader { block_number: 2, slot: 0, parent: Some(Hash<32>("d4f3cf2e")), body_hash: Hash<32>("24115f11") })
    ///     `-- Hdr(FakeHeader { block_number: 3, slot: 0, parent: Some(Hash<32>("83ee63d6")), body_hash: Hash<32>("7950c684") })
    ///```
    fn pretty_print(&self) -> String {
        self.best_chain()
            .ancestors(&self.arena)
            .last()
            .map(|root| format!("{:?}", root.debug_pretty_print(&self.arena)))
            .unwrap_or("".to_string())
    }

    fn best_chain(&self) -> NodeId {
        self.trackers
            .get(&Me)
            .expect("The headers tree should always have a node id for Me")
            .tip
    }

    fn set_best_chain(&mut self, node_id: NodeId) {
        let my_data = self
            .trackers
            .get_mut(&Me)
            .expect("The headers tree should always have a node id for Me");
        my_data.tip = node_id;
    }

    /// Insert headers into the arena and return the last created node id
    #[cfg(test)]
    fn insert_headers(&mut self, headers: &Vec<H>) -> NodeId {
        let mut iter = headers.into_iter();
        if let Some(first) = iter.next() {
            let rest: Vec<_> = iter.collect();
            let mut last_node_id: NodeId = self.arena.new_node(Tip::Hdr(first.clone()));

            for header in rest {
                let new_node_id = self.arena.new_node(Tip::Hdr(header.clone()));
                last_node_id.append(new_node_id, &mut self.arena);
                last_node_id = new_node_id;
            }
            self.set_best_chain(last_node_id);
        };
        self.best_chain()
    }

    /// Return the tip of the best chain that currently known
    pub fn best_chain_tip(&self) -> Option<&H> {
        self.arena
            .get(self.best_chain())
            .and_then(|n| n.get().to_header())
    }

    /// Return best chain fragment currently known
    pub fn best_chain_fragment(&self) -> Vec<&H> {
        self.get_chain_from(&self.best_chain())
    }

    pub fn select_roll_forward(
        &mut self,
        peer: &Peer,
        header: H,
    ) -> Result<ForwardChainSelection<H>, ConsensusError> {
        let tracker = Other(peer.clone());
        let peer_tip = match self.trackers.get(&tracker) {
            Some(peer_chain) => peer_chain.tip,
            None => return Err(ConsensusError::UnknownPeer(peer.clone())),
        };
        let peer_tip_node = self.unsafe_get_arena_node(peer_tip);
        let peer_tip_node_hash = peer_tip_node.get().hash();
        if header.hash() == peer_tip_node_hash {
            Ok(ForwardChainSelection::NoChange)
        } else if header.parent() == Some(peer_tip_node_hash) {
            let header_node_id = self.insert_header(&tracker, header.clone(), &peer_tip);
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
            .filter_map(|n_id| self.arena.get(n_id).and_then(|n| n.get().to_header()))
            .collect();
        chain.reverse();
        chain
    }

    /// Return an arena node when it is expected to be found
    #[allow(clippy::panic)]
    fn unsafe_get_arena_node(&self, node_id: NodeId) -> &Node<Tip<H>> {
        self.arena.get(node_id).unwrap_or_else(|| {
            panic!(
                "Node not found in the arena {}. The arena is {:?}",
                node_id, self.arena
            )
        })
    }

    /// Insert a new header in the arena and maintain:
    ///  - The peer tip
    ///  - The chain length
    #[allow(clippy::panic)]
    fn insert_header(
        &mut self,
        tracker: &ChainTracker,
        header: H,
        parent_node_id: &NodeId,
    ) -> NodeId {
        let mut peer_chain = self
            .trackers
            .get(tracker)
            .unwrap_or_else(|| panic!("no chain information found for peer {tracker}"))
            .clone();
        // If the current chain (before the new header) is already at the maximum length
        // we need to move the anchor point one header up in the chain
        if peer_chain.length == self.max_length {
            self.prune_unreachable_nodes(parent_node_id);
        } else {
            peer_chain.length += 1;
        };

        let header_node_id = self.arena.new_node(Tip::Hdr(header.clone()));
        parent_node_id.append(header_node_id, &mut self.arena);
        peer_chain.tip = header_node_id;
        self.trackers.insert(tracker.clone(), peer_chain);
        header_node_id
    }

    #[allow(clippy::panic)]
    fn prune_unreachable_nodes(&mut self, parent_node_id: &NodeId) {
        let ancestors = parent_node_id
            .ancestors(&self.arena)
            .collect::<Vec<NodeId>>();

        match ancestors[ancestors.len() - 2..ancestors.len()] {
            [new_root, root] => {
                let to_remove = root
                    .children(&self.arena)
                    .filter(|nid| *nid != new_root)
                    .collect::<Vec<NodeId>>();
                // FIXME: drop any peer using that node id
                for nid in to_remove {
                    nid.remove_subtree(&mut self.arena);
                }
                root.remove(&mut self.arena);
            }
            _ => panic!("This chain selection tree is configured with a maximum chain length lower than 2 ({}), which does not make sense", self.max_length),
        }
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
        // If we added the new node on top of the current best chain, we have a new tip
        if *parent_node_id == self.best_chain() {
            self.set_best_chain(*header_node_id);
            ForwardChainSelection::NewTip {
                peer: peer.clone(),
                tip: header,
            }
        } else {
            let current_tip_header = self.unsafe_get_arena_node(self.best_chain()).get();
            // If the new header does not improve the current chain height we keep the same best chain
            if header.block_height() <= current_tip_header.block_height() {
                ForwardChainSelection::NoChange
            } else {
                // Otherwise, if the new header creates a longer chain, we have a fork

                // The rollback point is the intersection of the previous best chain and the new one.
                // The fork_fragment is the list of header that must be recreated after the rollback point.
                let intersection_node_id: Option<NodeId> =
                    self.find_intersection_node_id(header_node_id, &self.best_chain());

                // We set the new best chain
                self.set_best_chain(*header_node_id);

                let mut fork_fragment: Vec<H> = header_node_id
                    .ancestors(&self.arena)
                    .take_while(|n| Some(*n) != intersection_node_id)
                    .filter_map(|n| self.arena.get(n).and_then(|n| n.get().to_header().cloned()))
                    .collect();
                fork_fragment.reverse();
                let fork = Fork {
                    peer: peer.clone(),
                    rollback_point: intersection_node_id
                        .and_then(|n| {
                            self.arena
                                .get(n)
                                .and_then(|n| n.get().to_header().map(|h| h.point()))
                        })
                        .unwrap_or(Point::Origin),
                    fork: fork_fragment,
                };
                ForwardChainSelection::SwitchToFork(fork)
            }
        }
    }

    /// Return the chain tip for a given Peer
    fn get_tip_for(&self, peer: &Peer) -> Result<Option<&H>, ConsensusError> {
        let node_id = self
            .trackers
            .get(&Other(peer.clone()))
            .ok_or_else(|| ConsensusError::UnknownPeer(peer.clone()))?
            .tip;
        Ok(self
            .arena
            .get(node_id)
            .and_then(|node| node.get().to_header()))
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
                    // The anchor of the peer chain is the root of the ancestors retrieved from the tip of the chain
                    let ancestors: Vec<NodeId> = node_id.ancestors(&self.arena).collect();
                    let peer_chain = TrackingData {
                        tip: node_id,
                        length: ancestors.len(),
                    };
                    self.trackers.insert(Other(peer.clone()), peer_chain);
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
    use test_macros::must_panic;

    #[test]
    fn empty() {
        let tree: HeadersTree<FakeHeader> = HeadersTree::new(10, 100);
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
        let mut tree = HeadersTree::new(10, 100);
        tree.insert_headers(&headers);

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
    fn roll_forward_from_another_peer_on_the_best_chain_is_noop() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);

        // Initialize bob with same chain than alice minus the last header
        let bob = Peer::new("bob");
        let bob_tip = headers.get(3).unwrap();
        tree.initialize_peer(&bob, &bob_tip.point()).unwrap();

        // Roll forward with a new header from bob
        let new_tip_for_bob = make_header_with_parent(bob_tip);
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
        let mut new_bob_headers = rollforward_from(&mut tree, middle, &bob, 2);

        // Adding a new header must create a fork
        let bob_new_header3 = make_header_with_parent(&new_bob_headers.last().unwrap());
        new_bob_headers.push(bob_new_header3);
        let fork: Vec<FakeHeader> = new_bob_headers;
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

    #[test]
    fn roll_forward_with_fork_to_a_disjoint_chain() {
        let alice = Peer::new("alice");
        let (mut tree, headers) = initialize_with_peer(5, &alice);
        let anchor = headers.first().unwrap();

        // Initialize bob with a completely different chain of the same size
        let bob = Peer::new("bob");
        tree.initialize_peer(&bob, &anchor.point()).unwrap();
        let mut bob_headers = rollforward_from(&mut tree, anchor, &bob, 4);
        let bob_tip = bob_headers.last().unwrap();

        // Adding a new header for bob must create a fork
        let bob_new_tip = make_header_with_parent(bob_tip);
        bob_headers.push(bob_new_tip);
        let fork = Fork {
            peer: bob.clone(),
            rollback_point: anchor.point(),
            fork: bob_headers,
        };

        assert_eq!(
            tree.select_roll_forward(&bob, bob_new_tip).unwrap(),
            ForwardChainSelection::SwitchToFork(fork)
        );
    }

    #[test]
    fn roll_forward_beyond_max_length() {
        // create a chain at max length
        let (mut tree, mut headers) = create_headers_tree(10);
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&alice, &peer_point).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        assert_eq!(
            tree.select_roll_forward(&alice, new_tip).unwrap(),
            ForwardChainSelection::NewTip {
                peer: alice,
                tip: new_tip
            }
        );
        assert_eq!(tree.best_chain_tip(), Some(&new_tip));

        // The expected chain ends with the new tip but starts with the second header
        // in order to stay below max_length
        headers.push(new_tip);
        let expected = headers.split_off(1);
        let expected = expected.iter().collect::<Vec<&FakeHeader>>();
        assert_eq!(tree.best_chain_fragment(), expected);
    }

    #[test]
    fn roll_forward_with_one_chain_an_unreachable_anchor_must_be_dropped() {
        // create a chain at max length
        let (mut tree, headers) = create_headers_tree(10);
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&alice, &peer_point).unwrap();

        // Now roll forward extending tip
        let new_tip = make_header_with_parent(tip);
        _ = tree.select_roll_forward(&alice, new_tip).unwrap();

        // The original chain anchor is still present in the arena but marked as removed (its data is dropped)
        assert_eq!(tree.size(), 11);

        // If we add a new header to the current chain, the arena reuses the removed node to store
        // the new header
        let new_tip = make_header_with_parent(&new_tip.clone());
        _ = tree.select_roll_forward(&alice, new_tip).unwrap();
        assert_eq!(tree.size(), 11);
    }

    #[test]
    fn roll_forward_beyond_limit_drops_unreachable_chain() {
        // create a chain at max length
        let (mut tree, headers) = create_headers_tree(10);
        let tip = headers.last().unwrap();

        let alice = Peer::new("alice");
        let peer_point = Point::Specific(10, tip.hash().to_vec());
        tree.initialize_peer(&alice, &peer_point).unwrap();

        let bob = Peer::new("bob");
        let first = headers.first().unwrap();
        tree.initialize_peer(&bob, &first.point()).unwrap();

        // Roll forward with 2 new headers from bob
        _ = rollforward_from(&mut tree, first, &bob, 2);

        // Now roll forward alice's tip twice
        // The tree must not grow in size because bob's arena nodes have been reused
        _ = rollforward_from(&mut tree, tip, &alice, 2);
        assert_eq!(tree.size(), 13);
    }

    #[test]
    #[must_panic(expected = "Cannot create a headers tree with maximum chain length lower than 2")]
    fn cannot_initialize_tree_with_k_lower_than_2() {
        HeadersTree::<FakeHeader>::new(1, 1000);
    }

    /// HELPERS
    fn create_headers_tree(size: u32) -> (HeadersTree<FakeHeader>, Vec<FakeHeader>) {
        let mut tree = HeadersTree::new(10, 100);
        let headers = generate_headers_anchored_at(None, size);
        _ = tree.insert_headers(&headers);
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

    /// Roll forward the tree:
    ///  - Starting from a given header,
    ///  - For a given peer
    ///  - A number of times
    ///
    /// And return the last created header
    fn rollforward_from(
        tree: &mut HeadersTree<FakeHeader>,
        header: &FakeHeader,
        peer: &Peer,
        times: u32,
    ) -> Vec<FakeHeader> {
        let mut result = vec![];
        let mut parent = header.clone();
        for _ in 0..times {
            let next_header = make_header_with_parent(&parent);
            tree.select_roll_forward(peer, next_header).unwrap();
            result.push(next_header);
            parent = next_header.clone();
        }
        result
    }
}
