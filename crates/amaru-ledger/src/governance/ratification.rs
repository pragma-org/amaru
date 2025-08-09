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

use crate::store::columns::proposals;
use amaru_kernel::{
    display_protocol_parameters_update, expect_stake_credential, ComparableProposalId,
    Constitution, Epoch, GovAction, Lovelace, Nullable, ProposalId, ProtocolParamUpdate,
    ProtocolVersion, ScriptHash, StakeCredential, UnitInterval,
};
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    rc::Rc,
};

// Top-level logic
// ----------------------------------------------------------------------------

pub fn ratify_proposals(mut proposals: Vec<(ProposalId, proposals::Row)>, _epoch: Epoch) {
    proposals.sort_by(|a, b| a.1.proposed_in.cmp(&b.1.proposed_in));

    let forest = proposals
        .drain(..)
        .fold(ProposalsForest::empty(), |mut forest, (id, row)| {
            forest
                .insert(ComparableProposalId::from(id), row.proposal.gov_action)
                // FIXME: Bubble this up. There should be no error here; this is a sign of a ledger
                // rule violation. It can only mean that a proposal was accepted without having an
                // existing parent.
                .unwrap_or_else(|e| panic!("{e}"));
            forest
        });

    println!("==================== RATIFYING PROPOSALS ====================\n\n{forest}\n\n");
}

// Proposals
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub enum CommitteeUpdate {
    NoConfidence,
    ChangeMembers {
        removed: BTreeSet<StakeCredential>,
        added: BTreeMap<StakeCredential, Epoch>,
        threshold: UnitInterval,
    },
}

impl fmt::Display for CommitteeUpdate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoConfidence => write!(f, "no-confidence"),
            Self::ChangeMembers {
                removed,
                added,
                threshold,
            } => {
                let mut need_separator = false;

                if !removed.is_empty() {
                    write!(f, "{} removed", removed.len())?;
                    need_separator = true;
                }

                if !added.is_empty() {
                    write!(
                        f,
                        "{}{} added",
                        if need_separator { ", " } else { "" },
                        added.len()
                    )?;
                    need_separator = true;
                }

                write!(
                    f,
                    "{}threshold={}/{}",
                    if need_separator { ", " } else { "" },
                    threshold.numerator,
                    threshold.denominator,
                )
            }
        }
    }
}

#[derive(Debug)]
pub enum OrphanProposal {
    TreasuryWithdrawal {
        withdrawals: BTreeMap<StakeCredential, Lovelace>,
        guardrails: Option<ScriptHash>,
    },
    NicePoll,
}

impl fmt::Display for OrphanProposal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrphanProposal::NicePoll => write!(f, "nice poll"),
            OrphanProposal::TreasuryWithdrawal { withdrawals, .. } => {
                let total = withdrawals
                    .iter()
                    .fold(0, |total, (_, single)| total + single)
                    / 1_000_000;
                write!(f, "withdrawal={total}₳")
            }
        }
    }
}

// ProposalsTree
// ----------------------------------------------------------------------------

/// A data-structure for holding arbitrary chains of proposals of the same kind.
///
/// Each proposal has a parent (although, the very first proposal haven't, hence the Option) and
/// may have 0, 1 or many children. A child node is one whose parent's proposal is the parent's
/// node.
///
#[derive(Debug)]
pub enum ProposalsTree<A> {
    Empty,
    Node {
        parent: Option<ComparableProposalId>,
        siblings: Vec<Sibling<A>>,
    },
}

#[derive(Debug)]
pub struct Sibling<A> {
    pub id: ComparableProposalId,
    pub proposal: A,
    pub children: Vec<ProposalsTree<A>>,
}

impl<A> Sibling<A> {
    pub fn new(id: ComparableProposalId, proposal: A) -> Self {
        Self {
            id,
            proposal,
            children: vec![],
        }
    }
}

impl<A: fmt::Debug> ProposalsTree<A> {
    pub fn insert(
        &mut self,
        id: ComparableProposalId,
        parent: Option<ComparableProposalId>,
        proposal: A,
    ) -> Result<(), ProposalsInsertError<A>> {
        use ProposalsInsertError::*;

        match self {
            ProposalsTree::Empty => {
                *self = ProposalsTree::Node {
                    parent,
                    siblings: vec![Sibling::new(id, proposal)],
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
                    siblings.push(Sibling::new(id, proposal));
                    return Ok(());
                }

                // Otherwise, we do a (depth-first) search for the parent. Note that ideally, we
                // should perform a breadth-first search here because we do generally expect more
                // proposals in the earlier levels of the tree. But we don't expect _that many_
                // proposals anyway due to the high deposit. So even a depth-first search should
                // perform reasonably okay.
                //
                // Besides, they have similar worst-case performances.

                let initial_state = Err(UnknownParent {
                    id,
                    parent,
                    proposal,
                });

                siblings.iter_mut().fold(initial_state, |needle, sibling| {
                    needle.or_else(
                        |UnknownParent {
                             id,
                             parent,
                             proposal,
                         }| {
                            // One of the sibling at this level has the same id as the proposal's
                            // parent, so it is the parent. We can stop the search.
                            if Some(&sibling.id) == parent.as_ref() {
                                sibling.children.push(ProposalsTree::Node {
                                    parent: Some(sibling.id.clone()),
                                    siblings: vec![Sibling::new(id, proposal)],
                                });
                                return Ok(());
                            }

                            // Otherwise, we must check children all children of that sibling.
                            let needle = Err(UnknownParent {
                                id,
                                parent,
                                proposal,
                            });

                            sibling.children.iter_mut().fold(needle, |needle, child| {
                                needle.or_else(
                                    |UnknownParent {
                                         id,
                                         parent,
                                         proposal,
                                     }| {
                                        child.insert(id, parent, proposal)
                                    },
                                )
                            })
                        },
                    )
                })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProposalsInsertError<A> {
    #[error("proposal {id:?} -> {proposal:?} has an unknown parent {parent:?}")]
    UnknownParent {
        id: ComparableProposalId,
        parent: Option<ComparableProposalId>,
        proposal: A,
    },
}

impl<A: fmt::Debug + 'static> ProposalsInsertError<A> {
    pub fn generalize(self) -> ProposalsInsertError<Box<dyn fmt::Debug + 'static>> {
        match self {
            Self::UnknownParent {
                id,
                parent,
                proposal,
            } => ProposalsInsertError::UnknownParent {
                id,
                parent,
                proposal: Box::new(proposal),
            },
        }
    }
}

// ProposalsForest
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub struct ProposalsForest {
    pub protocol_parameters_updates: ProposalsTree<ProtocolParamUpdate>,
    pub hard_forks: ProposalsTree<ProtocolVersion>,
    pub constitutional_committee_updates: ProposalsTree<CommitteeUpdate>,
    pub constitution_updates: ProposalsTree<Constitution>,
    pub others: Vec<OrphanProposal>,
}

impl ProposalsForest {
    pub fn empty() -> Self {
        ProposalsForest {
            protocol_parameters_updates: ProposalsTree::Empty,
            hard_forks: ProposalsTree::Empty,
            constitutional_committee_updates: ProposalsTree::Empty,
            constitution_updates: ProposalsTree::Empty,
            others: vec![],
        }
    }

    pub fn insert(
        &mut self,
        id: ComparableProposalId,
        proposal: GovAction,
    ) -> Result<(), ProposalsInsertError<Box<dyn fmt::Debug>>> {
        use amaru_kernel::GovAction::*;
        match proposal {
            ParameterChange(parent, update, _guardrails_script) => self
                .protocol_parameters_updates
                .insert(id, into_parent_id(parent), *update)
                .map_err(ProposalsInsertError::generalize),

            HardForkInitiation(parent, protocol_version) => self
                .hard_forks
                .insert(id, into_parent_id(parent), protocol_version)
                .map_err(ProposalsInsertError::generalize),

            TreasuryWithdrawals(withdrawals, guardrails_script) => {
                let withdrawals = withdrawals.to_vec().into_iter().fold(
                    BTreeMap::new(),
                    |mut accum, (reward_account, amount)| {
                        accum.insert(expect_stake_credential(&reward_account), amount);
                        accum
                    },
                );

                self.others.push(OrphanProposal::TreasuryWithdrawal {
                    withdrawals,
                    guardrails: Option::from(guardrails_script),
                });

                Ok(())
            }

            UpdateCommittee(parent, removed, added, threshold) => self
                .constitutional_committee_updates
                .insert(
                    id,
                    into_parent_id(parent),
                    CommitteeUpdate::ChangeMembers {
                        removed: removed.to_vec().into_iter().collect(),
                        added: added
                            .to_vec()
                            .into_iter()
                            .map(|(k, v)| (k, Epoch::from(v)))
                            .collect(),
                        threshold,
                    },
                )
                .map_err(ProposalsInsertError::generalize),

            NoConfidence(parent) => self
                .constitutional_committee_updates
                .insert(id, into_parent_id(parent), CommitteeUpdate::NoConfidence)
                .map_err(ProposalsInsertError::generalize),

            NewConstitution(parent, constitution) => self
                .constitution_updates
                .insert(id, into_parent_id(parent), constitution)
                .map_err(ProposalsInsertError::generalize),

            Information => {
                self.others.push(OrphanProposal::NicePoll);
                Ok(())
            }
        }
    }
}

impl fmt::Display for ProposalsForest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // ---- Renderers -----------------------------------------------------

        // Prints a section header + the tree underneath.
        fn section<A: 'static>(
            f: &mut fmt::Formatter<'_>,
            title: &str,
            summarize: Rc<dyn Fn(&A, &str, bool) -> Result<String, fmt::Error>>,
            tree: &ProposalsTree<A>,
        ) -> fmt::Result {
            if matches!(tree, ProposalsTree::Empty) {
                return Ok(());
            }
            writeln!(f, "{title}")?;
            render_tree(f, summarize, tree)?;
            writeln!(f)?;
            Ok(())
        }

        // Renders a whole tree (which is just a bag of siblings at each Node).
        fn render_tree<A: 'static>(
            f: &mut fmt::Formatter<'_>,
            summarize: Rc<dyn Fn(&A, &str, bool) -> Result<String, fmt::Error>>,
            tree: &ProposalsTree<A>,
        ) -> fmt::Result {
            match tree {
                ProposalsTree::Empty => Ok(()),
                ProposalsTree::Node { siblings, .. } => {
                    for (i, s) in siblings.iter().enumerate() {
                        let is_last = i + 1 == siblings.len();
                        render_sibling(f, s, "", summarize.clone(), is_last)?;
                    }
                    Ok(())
                }
            }
        }

        // Render a single sibling + its (flattened) children.
        fn render_sibling<A: 'static>(
            f: &mut fmt::Formatter<'_>,
            s: &Sibling<A>,
            prefix: &str,
            summarize: Rc<dyn Fn(&A, &str, bool) -> Result<String, fmt::Error>>,
            is_last: bool,
        ) -> fmt::Result {
            let branch = if is_last { "└─" } else { "├─" };
            writeln!(
                f,
                "{prefix}{branch} {}: {}",
                s.id.to_string().chars().take(12).collect::<String>(),
                summarize(&s.proposal, prefix, is_last)?,
            )?;

            // Children are a Vec<ProposalsTree<A>>; flatten to a linear list of Sibling<A>
            // to get correct "last" detection for drawing.
            let flat: Vec<&Sibling<A>> = collect_child_siblings(&s.children);
            let next_prefix = if is_last {
                format!("{prefix}   ")
            } else {
                format!("{prefix}│  ")
            };

            for (idx, cs) in flat.iter().enumerate() {
                let last_here = idx + 1 == flat.len();
                render_sibling(f, cs, &next_prefix, summarize.clone(), last_here)?;
            }
            Ok(())
        }

        // Gather all siblings from all non-empty child subtrees, in order.
        fn collect_child_siblings<A>(children: &[ProposalsTree<A>]) -> Vec<&Sibling<A>> {
            let mut out = Vec::new();
            for c in children {
                if let ProposalsTree::Node { siblings, .. } = c {
                    for s in siblings {
                        out.push(s);
                    }
                }
            }
            out
        }

        // ---- Forest printing -----------------------------------------------

        // Print each non-empty category
        section::<ProtocolParamUpdate>(
            f,
            "Protocol Parameter Updates",
            Rc::new(|pp, prefix, is_last| {
                Ok(format!(
                    "\n{}",
                    display_protocol_parameters_update(
                        pp,
                        &format!(
                            "{}    · ",
                            if is_last {
                                format!("{prefix} ")
                            } else {
                                format!("{prefix}│")
                            }
                        )
                    )?
                ))
            }),
            &self.protocol_parameters_updates,
        )?;

        section::<ProtocolVersion>(
            f,
            "Hard forks",
            Rc::new(|protocol_version, _, _| {
                Ok(format!(
                    "version={}.{}",
                    protocol_version.0, protocol_version.1
                ))
            }),
            &self.hard_forks,
        )?;

        section::<CommitteeUpdate>(
            f,
            "Constitutional Committee Updates",
            Rc::new(|committee_update, _, _| Ok(committee_update.to_string())),
            &self.constitutional_committee_updates,
        )?;

        section::<Constitution>(
            f,
            "Constitution updates",
            Rc::new(|constitution, _, _| {
                Ok(format!(
                    "{} with {}>",
                    constitution.anchor.url,
                    match constitution.guardrail_script {
                        Nullable::Some(hash) => format!(
                            "guardrails={}",
                            hash.to_string().chars().take(8).collect::<String>()
                        ),
                        Nullable::Undefined | Nullable::Null => "no guardrails script".to_string(),
                    },
                ))
            }),
            &self.constitution_updates,
        )?;

        // Others, represented as a flat sequence.
        if !self.others.is_empty() {
            writeln!(f, "Others")?;
            for (i, o) in self.others.iter().enumerate() {
                let is_last = i + 1 == self.others.len();
                let branch = if is_last { "└─" } else { "├─" };
                writeln!(f, "{branch} {o}")?;
            }
        }

        Ok(())
    }
}

// Helpers
// ----------------------------------------------------------------------------

fn into_parent_id(nullable: Nullable<ProposalId>) -> Option<ComparableProposalId> {
    match nullable {
        Nullable::Undefined | Nullable::Null => None,
        Nullable::Some(id) => Some(ComparableProposalId::from(id)),
    }
}
