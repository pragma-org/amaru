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
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    rc::Rc,
};

// Top-level logic
// ----------------------------------------------------------------------------

/// All informations needed to ratify votes.
pub struct RatificationContext {
    /// The epoch that just ended.
    pub epoch: Epoch,

    /// The protocol version at the moment the epoch ended.
    pub protocol_version: ProtocolVersion,

    /// The current roots (i.e. latest enacted proposal ids) for each of the
    /// relevant proposal categories.
    pub roots: ProposalRoots,
}

pub fn ratify_proposals(
    ctx: RatificationContext,
    mut proposals: Vec<(ProposalId, proposals::Row)>,
) {
    proposals.sort_by(|a, b| a.1.proposed_in.cmp(&b.1.proposed_in));

    let mut forest = proposals
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

    // The ratification of some proposals causes all other subsequent proposals' ratification to be
    // delayed to the next epoch boundary. Initially, there's none and we'll switch the flag if any
    // of the following proposal kind gets ratified:
    //
    // - a motion of no confidence; or
    // - a hard fork; or
    // - a constitutional committee update; or
    // - a constitution update.
    //
    // Said differently, there can be many treasury withdrawals, protocol parameters changes or
    // nice polls; but as soon as one of the other is encountered; EVERYTHING (including treasury
    // withdrawals and parameters changes) is postponed until the next epoch.
    let mut delayed = false;

    while let Some((id, proposal)) = guard(!delayed, || forest.peek()) {
        // TODO: There are additional checks we should perform at the moment of ratification
        //
        // - On constitutional committee updates, we should ensure that any term limit is still
        //   valid. This can happen if a protocol parameter change that changes the max term limit
        //   is ratified *before* a committee update, possibly rendering it invalid.
        //
        // - On treasury withdrawals, we must ensure there's still enough money in the treasury.
        //   This is necessary since there can be an arbitrary number of withdrawals that have been
        //   ratified and enacted just before; possibly depleting the treasury.
        //
        // Note that either way, it doesn't _invalidate_ proposals, since time and subsequent
        // proposals may turn the tide again. They should simply be skipped, and revisited at the
        // next epoch boundary.

        // Ensures that the next proposal points to an active root. Not being the case isn't
        // necessarily an issue or a sign that something went wrong.
        //
        // In fact, since proposals can form arbitrarily long chain, it is very plausible that a
        // proposal points to another that isn't ratified just yet.
        //
        // Encountering a non-matching root also doesn't mean we shouldn't process other proposals.
        // The order is given by their point of submission; and thus, proposals submitted later may
        // points to totally different (and active) roots.
        if roots.matching(&proposal) && is_accepted_by_everyone(&ctx, &proposal) {
            todo!("a proposal has been ratified, it must now be enacted!")
        }
    }
}

fn is_accepted_by_everyone(ctx: &RatificationContext, proposal: &ProposalEnum) -> bool {
    is_accepted_by_constitutional_committee(ctx, proposal)
        && is_accepted_by_stake_pool_operators(ctx, proposal)
        && is_accepted_by_delegate_representatives(ctx, proposal)
}

fn is_accepted_by_constitutional_committee(
    ctx: &RatificationContext,
    proposal: &ProposalEnum,
) -> bool {
    todo!()
}

fn is_accepted_by_stake_pool_operators(ctx: &RatificationContext, proposal: &ProposalEnum) -> bool {
    todo!()
}

fn is_accepted_by_delegate_representatives(
    ctx: &RatificationContext,
    proposal: &ProposalEnum,
) -> bool {
    todo!()
}

// ProposalRoots
// ----------------------------------------------------------------------------

#[derive(Debug)]
pub struct ProposalRoots {
    pub protocol_parameters: Option<ComparableProposalId>,
    pub hard_fork: Option<ComparableProposalId>,
    pub constitutional_committee: Option<ComparableProposalId>,
    pub constitution: Option<ComparableProposalId>,
}

impl ProposalRoots {
    pub fn matching(&self, proposal: &ProposalEnum) -> bool {
        match proposal {
            // Orphans have no parents, so no roots. Hence it always _matches_.
            ProposalEnum::Orphan(..) => true,
            ProposalEnum::ProtocolParameters(_, parent) => {
                parent.as_deref() == self.protocol_parameters.as_ref()
            }
            ProposalEnum::HardFork(_, parent) => parent.as_deref() == self.hard_fork.as_ref(),
            ProposalEnum::ConstitutionalCommittee(_, parent) => {
                parent.as_deref() == self.constitutional_committee.as_ref()
            }
            ProposalEnum::Constitution(_, parent) => {
                parent.as_deref() == self.constitution.as_ref()
            }
        }
    }
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
    ProtocolParameters(ProtocolParamUpdate, Option<Rc<ComparableProposalId>>),
    HardFork(ProtocolVersion, Option<Rc<ComparableProposalId>>),
    ConstitutionalCommittee(CommitteeUpdate, Option<Rc<ComparableProposalId>>),
    Constitution(Constitution, Option<Rc<ComparableProposalId>>),
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
    sequence: VecDeque<Rc<ComparableProposalId>>,

    // Finally, the relation between proposals is preserved through multiple tree-like structures.
    // This is what gives this data-structure its name.
    protocol_parameters: ProposalsTree,
    hard_fork: ProposalsTree,
    constitutional_committee: ProposalsTree,
    constitution: ProposalsTree,
}

impl ProposalsForest {
    pub fn empty() -> Self {
        ProposalsForest {
            proposals: BTreeMap::new(),
            sequence: VecDeque::new(),
            protocol_parameters: ProposalsTree::Empty,
            hard_fork: ProposalsTree::Empty,
            constitutional_committee: ProposalsTree::Empty,
            constitution: ProposalsTree::Empty,
        }
    }

    /// Returns the proposal that should be handled next, if any. The order is given by the
    /// insertion order of proposals. Although, other operations on the forest may results in
    /// pruning the forest by removing intermediate proposals.
    pub fn peek(&self) -> Option<(&ComparableProposalId, &ProposalEnum)> {
        let k = self.sequence.front()?;
        self.proposals.get(k).map(|v| (k.as_ref(), v))
    }

    /// Insert a proposal in the forest. This retains the order of insertion, so it is assumed
    /// that:
    ///
    /// 1. The caller has taken care of ordering proposals so that when a proposal has a parent
    ///    relationship with another, that other has been inserted before.
    ///
    /// 2. Except from the first proposal at the root of the tree, there's no proposal referring to
    ///    a non-existing parent (which is vaguely similar to the first point).
    ///
    /// If these two conditions are respected, then `insert` cannot fail and will always yield
    /// `Ok`.
    pub fn insert(
        &mut self,
        id: ComparableProposalId,
        proposal: GovAction,
    ) -> Result<(), ProposalsInsertError> {
        use amaru_kernel::GovAction::*;

        let id = Rc::new(id);

        self.sequence.push_back(id.clone());

        match proposal {
            ParameterChange(parent, update, _guardrails_script) => {
                let parent = into_parent_id(parent);

                self.protocol_parameters
                    .insert(id.clone(), parent.clone())?;

                self.proposals
                    .insert(id, ProposalEnum::ProtocolParameters(*update, parent));

                Ok(())
            }

            HardForkInitiation(parent, protocol_version) => {
                let parent = into_parent_id(parent);

                self.hard_fork.insert(id.clone(), parent.clone())?;

                self.proposals
                    .insert(id, ProposalEnum::HardFork(protocol_version, parent));

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
                let parent = into_parent_id(parent);

                self.constitutional_committee
                    .insert(id.clone(), parent.clone())?;

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::ConstitutionalCommittee(
                        CommitteeUpdate::ChangeMembers {
                            removed: removed.to_vec().into_iter().collect(),
                            added: added
                                .to_vec()
                                .into_iter()
                                .map(|(k, v)| (k, Epoch::from(v)))
                                .collect(),
                            threshold,
                        },
                        parent,
                    ),
                );

                Ok(())
            }

            NoConfidence(parent) => {
                let parent = into_parent_id(parent);

                self.constitutional_committee
                    .insert(id.clone(), parent.clone())?;

                self.proposals.insert(
                    id.clone(),
                    ProposalEnum::CommitteeUpdate(CommitteeUpdate::NoConfidence, parent),
                );

                Ok(())
            }

            NewConstitution(parent, constitution) => {
                let parent = into_parent_id(parent);

                self.constitution.insert(id.clone(), parent.clone())?;

                self.proposals
                    .insert(id.clone(), ProposalEnum::Constitution(constitution, parent));

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
                Some(ProposalEnum::ProtocolParametersUpdate(a, _)) => Some(a),
                _ => None,
            }),
            Rc::new(|pp, prefix| {
                Ok(format!(
                    "\n{}",
                    display_protocol_parameters_update(pp, &format!("{prefix}· "))?
                ))
            }),
            &self.protocol_parameters,
        )?;

        section::<ProtocolVersion>(
            f,
            "Hard forks",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::HardFork(a, _)) => Some(a),
                _ => None,
            }),
            Rc::new(|protocol_version, _| {
                Ok(format!(
                    "version={}.{}",
                    protocol_version.0, protocol_version.1
                ))
            }),
            &self.hard_fork,
        )?;

        section::<CommitteeUpdate>(
            f,
            "Constitutional Committee Updates",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::CommitteeUpdate(a, _)) => Some(a),
                _ => None,
            }),
            Rc::new(|committee_update, _| Ok(committee_update.to_string())),
            &self.constitutional_committee,
        )?;

        section::<Constitution>(
            f,
            "Constitution updates",
            Rc::new(|id| match self.proposals.get(id) {
                Some(ProposalEnum::Constitution(a, _)) => Some(a),
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
            &self.constitution,
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

/// Execute the guarded action if the predicate is `true`; returns `None` otherwise.
fn guard<A>(predicate: bool, action: impl Fn() -> Option<A>) -> Option<A> {
    if predicate {
        action()
    } else {
        None
    }
}
