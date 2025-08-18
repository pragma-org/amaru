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

use amaru_kernel::Slot;
use slot_arithmetic::EraHistoryError;
use std::{collections::BTreeSet, rc::Rc};

// ProposalsTree
// ----------------------------------------------------------------------------

/// A data-structure for holding arbitrary chains of proposals of the same kind.
///
/// Each proposal has a parent (although, the very first proposal haven't, hence the Option) and
/// may have 0, 1 or many children. A child node is one whose parent's proposal is the parent's
/// node.
#[derive(Debug, Clone)]
pub struct ProposalsTree<T> {
    /// The root of the tree, which *most of the time*, will be `Some<_>`. The only moment it
    /// isn't, is prior to any governance proposal of the tree's type having been enacted. Said
    /// differently, apart from shortly after the bootstrapping phase, we will stop encountering
    /// empty roots.
    root: Option<Rc<T>>,

    /// Siblings represents all proposals that have the same parent; and more specifically, that
    /// have the root as a parent. This feels a bit artificial (and it is), but it having these
    /// here allows defining a nice recursive `Sibling` structure without optional root. The quirks
    /// of bootstrapping are handled here, at the top-level.
    siblings: Vec<Sibling<T>>,

    /// An extra set of already seen ids, mostly used to check for invariant and ensure proper use
    /// of the ProposalsTree API.
    seen: BTreeSet<Rc<T>>,
}

impl<T: Ord + std::fmt::Debug> ProposalsTree<T> {
    /// Create a new empty ProposalsTree from an arbitrary root.
    pub fn new(root: Option<Rc<T>>) -> Self {
        Self {
            root,
            siblings: Vec::new(),
            seen: BTreeSet::new(),
        }
    }

    /// Obtain the current root, which changes each time a proposal is enacted.
    pub fn root(&self) -> Option<Rc<T>> {
        self.root.clone()
    }

    /// Obtain a reference to the root.
    pub fn as_root(&self) -> Option<&T> {
        self.root.as_deref()
    }

    /// View the current top-level siblings (i.e. those that can be enacted).
    pub fn siblings(&self) -> &Vec<Sibling<T>> {
        &self.siblings
    }

    /// Assert whether the tree is empty (i.e. it's only a root).
    pub fn is_empty(&self) -> bool {
        self.siblings.is_empty()
    }

    /// Enact a proposal as new root, collecting all propoals rendered obsolete through that
    /// promotion (that is, all siblings and their children).
    pub fn enact(&mut self, id: Rc<T>) -> Result<(Rc<T>, BTreeSet<Rc<T>>), ProposalsEnactError<T>> {
        use ProposalsEnactError::*;

        match Sibling::partition(std::mem::take(&mut self.siblings), &id) {
            (None, _) => Err(UnknownProposal { id }),

            (Some(new_root), now_obsolete) => {
                self.root = Some(new_root.id);
                self.siblings = new_root.children;

                let mut pruned = BTreeSet::new();
                Sibling::collect_all(&mut pruned, now_obsolete);

                self.seen.remove(&id);

                Ok((id, pruned))
            }
        }
    }

    /// Insert a not-already-inserted proposal in the tree.
    pub fn insert(
        &mut self,
        id: Rc<T>,
        parent: Option<Rc<T>>,
    ) -> Result<(), ProposalsInsertError<T>> {
        use ProposalsInsertError::*;

        assert!(
            self.seen.insert(id.clone()),
            "api misuse: trying to re-insert an already existing element: {id:?}\n{self:?}"
        );

        match (self.root.as_deref(), parent) {
            (Some(..), None) => Err(UnknownParent { id, parent: None }),

            (None, None) => {
                self.siblings.push(Sibling::new(id));
                Ok(())
            }

            (_, Some(parent)) => {
                // If they have the same parent, they are siblings and we're done searching.
                // This is by far, the most common case since proposals will usually end up
                // targetting the 'root' of the tree (that is, the latest-approved proposal).
                if self.root.as_deref() == Some(&parent) {
                    self.siblings.push(Sibling::new(id));
                    return Ok(());
                }

                // Otherwise, we do a (depth-first) search for the parent. Note that ideally, we
                // should perform a breadth-first search here because we do generally expect more
                // proposals in the earlier levels of the tree. But we don't expect _that many_
                // proposals anyway due to the high deposit. So even a depth-first search should
                // perform reasonably okay.
                //
                // Besides, they have similar worst-case performances.
                let st = Err(SiblingInsertError::UnknownParent { id, parent });
                self.siblings
                    .iter_mut()
                    .fold(st, |st, sibling| {
                        st.or_else(|SiblingInsertError::UnknownParent { id, parent }| {
                            sibling.insert(id, parent)
                        })
                    })
                    .map_err(ProposalsInsertError::from)
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProposalsInsertError<T> {
    #[error("proposal {id:?} has an unknown parent {parent:?}")]
    UnknownParent { id: Rc<T>, parent: Option<Rc<T>> },

    #[error("failed to compute epoch from slot {0:?}: {1}")]
    InternalSlotToEpochError(Slot, EraHistoryError),
}

impl<T> From<SiblingInsertError<T>> for ProposalsInsertError<T> {
    fn from(SiblingInsertError::UnknownParent { id, parent }: SiblingInsertError<T>) -> Self {
        Self::UnknownParent {
            id,
            parent: Some(parent),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProposalsEnactError<T> {
    #[error("unknown proposal {id:?}")]
    UnknownProposal { id: Rc<T> },
}

// Sibling
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct Sibling<T> {
    id: Rc<T>,
    children: Vec<Sibling<T>>,
}

impl<T: Eq + Ord> Sibling<T> {
    pub fn new(id: Rc<T>) -> Self {
        Self {
            id,
            children: vec![],
        }
    }

    #[allow(dead_code)] // Actually used in tests.
    pub fn id(&self) -> Rc<T> {
        self.id.clone()
    }

    pub fn as_id(&self) -> &T {
        self.id.as_ref()
    }

    pub fn children(&self) -> &Vec<Sibling<T>> {
        &self.children
    }

    pub fn insert(&mut self, id: Rc<T>, parent: Rc<T>) -> Result<(), SiblingInsertError<T>> {
        use SiblingInsertError::*;

        // One of the sibling at this level has the same id as the proposal's
        // parent, so it is the parent. We can stop the search.
        if self.id.as_ref() == parent.as_ref() {
            self.children.push(Sibling::new(id));
            return Ok(());
        }

        // Otherwise, we must check children of that sibling.
        let st = Err(UnknownParent { id, parent });
        self.children.iter_mut().fold(st, |st, child| {
            st.or_else(|UnknownParent { id, parent }| child.insert(id, parent))
        })
    }

    /// Split a set of nodes based on a pivot sibling. The result contains Some<pivot> if it was
    /// found amongst the node, and the rest of the nodes on the other hand.
    fn partition(nodes: Vec<Self>, pivot: &T) -> (Option<Self>, Vec<Self>) {
        let capacity = nodes.len();
        nodes.into_iter().fold(
            (None, Vec::with_capacity(capacity)),
            |(new_root, mut now_obsolete), sibling| {
                if sibling.id.as_ref() == pivot {
                    (Some(sibling), now_obsolete)
                } else {
                    now_obsolete.push(sibling);
                    (new_root, now_obsolete)
                }
            },
        )
    }

    /// Recursively collect all nodes and their children into a mutable accumulator.
    fn collect_all(accum: &mut BTreeSet<Rc<T>>, nodes: Vec<Self>) {
        nodes.into_iter().for_each(|s| {
            accum.insert(s.id);
            Self::collect_all(accum, s.children);
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SiblingInsertError<T> {
    #[error("proposal {id:?} has an unknown parent {parent:?}")]
    UnknownParent { id: Rc<T>, parent: Rc<T> },
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{ProposalsEnactError, ProposalsTree};
    use proptest::{collection, prelude::*};
    use std::{collections::BTreeSet, rc::Rc};

    proptest! {
        #[test]
        fn prop_cannot_insert_with_unknown_parent((mut tree, next, _) in any_proposals_tree()) {
            let result = tree.insert(Rc::new(next), Some(Rc::new(next + 1)));
            prop_assert!(result.is_err(), "{result:?}");
        }
    }

    proptest! {
        #[test]
        fn prop_enact_promote_as_new_root((tree, _, _) in any_proposals_tree()) {
            let frontier: BTreeSet<_> = tree.siblings().iter().map(|s| s.id.clone()).collect();

            for node in frontier.iter() {
                let mut tree = tree.clone();
                let (id, pruned) = tree.enact(node.clone()).unwrap();

                prop_assert!(
                    id.as_ref() == node.as_ref(),
                    "returned root isn't the enacted elements: {id:?} vs {node:?}"
                );

                let new_root = tree.root();

                prop_assert!(
                    new_root.as_deref() == Some(id.as_ref()),
                    "new stored root isn't the enacted element: {id:?} vs {new_root:?}",
                );

                prop_assert!(
                    frontier.difference(&pruned).collect::<Vec<&Rc<u8>>>() == vec![&id],
                    "invalid pruned set after enacting {id:?} -> {pruned:?}",
                );
            }
        }
    }

    proptest! {
        #[test]
        fn prop_cannot_enact_outside_frontier((tree, _, _) in any_proposals_tree()) {
            let frontier_children: BTreeSet<_> = tree
                .siblings()
                .iter()
                .flat_map(|s| s.children.iter().map(|s| s.id()))
                .collect();

            for node in frontier_children.iter() {
                let result = tree.clone().enact(node.clone());
                prop_assert!(
                    matches!(result, Err(ProposalsEnactError::UnknownProposal { id }) if id == node.clone()),
                    "non-frontier node {node:?} was enacted ?!"
                );
            }
        }
    }

    proptest! {
        #[test]
        fn prop_cannot_enact_unknown_proposal((mut tree, next, _) in any_proposals_tree()) {
            let result = tree.enact(Rc::new(next));
            prop_assert!(result.is_err(), "{result:?}");
        }
    }

    proptest! {
        #[test]
        #[should_panic]
        fn prop_cannot_insert_already_existing_element((mut tree, next, any_parent) in any_proposals_tree()) {
            #[allow(unused_must_use)]
            tree.insert(Rc::new(next - 1), any_parent);
        }
    }

    // Construct a proposal tree (possibly empty), where siblings have arbitrary parents. The tree
    // is constructing by iteratively inserting siblings into a base empty tree whose root is
    // chosen arbitrarily.
    //
    // Siblings' have parents amongst any of the previously inserted sibling (or the root).
    fn any_proposals_tree() -> impl Strategy<Value = (ProposalsTree<u8>, u8, Option<Rc<u8>>)> {
        let any_root = prop_oneof![Just(None), Just(Some(0))];
        let any_parents = collection::vec(any::<u8>(), 0..32);
        let any_parent = any::<u8>();

        (any_root, any_parents, any_parent).prop_map(|(root, parents, some_parent)| {
            let mut next = 1;

            let root = root.map(Rc::new);

            let mut known_parents = vec![root.clone()];
            let mut tree = ProposalsTree::new(root);

            let select = |ixs: &Vec<Option<Rc<u8>>>, ix: u8| {
                ixs.get(ix as usize % ixs.len())
                    .unwrap_or_else(|| panic!("out of bound: {ix} -> {ixs:?}"))
                    .clone()
            };

            for parent in parents {
                let sibling = Rc::new(next);

                tree.insert(sibling.clone(), select(&known_parents, parent))
                    .unwrap();

                known_parents.push(Some(sibling));

                next += 1;
            }

            (tree, next, select(&known_parents, some_parent))
        })
    }
}
