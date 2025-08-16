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

use std::{collections::BTreeSet, rc::Rc};

// ProposalsTree
// ----------------------------------------------------------------------------

/// A data-structure for holding arbitrary chains of proposals of the same kind.
///
/// Each proposal has a parent (although, the very first proposal haven't, hence the Option) and
/// may have 0, 1 or many children. A child node is one whose parent's proposal is the parent's
/// node.
///
#[derive(Debug)]
pub struct ProposalsTree<T> {
    pub root: Option<Rc<T>>,
    pub siblings: Vec<Sibling<T>>,
}

impl<T: Ord> ProposalsTree<T> {
    pub fn new(root: Option<Rc<T>>) -> Self {
        Self {
            root,
            siblings: Vec::new(),
        }
    }

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

                Ok((id, pruned))
            }
        }
    }

    pub fn insert(
        &mut self,
        id: Rc<T>,
        parent: Option<Rc<T>>,
    ) -> Result<(), ProposalsInsertError<T>> {
        use ProposalsInsertError::*;

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

#[derive(Debug)]
pub struct Sibling<T> {
    pub id: Rc<T>,
    pub children: Vec<Sibling<T>>,
}

impl<T: Eq + Ord> Sibling<T> {
    pub fn new(id: Rc<T>) -> Self {
        Self {
            id,
            children: vec![],
        }
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
