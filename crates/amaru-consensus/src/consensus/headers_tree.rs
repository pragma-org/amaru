use amaru_ouroboros_traits::IsHeader;
use indextree::{Arena, NodeId};

pub struct HeadersTree<H> {
    arena: Arena<H>,
    best_chain: Option<NodeId>,
}

impl<H: IsHeader> HeadersTree<H> {
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
            HeadersTree { arena, best_chain: Some(last_node_id) }
        } else {
            HeadersTree { arena, best_chain: None }
        }
    }

    pub fn best_chain(&self) -> Option<&H> {
        self.best_chain.and_then(|node_id| self.arena.get(node_id).map(|n| n.get()))
    }
}

#[cfg(test)]
mod tests {
    use amaru_ouroboros_traits::fake::FakeHeader;
    use super::*;
    use crate::consensus::chain_selection::tests::generate_headers_anchored_at;

    #[test]
    fn test_empty() {
        let tree: HeadersTree<FakeHeader> = HeadersTree::new(vec![]);
        assert_eq!(tree.best_chain(), None);
    }

    #[test]
    fn test_intersection() {
        let headers = generate_headers_anchored_at(None, 5);
        let last = headers.last().unwrap();
        let tree = HeadersTree::new(headers.clone());

        assert_eq!(tree.best_chain(), Some(last));
    }
}
