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

// CommitteeUpdate
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

// OrphanProposal
// ----------------------------------------------------------------------------

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

// ProposalEnum
// ----------------------------------------------------------------------------

/// Akin to a GovAction, but with a split that is more tailored to the ratification needs.
/// In particular:
///
/// - Motion of no confidence and update to the constitutional commitee are grouped together as
///   `CommitteeUpdate`. This is because they, in fact, belong to the same chain of
///   relationships.
///
/// - Treasury withdrawals and polls (a.k.a 'info actions') are also grouped together, as they're
///   the only actions that do not need to form a chain; they have no parents (hence,
///   `OrphanProposal`)
#[derive(Debug)]
pub enum ProposalEnum {
    ProtocolParametersUpdate(ProtocolParamUpdate),
    HardFork(ProtocolVersion),
    CommitteeUpdate(CommitteeUpdate),
    Constitution(Constitution),
    Orphan(OrphanProposal),
}

// ProposalsForest
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub struct ProposalsForest {
    /// We keep a map of id -> ProposalEnum. This serves as a lookup table to retrieve proposals
    /// from the forest in a timely manner while the relationships between all proposals is
    /// maintained independently.
    proposals: BTreeMap<Rc<ComparableProposalId>, ProposalEnum>,

    /// The order in which proposals are inserted matters. The forest is an insertion-preserving
    /// structure. Iterating on the forest will yield the proposals in the order they were
    /// inserted.
    sequence: Vec<Rc<ComparableProposalId>>,

    // Finally, the relation between proposals is preserved through multiple tree-like structures.
    // This is what gives this data-structure its name.
    protocol_parameters_updates: ProposalsTree,
    hard_forks: ProposalsTree,
    constitutional_committee_updates: ProposalsTree,
    constitution_updates: ProposalsTree,
}

impl ProposalsForest {
    pub fn empty() -> Self {
        ProposalsForest {
            proposals: BTreeMap::new(),
            sequence: vec![],
            protocol_parameters_updates: ProposalsTree::Empty,
            hard_forks: ProposalsTree::Empty,
            constitutional_committee_updates: ProposalsTree::Empty,
            constitution_updates: ProposalsTree::Empty,
        }
    }

    pub fn insert(
        &mut self,
        id: ComparableProposalId,
        proposal: GovAction,
    ) -> Result<(), ProposalsInsertError> {
        use amaru_kernel::GovAction::*;

        let id = Rc::new(id);

        self.sequence.push(id.clone());

        match proposal {
            ParameterChange(parent, update, _guardrails_script) => {
                self.protocol_parameters_updates
                    .insert(id.clone(), into_parent_id(parent))?;

                self.proposals
                    .insert(id, ProposalEnum::ProtocolParametersUpdate(*update));

                Ok(())
            }

            HardForkInitiation(parent, protocol_version) => {
                self.hard_forks.insert(id.clone(), into_parent_id(parent))?;

                self.proposals
                    .insert(id, ProposalEnum::HardFork(protocol_version));

                Ok(())
            }

            TreasuryWithdrawals(withdrawals, guardrails_script) => {
                let withdrawals = withdrawals.to_vec().into_iter().fold(
                    BTreeMap::new(),
                    |mut accum, (reward_account, amount)| {
                        accum.insert(expect_stake_credential(&reward_account), amount);
                        accum
                    },
                );

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal {
                        withdrawals,
                        guardrails: Option::from(guardrails_script),
                    }),
                );

                Ok(())
            }

            UpdateCommittee(parent, removed, added, threshold) => {
                self.constitutional_committee_updates
                    .insert(id.clone(), into_parent_id(parent))?;

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::CommitteeUpdate(CommitteeUpdate::ChangeMembers {
                        removed: removed.to_vec().into_iter().collect(),
                        added: added
                            .to_vec()
                            .into_iter()
                            .map(|(k, v)| (k, Epoch::from(v)))
                            .collect(),
                        threshold,
                    }),
                );

                Ok(())
            }

            NoConfidence(parent) => {
                self.constitutional_committee_updates
                    .insert(id.clone(), into_parent_id(parent))?;

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::CommitteeUpdate(CommitteeUpdate::NoConfidence),
                );

                Ok(())
            }

            NewConstitution(parent, constitution) => {
                self.constitution_updates
                    .insert(id.clone(), into_parent_id(parent))?;

                self.proposals
                    .insert(id.clone(), ProposalEnum::Constitution(constitution));

                Ok(())
            }

            Information => {
                self.proposals
                    .insert(id.clone(), ProposalEnum::Orphan(OrphanProposal::NicePoll));

                Ok(())
            }
        }
    }
}

/// Pretty-print a forest. Proposals are shown by groups, and in order within each group. The total
/// ordering is however lost in this representation.
///
/// For example:
///
/// ```no_run
/// Protocol Parameter Updates
/// └─ 0.f6cb185a1f:
///    │ · min_fee_b=42
///    │ · max_block_ex_units={mem=300000, cpu=30000}
///    ├─ 0.27997e2a0b:
///    │    · key_deposit=5000000
///    └─ 0.71762f767d:
///         · key_deposit=1234567
///
/// Hard forks
/// └─ 0.19a065f326: version=10.0
///
/// Others
/// ├─ nice poll
/// ├─ withdrawal=1₳
/// └─ withdrawal=300000₳
/// ```
impl fmt::Display for ProposalsForest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // ---- Renderers -----------------------------------------------------

        // Prints a section header + the tree underneath.
        fn section<'a, A>(
            f: &mut fmt::Formatter<'_>,
            title: &str,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            tree: &'a ProposalsTree,
        ) -> fmt::Result {
            if matches!(tree, ProposalsTree::Empty) {
                return Ok(());
            }
            writeln!(f, "{title}")?;
            render_tree(f, lookup, summarize, tree)?;
            writeln!(f)?;
            Ok(())
        }

        // Renders a whole tree (which is just a bag of siblings at each Node).
        fn render_tree<'a, A>(
            f: &mut fmt::Formatter<'_>,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            tree: &'a ProposalsTree,
        ) -> fmt::Result {
            match tree {
                ProposalsTree::Empty => Ok(()),
                ProposalsTree::Node { siblings, .. } => {
                    for (i, s) in siblings.iter().enumerate() {
                        let is_last = i + 1 == siblings.len();
                        render_sibling(f, s, "", lookup.clone(), summarize.clone(), is_last)?;
                    }
                    Ok(())
                }
            }
        }

        // Render a single sibling + its (flattened) children.
        fn render_sibling<'a, A>(
            f: &mut fmt::Formatter<'_>,
            s: &'a Sibling,
            prefix: &str,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            is_last: bool,
        ) -> fmt::Result {
            let branch = if is_last { "└─" } else { "├─" };

            // Children are a Vec<ProposalsTree<A>>; flatten to a linear list of Sibling<A>
            // to get correct "last" detection for drawing.
            let flat: Vec<&Sibling> = collect_child_siblings(&s.children);
            let next_prefix = if is_last {
                format!("{prefix}   ")
            } else {
                format!("{prefix}│  ")
            };

            writeln!(
                f,
                "{prefix}{branch} {}: {}",
                s.id.to_string().chars().take(12).collect::<String>(),
                match lookup(&s.id) {
                    None => "?".to_string(), // NOTE: should be impossible on a well-formed forest.
                    Some(a) => summarize(
                        a,
                        &if is_last && flat.is_empty() {
                            format!(" {prefix}    ")
                        } else if is_last {
                            format!(" {prefix}  │ ")
                        } else if flat.is_empty() {
                            format!("│{prefix}    ")
                        } else {
                            format!("│{prefix}  │ ")
                        }
                    )?,
                }
            )?;

            for (idx, cs) in flat.iter().enumerate() {
                let last_here = idx + 1 == flat.len();
                render_sibling(
                    f,
                    cs,
                    &next_prefix,
                    lookup.clone(),
                    summarize.clone(),
                    last_here,
                )?;
            }
            Ok(())
        }

        // Gather all siblings from all non-empty child subtrees, in order.
        fn collect_child_siblings(children: &[ProposalsTree]) -> Vec<&Sibling> {
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

        section::<ProtocolParamUpdate>(
            f,
            "Protocol Parameter Updates",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::ProtocolParametersUpdate(a)) => Some(a),
                _ => None,
            }),
            Rc::new(|pp, prefix| {
                Ok(format!(
                    "\n{}",
                    display_protocol_parameters_update(pp, &format!("{prefix}· "))?
                ))
            }),
            &self.protocol_parameters_updates,
        )?;

        section::<ProtocolVersion>(
            f,
            "Hard forks",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::HardFork(a)) => Some(a),
                _ => None,
            }),
            Rc::new(|protocol_version, _| {
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
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::CommitteeUpdate(a)) => Some(a),
                _ => None,
            }),
            Rc::new(|committee_update, _| Ok(committee_update.to_string())),
            &self.constitutional_committee_updates,
        )?;

        section::<Constitution>(
            f,
            "Constitution updates",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::Constitution(a)) => Some(a),
                _ => None,
            }),
            Rc::new(|constitution, _| {
                Ok(format!(
                    "{} with {}",
                    constitution.anchor.url,
                    match constitution.guardrail_script {
                        Nullable::Some(hash) => format!(
                            "guardrails={}",
                            hash.to_string().chars().take(8).collect::<String>()
                        ),
                        Nullable::Undefined | Nullable::Null => "no guardrails".to_string(),
                    },
                ))
            }),
            &self.constitution_updates,
        )?;

        let others = self
            .sequence
            .iter()
            .filter_map(|id| match self.proposals.get(id) {
                Some(ProposalEnum::Orphan(o)) => Some(o),
                _ => None,
            })
            .collect::<Vec<_>>();

        // Others, represented as a flat sequence.
        if !others.is_empty() {
            writeln!(f, "Others")?;
            for (i, o) in others.iter().enumerate() {
                let is_last = i + 1 == others.len();
                let branch = if is_last { "└─" } else { "├─" };
                writeln!(f, "{branch} {o}")?;
            }
        }

        Ok(())
    }
}

// Helpers
// ----------------------------------------------------------------------------

fn into_parent_id(nullable: Nullable<ProposalId>) -> Option<Rc<ComparableProposalId>> {
    match nullable {
        Nullable::Undefined | Nullable::Null => None,
        Nullable::Some(id) => Some(Rc::new(ComparableProposalId::from(id))),
    }
}
