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

use amaru_kernel::ComparableProposalId;
use std::rc::Rc;

/// A data-structure for holding arbitrary chains of proposals of the same kind.
///
/// Each proposal has a parent (although, the very first proposal haven't, hence the Option) and
/// may have 0, 1 or many children. A child node is one whose parent's proposal is the parent's
/// node.
///
#[derive(Debug)]
pub enum ProposalsTree {
    Empty,
    Node {
        parent: Option<Rc<ComparableProposalId>>,
        siblings: Vec<Sibling>,
    },
}

#[derive(Debug)]
pub struct Sibling {
    pub id: Rc<ComparableProposalId>,
    pub children: Vec<ProposalsTree>,
}

impl Sibling {
    pub fn new(id: Rc<ComparableProposalId>) -> Self {
        Self {
            id,
            children: vec![],
        }
    }
}

impl ProposalsTree {
    pub fn insert(
        &mut self,
        id: Rc<ComparableProposalId>,
        parent: Option<Rc<ComparableProposalId>>,
    ) -> Result<(), ProposalsInsertError> {
        use ProposalsInsertError::*;

        match self {
            ProposalsTree::Empty => {
                *self = ProposalsTree::Node {
                    parent,
                    siblings: vec![Sibling::new(id)],
                };
                Ok(())
            }
            ProposalsTree::Node {
                parent: siblings_parent,
                siblings,
            } => {
                // If they have the same parent, they are siblings and we're done searching.
                // This is by far, the most common case since proposals will usually end up
                // targetting the 'root' of the tree (that is, the latest-approved proposal).
                if siblings_parent == &parent {
                    siblings.push(Sibling::new(id));
                    return Ok(());
                }

                // Otherwise, we do a (depth-first) search for the parent. Note that ideally, we
                // should perform a breadth-first search here because we do generally expect more
                // proposals in the earlier levels of the tree. But we don't expect _that many_
                // proposals anyway due to the high deposit. So even a depth-first search should
                // perform reasonably okay.
                //
                // Besides, they have similar worst-case performances.

                let initial_state = Err(UnknownParent { id, parent });

                siblings.iter_mut().fold(initial_state, |needle, sibling| {
                    needle.or_else(|UnknownParent { id, parent }| {
                        // One of the sibling at this level has the same id as the proposal's
                        // parent, so it is the parent. We can stop the search.
                        if Some(sibling.id.as_ref()) == parent.as_deref() {
                            sibling.children.push(ProposalsTree::Node {
                                parent: Some(sibling.id.clone()),
                                siblings: vec![Sibling::new(id)],
                            });
                            return Ok(());
                        }

                        // Otherwise, we must check children all children of that sibling.
                        let needle = Err(UnknownParent { id, parent });

                        sibling.children.iter_mut().fold(needle, |needle, child| {
                            needle.or_else(|UnknownParent { id, parent }| child.insert(id, parent))
                        })
                    })
                })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProposalsInsertError {
    #[error("proposal {id:?} has an unknown parent {parent:?}")]
    UnknownParent {
        id: Rc<ComparableProposalId>,
        parent: Option<Rc<ComparableProposalId>>,
    },
}
