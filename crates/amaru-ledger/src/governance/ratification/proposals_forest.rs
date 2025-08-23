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

use super::{
    proposals_roots::ProposalsRootsRc,
    proposals_tree::{ProposalsTree, Sibling},
    CommitteeUpdate, OrphanProposal, ProposalEnum,
};
use crate::{store::columns::proposals, summary::into_safe_ratio};
use amaru_kernel::{
    display_protocol_parameters_update, expect_stake_credential, ComparableProposalId,
    Constitution, Epoch, EraHistory, GovAction, Nullable, ProposalId, ProposalPointer,
    ProtocolParamUpdate, ProtocolVersion,
};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    rc::Rc,
};

pub use super::proposals_tree::{ProposalsEnactError, ProposalsInsertError};

#[derive(Debug, Clone)]
pub struct ProposalsForest {
    /// We keep a map of id -> ProposalEnum. This serves as a lookup table to retrieve proposals
    /// from the forest in a timely manner while the relationships between all proposals is
    /// maintained independently.
    proposals: BTreeMap<Rc<ComparableProposalId>, ProposedIn<ProposalEnum>>,

    /// The order in which proposals are inserted matters. The forest is an insertion-preserving
    /// structure. Iterating on the forest will yield the proposals in the order they were
    /// inserted.
    sequence: VecDeque<Rc<ComparableProposalId>>,

    /// A flag indicating whether the ratification is now interrupted due to a
    /// high-priority/high-impact proposal (i.e. hard-fork, constitutional committee or
    /// constitution) having been ratified.
    is_interrupted: bool,

    /// The current epoch, a.k.a minimal epoch proposals must have been submitted after to be
    /// considered for ratification. This allows skipping the ratification of *just* submitted
    /// proposals, since it should happens with an epoch of delay.
    current_epoch: Epoch,

    // Finally, the relation between proposals of the same nature is preserved through multiple
    // tree-like structures. This is proposal gives this data-structure its name.
    protocol_parameters: ProposalsTree<ComparableProposalId>,
    hard_fork: ProposalsTree<ComparableProposalId>,
    constitutional_committee: ProposalsTree<ComparableProposalId>,
    constitution: ProposalsTree<ComparableProposalId>,
}

impl ProposalsForest {
    pub fn new(current_epoch: Epoch, roots: &ProposalsRootsRc) -> Self {
        ProposalsForest {
            current_epoch,
            is_interrupted: false,

            proposals: BTreeMap::new(),
            sequence: VecDeque::new(),

            // NOTE: clones are cheap, roots are `Rc`.
            protocol_parameters: ProposalsTree::new(roots.protocol_parameters.clone()),
            hard_fork: ProposalsTree::new(roots.hard_fork.clone()),
            constitutional_committee: ProposalsTree::new(roots.constitutional_committee.clone()),
            constitution: ProposalsTree::new(roots.constitution.clone()),
        }
    }

    /// Returns an iterator over the forest's proposal.
    pub fn new_compass(&self) -> ProposalsForestCompass {
        ProposalsForestCompass::new(self)
    }

    /// Insert many proposals at once, consuming them.
    ///
    /// Pre-condition: all proposals MUST HAVE NOT expire (i.e. be valid until the current epoch).
    pub fn drain(
        mut self,
        era_history: &'_ EraHistory,
        mut proposals: Vec<(ComparableProposalId, proposals::Row)>,
    ) -> Result<Self, ProposalsInsertError<ComparableProposalId>> {
        let current_epoch = self.current_epoch;

        proposals
            .drain(..)
            .try_fold::<_, _, Result<_, ProposalsInsertError<_>>>(
                &mut self,
                |forest, (id, row)| {
                    // There shouldn't be any invalid proposals left at this point.
                    assert!(
                        row.valid_until + 1 >= current_epoch,
                        "proposal {id:?} is expired (ratification epoch = {current_epoch}) but was \
                        drained into the forest: {row:?}",
                    );

                    forest.insert(era_history, id, row.proposed_in, row.proposal.gov_action)?;

                    Ok(forest)
                },
            )?;

        Ok(self)
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
        era_history: &'_ EraHistory,
        id: ComparableProposalId,
        proposed_in: ProposalPointer,
        proposal: GovAction,
    ) -> Result<(), ProposalsInsertError<ComparableProposalId>> {
        use amaru_kernel::GovAction::*;

        let id = Rc::new(id);

        let mut insert = |proposal| -> Result<(), ProposalsInsertError<ComparableProposalId>> {
            priority_insert(
                &mut self.sequence,
                id.clone(),
                (&proposed_in, &proposal),
                &self.proposals,
            );

            let slot = proposed_in.slot();

            let epoch = era_history
                .slot_to_epoch(slot, slot)
                .map_err(|e| ProposalsInsertError::InternalSlotToEpochError(slot, e))?;

            self.proposals.insert(
                id.clone(),
                ProposedIn {
                    epoch,
                    pointer: proposed_in,
                    proposal,
                },
            );

            Ok(())
        };

        match proposal {
            ParameterChange(parent, update, _guardrails_script) => {
                let parent = into_parent_id(parent);

                self.protocol_parameters
                    .insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::ProtocolParameters(*update, parent))
            }

            HardForkInitiation(parent, protocol_version) => {
                let parent = into_parent_id(parent);

                self.hard_fork.insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::HardFork(protocol_version, parent))
            }

            TreasuryWithdrawals(withdrawals, _guardrails_script) => {
                let withdrawals = withdrawals.to_vec().into_iter().fold(
                    BTreeMap::new(),
                    |mut accum, (reward_account, amount)| {
                        accum.insert(expect_stake_credential(&reward_account), amount);
                        accum
                    },
                );

                insert(ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal(
                    withdrawals,
                )))
            }

            UpdateCommittee(parent, removed, added, threshold) => {
                let parent = into_parent_id(parent);

                self.constitutional_committee
                    .insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::ConstitutionalCommittee(
                    CommitteeUpdate::ChangeMembers {
                        removed: removed.to_vec().into_iter().collect(),
                        added: added
                            .to_vec()
                            .into_iter()
                            .map(|(k, v)| (k, Epoch::from(v)))
                            .collect(),
                        threshold: into_safe_ratio(&threshold),
                    },
                    parent,
                ))
            }

            NoConfidence(parent) => {
                let parent = into_parent_id(parent);

                self.constitutional_committee
                    .insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::ConstitutionalCommittee(
                    CommitteeUpdate::NoConfidence,
                    parent,
                ))
            }

            NewConstitution(parent, constitution) => {
                let parent = into_parent_id(parent);

                self.constitution.insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::Constitution(constitution, parent))
            }

            Information => insert(ProposalEnum::Orphan(OrphanProposal::NicePoll)),
        }
    }

    /// Get the current roots of the forest.
    pub fn roots(&self) -> ProposalsRootsRc {
        // NOTE: clone are cheap here, because everything is an `Rc`.
        ProposalsRootsRc {
            protocol_parameters: self.protocol_parameters.root(),
            hard_fork: self.hard_fork.root(),
            constitutional_committee: self.constitutional_committee.root(),
            constitution: self.constitution.root(),
        }
    }

    /// Enact a proposal in the forest. Which means:
    ///
    /// 1. Promote it as new root in its appropriate sub-tree, and prune all its siblings and their
    ///    children. Those proposals are now unreachable / unvotable, and will be cleared from the
    ///    state.
    ///
    ///    Note that, orphan proposals are simply cleared from the orphan list.
    ///
    /// 2. Remove the enacted proposal and any of the pruned proposal from the `proposals` lookup
    ///    table.
    ///
    /// 3. Amend the `sequence` accordingly as well.
    ///
    /// 4. And finally, we must remember whether that enacted proposal is:
    ///
    ///     - a `HardFork`; or
    ///     - a `ConstitutionalCommittee`; or
    ///     - a `Constitution`
    ///
    ///    No other proposal can be ratified in the same epoch boundary.
    pub fn enact(
        &mut self,
        id: Rc<ComparableProposalId>,
        proposal: &ProposalEnum,
        compass: &mut ProposalsForestCompass,
    ) -> Result<BTreeSet<Rc<ComparableProposalId>>, ProposalsEnactError<ComparableProposalId>> {
        // Promote to new root & remember delaying cases
        let (id, mut pruned) = match proposal {
            ProposalEnum::HardFork(..) => {
                self.is_interrupted = true;
                self.hard_fork.enact(id)
            }
            ProposalEnum::ConstitutionalCommittee(..) => {
                self.is_interrupted = true;
                self.constitutional_committee.enact(id)
            }
            ProposalEnum::Constitution(..) => {
                self.is_interrupted = true;
                self.constitution.enact(id)
            }
            ProposalEnum::ProtocolParameters(..) => self.protocol_parameters.enact(id),
            ProposalEnum::Orphan(..) =>
            {
                #[allow(clippy::map_entry)]
                if self.proposals.contains_key(&id) {
                    Ok((id, BTreeSet::new()))
                } else {
                    Err(ProposalsEnactError::UnknownProposal { id })
                }
            }
        }?;

        pruned.insert(id);

        // Clean up the lookup table.
        self.proposals
            .retain(|pid, _| !pruned.contains(pid.as_ref()));

        // Clean up sequence, while preserving its order.
        self.sequence.retain(|sid| !pruned.contains(sid.as_ref()));

        // Force replacement of the compass; since any previous one is now obsolete.
        *compass = self.new_compass();

        Ok(pruned)
    }

    /// Check whether a given proposal's parent matches the current forest root. Orphans proposals
    /// have no parents, hence they are always considering matching.
    fn matching_root(&self, proposal: &ProposalEnum) -> bool {
        match proposal {
            ProposalEnum::Orphan(..) => true,

            ProposalEnum::ProtocolParameters(_, parent) => {
                parent.as_deref() == self.protocol_parameters.as_root()
            }

            ProposalEnum::HardFork(_, parent) => parent.as_deref() == self.hard_fork.as_root(),

            ProposalEnum::ConstitutionalCommittee(_, parent) => {
                parent.as_deref() == self.constitutional_committee.as_root()
            }

            ProposalEnum::Constitution(_, parent) => {
                parent.as_deref() == self.constitution.as_root()
            }
        }
    }
}

/// A mutable cursor to navigate the forest. This allows to iterate over the forest elements
/// without holding a mutable reference on the forest. The mutation is being seggregated in the
/// compass.
///
/// This enables a consumer to walk the forest, and perform short-lived mutations on it (prune
/// trees by enacting proposals). Following any mutation, a new compass needs to be acquired.
/// Re-using an old compass will create a panic.
#[derive(Debug)]
pub struct ProposalsForestCompass {
    cursor: usize,
    original_len: usize,
}

impl ProposalsForestCompass {
    pub fn new(forest: &ProposalsForest) -> Self {
        Self {
            cursor: 0,
            original_len: forest.sequence.len(),
        }
    }

    /// Get the next proposal in line for ratification. This relies on a few invariant from the
    /// ProposalsForest, such that:
    ///
    /// - the `sequence` ultimately defines the order
    /// - any id present in the sequence also exists in the `proposals` lookup table.
    /// - a cursor isn't reused following an enactment.
    pub fn next<'forest>(
        &mut self,
        forest: &'forest ProposalsForest,
    ) -> Option<(
        Rc<ComparableProposalId>,
        (&'forest ProposalEnum, &'forest ProposalPointer),
    )> {
        assert!(
            forest.sequence.len() == self.original_len,
            "compass re-used on a forest that has changed; you should have created a new compass."
        );

        // == TL; DR;
        //   A high-priority/high-impact proposal has already been enacted; so we prevent the
        //   ratification of any new proposal from then on.
        //
        // == Longer explanation:
        //   The ratification of some proposals causes all other subsequent proposals' ratification
        //   to be delayed to the next epoch boundary. Initially, there's none and we'll switch the
        //   flag if any of the following proposal kind gets ratified:
        //
        //   - a motion of no confidence; or
        //   - a hard fork; or
        //   - a constitutional committee update; or
        //   - a constitution update.
        //
        //   Said differently, there can be many treasury withdrawals, protocol parameters changes
        //   or nice polls; but as soon as one of the other is encountered; EVERYTHING (including
        //   treasury withdrawals and parameters changes) is postponed until the next epoch.
        if forest.is_interrupted {
            return None;
        }

        loop {
            let result: Option<(
                Rc<ComparableProposalId>,
                (&'forest ProposalEnum, &'forest ProposalPointer),
            )> = {
                let id = forest.sequence.get(self.cursor)?.clone();

                let ProposedIn {
                    epoch: proposed_in,
                    proposal,
                    pointer,
                } = forest.proposals.get(&id).unwrap_or_else(|| {
                    unreachable!("forest's sequence knows of the id {id:?} but it wasn't found in the lookup-table");
                });

                self.cursor += 1;

                // Proposals are ratified with an epoch of delay. So
                //
                // - if a proposal is submitted in epoch e, it musn't be ratified in the
                // transition from e -> e + 1, but from the transition from e + 1 -> e + 2.
                //
                // - Yet, the forest will always include ALL proposals, since we must potentially
                // prune recent proposals due to the enactment of older proposals.
                //
                // - `forest.current_epoch` contains the minimum epoch for which we might consider
                // for ratification. If a proposal was submitted in the epoch that just ended, we
                // skip it.
                if proposed_in >= &forest.current_epoch {
                    return None;
                }

                // Ensures that the next proposal points to an active root. Not being the case isn't
                // necessarily an issue or a sign that something went wrong.
                //
                // In fact, since proposals can form arbitrarily long chain, it is very plausible that a
                // proposal points to another that isn't ratified just yet.
                //
                // Encountering a non-matching root also doesn't mean we shouldn't process other proposals.
                // The order is given by their point of submission; and thus, proposals submitted later may
                // points to totally different (and active) roots. So we just skip those proposals.
                if forest.matching_root(proposal) {
                    Some((id, (proposal, pointer)))
                } else {
                    None
                }
            };

            if result.is_some() || self.cursor >= self.original_len {
                return result;
            }
        }
    }
}

/// Pretty-print a forest. Proposals are shown by groups, and in order *within each group*. The
/// total ordering is however lost in this representation.
///
/// For example:
///
/// ```ignore
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
            tree: &'a ProposalsTree<ComparableProposalId>,
        ) -> fmt::Result {
            if tree.is_empty() {
                return Ok(());
            }
            writeln!(f, "{title}")?;
            render_tree(f, lookup, summarize, tree)?;
            Ok(())
        }

        // Renders a whole tree (which is just a bag of siblings at each Node).
        fn render_tree<'a, A>(
            f: &mut fmt::Formatter<'_>,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            tree: &'a ProposalsTree<ComparableProposalId>,
        ) -> fmt::Result {
            let siblings = tree.siblings();
            for (i, s) in siblings.iter().enumerate() {
                let is_last = i + 1 == siblings.len();
                render_sibling(f, s, "", lookup.clone(), summarize.clone(), is_last)?;
            }
            Ok(())
        }

        // Render a single sibling + its (flattened) children.
        fn render_sibling<'a, A>(
            f: &mut fmt::Formatter<'_>,
            s: &'a Sibling<ComparableProposalId>,
            prefix: &str,
            lookup: Rc<dyn Fn(&'_ ComparableProposalId) -> Option<&'a A> + 'a>,
            summarize: Rc<dyn Fn(&A, &str) -> Result<String, fmt::Error>>,
            is_last: bool,
        ) -> fmt::Result {
            let branch = if is_last { "└─" } else { "├─" };

            // Children are a Vec<ProposalsTree<A>>; flatten to a linear list of Sibling<A>
            // to get correct "last" detection for drawing.
            let children = s.children();
            let next_prefix = if is_last {
                format!("{prefix}   ")
            } else {
                format!("{prefix}│  ")
            };

            writeln!(
                f,
                "{prefix}{branch} {}: {}",
                s.as_id().to_compact_string(),
                match lookup(s.as_id()) {
                    None => "?".to_string(), // NOTE: should be impossible on a well-formed forest.
                    Some(a) => summarize(
                        a,
                        &if is_last && children.is_empty() {
                            format!(" {prefix}    ")
                        } else if is_last {
                            format!(" {prefix}  │ ")
                        } else if children.is_empty() {
                            format!("│{prefix}    ")
                        } else {
                            format!("│{prefix}  │ ")
                        }
                    )?,
                }
            )?;

            for (idx, cs) in children.iter().enumerate() {
                let last_here = idx + 1 == children.len();
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

        // ---- Forest printing -----------------------------------------------

        if !self.sequence.is_empty() {
            writeln!(f)?;
        } else {
            write!(f, "empty")?;
        }

        section::<CommitteeUpdate>(
            f,
            "Constitutional Committee Updates",
            Rc::new(|id| {
                if let ProposalEnum::ConstitutionalCommittee(a, _) =
                    &self.proposals.get(id)?.proposal
                {
                    Some(a)
                } else {
                    None
                }
            }),
            Rc::new(|committee_update, _| Ok(committee_update.to_string())),
            &self.constitutional_committee,
        )?;

        section::<Constitution>(
            f,
            "Constitution updates",
            Rc::new(|id| {
                if let ProposalEnum::Constitution(a, _) = &self.proposals.get(id)?.proposal {
                    Some(a)
                } else {
                    None
                }
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

        section::<ProtocolVersion>(
            f,
            "Hard forks",
            Rc::new(|id| {
                if let ProposalEnum::HardFork(a, _) = &self.proposals.get(id)?.proposal {
                    Some(a)
                } else {
                    None
                }
            }),
            Rc::new(|protocol_version, _| {
                Ok(format!(
                    "version={}.{}",
                    protocol_version.0, protocol_version.1
                ))
            }),
            &self.hard_fork,
        )?;

        section::<ProtocolParamUpdate>(
            f,
            "Protocol Parameter Updates",
            Rc::new(|id| {
                if let ProposalEnum::ProtocolParameters(a, _) = &self.proposals.get(id)?.proposal {
                    Some(a)
                } else {
                    None
                }
            }),
            Rc::new(|pp, prefix| {
                Ok(format!(
                    "\n{}",
                    display_protocol_parameters_update(pp, &format!("{prefix}· "))?
                ))
            }),
            &self.protocol_parameters,
        )?;

        let others = self
            .sequence
            .iter()
            .filter_map(|id| {
                if let ProposalEnum::Orphan(o) = &self.proposals.get(id).as_ref()?.proposal {
                    Some((id, o))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        // Others, represented as a flat sequence.
        if !others.is_empty() {
            writeln!(f, "Others")?;
            for (i, (id, other)) in others.iter().enumerate() {
                let is_last = i + 1 == others.len();
                let branch = if is_last { "└─" } else { "├─" };
                writeln!(f, "{branch} {}: {other}", id.to_compact_string())?;
            }
        }

        Ok(())
    }
}

// Helpers
// ----------------------------------------------------------------------------

/// A type akin to a (Epoch, T), but with field name for readability.
#[derive(Debug, Clone)]
pub struct ProposedIn<T> {
    pub epoch: Epoch,
    pub pointer: ProposalPointer,
    pub proposal: T,
}

fn into_parent_id(nullable: Nullable<ProposalId>) -> Option<Rc<ComparableProposalId>> {
    match nullable {
        Nullable::Undefined | Nullable::Null => None,
        Nullable::Some(id) => Some(Rc::new(ComparableProposalId::from(id))),
    }
}

/// Insert a proposal in a sequence while maintaining a priority order. The priority is given by
/// the type of proposal.
fn priority_insert(
    seq: &mut VecDeque<Rc<ComparableProposalId>>,
    new_id: Rc<ComparableProposalId>,
    (new_pointer, new_proposal): (&ProposalPointer, &ProposalEnum),
    proposals: &BTreeMap<Rc<ComparableProposalId>, ProposedIn<ProposalEnum>>,
) {
    let mut insertion_ix = 0;

    for id in seq.iter() {
        if let Some(ProposedIn {
            proposal, pointer, ..
        }) = proposals.get(id)
        {
            match proposal.cmp_priority(new_proposal) {
                // Next proposal has a lower priority;
                // -> we must insert before it.
                Ordering::Less => {
                    break;
                }

                // Next proposal has equal priority, but was submitted after us;
                // -> we must insert before it
                Ordering::Equal if pointer.cmp(new_pointer) == Ordering::Greater => {
                    break;
                }

                Ordering::Greater | Ordering::Equal => (),
            }

            insertion_ix += 1;
        } else {
            unreachable!(
                "invariant violation: \
                proposal {id:?} in sequence {seq:?} but not in lookup-table {proposals:?}"
            );
        }
    }

    seq.insert(insertion_ix, new_id);
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(all(test, not(target_os = "windows")))]
mod tests {
    use super::ProposalsForest;
    use crate::governance::ratification::{
        tests::{
            any_committee_update, any_proposal_enum, ERA_HISTORY, MAX_ARBITRARY_EPOCH,
            MIN_ARBITRARY_EPOCH,
        },
        CommitteeUpdate, ProposalEnum, ProposalsRootsRc,
    };
    use amaru_kernel::{
        tests::{
            any_comparable_proposal_id, any_constitution, any_gov_action, any_proposal_pointer,
            any_protocol_params_update, any_protocol_version,
        },
        ComparableProposalId, Epoch, GovAction, KeyValuePairs, Nullable, ProposalId,
        ProposalPointer, RationalNumber, Set,
    };
    use proptest::{collection, prelude::*};
    use std::{cmp::Ordering, collections::BTreeSet, rc::Rc};

    const MAX_TREE_SIZE: usize = 8;

    fn check_invariants(forest: &ProposalsForest) -> usize {
        let size = forest.sequence.len();

        assert_eq!(
            size,
            forest.proposals.len(),
            "invariant violation: len(sequence) != size(proposals)"
        );

        assert_eq!(
            size,
            forest.sequence.iter().collect::<BTreeSet<_>>().len(),
            "invariant violation: sequence contains duplicates"
        );

        for id in forest.sequence.iter() {
            assert!(
                forest.proposals.contains_key(id),
                "invariant violation: {id} in sequence but not in proposals"
            );
        }

        size
    }

    proptest! {
        #[test]
        fn prop_insert_increase_sizes_by_one(
            DebugAsDisplay(mut forest) in any_proposals_forest(),
            id in any_comparable_proposal_id(),
            mut action in any_gov_action(),
            pointer in any_proposal_pointer(u64::MAX),
            parent in any::<u8>()
        ) {
            let size_before = check_invariants(&forest);

            let parents = possible_parents(&forest, &action);
            if !parents.is_empty() {
                action = set_parent(action, select(&parents, parent));
            }

            forest.insert(&ERA_HISTORY, id, pointer, action).unwrap();

            let size_after = check_invariants(&forest);

            prop_assert_eq!(size_before + 1, size_after);
        }
    }

    proptest! {
        #[test]
        fn prop_compass_traverse_whole_forest_and_eventually_yield_none(
            DebugAsDisplay(forest) in any_proposals_forest(),
        ) {
            use super::ProposalEnum::*;

            let mut previous_proposal = None;
            let mut compass = forest.new_compass();

            while let Some((id, (proposal, pointer))) = compass.next(&forest) {
                // Controls that the yielded proposal always has a matching root.
                let roots = forest.roots();
                match proposal {
                    ProtocolParameters(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.protocol_parameters.as_ref(),
                        "yielded proposal has a different root than latest enacted one",
                    ),
                    HardFork(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.hard_fork.as_ref(),
                        "yielded proposal has a different root than latest enacted one",
                    ),
                    ConstitutionalCommittee(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.constitutional_committee.as_ref(),
                        "yielded proposal has a different root than latest enacted one",
                    ),
                    Constitution(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.constitution.as_ref(),
                        "yielded proposal has a different root than latest enacted one",
                    ),
                    Orphan(..) => ()
                }

                prop_assert!(
                    ERA_HISTORY.slot_to_epoch(pointer.slot(), pointer.slot()).unwrap() < forest.current_epoch,
                    "yielded proposal too early for ratification",
                );

                if let Some((previous_proposal, previous_pointer)) = previous_proposal {
                    // Controls that any yielded proposal comes with a lower priority than any
                    // previously yielded one.
                    let relative_priority = proposal.cmp_priority(previous_proposal);
                    prop_assert!(
                        relative_priority == Ordering::Equal || relative_priority == Ordering::Less,
                        "yielded proposal has a higher priority than the previously yielded proposal",
                    );

                    // Controls that any yielded proposal comes strictly after any previously yielded
                    // one; unless the previous one has higher priority.
                    if relative_priority == Ordering::Equal {
                        prop_assert!(
                            pointer.cmp(previous_pointer) == Ordering::Greater,
                            "yielded proposal {id} at {pointer:?} was submitted before previous one at {previous_pointer:?}",
                        );
                    }
                }

                previous_proposal = Some((proposal, pointer));
            }
        }
    }

    proptest! {
        #[test]
        fn prop_cannot_enact_unknown_proposal(
            DebugAsDisplay(mut forest) in any_proposals_forest(),
            proposal_id in any_comparable_proposal_id(),
            proposal in any_proposal_enum(),
        ) {
            let mut compass = forest.new_compass();
            prop_assert!(forest.enact(Rc::new(proposal_id), &proposal, &mut compass).is_err());
        }
    }

    proptest! {
        #[test]
        #[should_panic]
        fn prop_cannot_insert_root(
            (DebugAsDisplay(mut forest), root) in any_grown_proposals_forest(),
            action in any_gov_action(),
            proposed_in in any_proposal_pointer(u64::MAX),
        ) {
            let _ = forest.insert(&ERA_HISTORY, root, proposed_in, action);
        }
    }

    proptest! {
        #[test]
        fn prop_can_enact_any_of_the_yielded_proposal(
            DebugAsDisplay(forest) in any_non_empty_proposals_forest()
        ) {
            let mut compass = forest.new_compass();
            let mut to_enact = Vec::new();
            while let Some((id, (proposal, _))) = compass.next(&forest) {
                to_enact.push((id, proposal.clone()));
            }

            for (id, proposal) in to_enact.into_iter() {
                // We're going to mutate the forest, so we need a clone to test this across all
                // proposals.
                let mut forest = forest.clone();

                let mut compass = forest.new_compass();

                let pruned = forest.enact(id.clone(), &proposal, &mut compass).unwrap();

                // Control that
                // (1) the compass still work;
                // (2) we never yield a pruned proposal;
                // (3) no proposal are yielded after enacting a high-impact one;
                let mut yielded_another = false;
                while let Some((id, _)) = compass.next(&forest) {
                    yielded_another = true;
                    prop_assert!(
                        !pruned.contains(&id),
                        "compass yielded a pruned proposal!",
                    );
                }

                let is_high_impact = matches!(
                    &proposal,
                    ProposalEnum::HardFork(..) |
                    ProposalEnum::Constitution(..) |
                    ProposalEnum::ConstitutionalCommittee(..),
                );

                prop_assert!(
                    !(is_high_impact && yielded_another),
                    "yielded another proposal after enacting a high-impact one"
                );

                // Control that the enacted proposal is one of the new root.
                prop_assert!(
                    forest.roots().protocol_parameters.as_deref() == Some(id.as_ref()) ||
                    forest.roots().hard_fork.as_deref() == Some(id.as_ref()) ||
                    forest.roots().constitutional_committee.as_deref() == Some(id.as_ref()) ||
                    forest.roots().constitution.as_deref() == Some(id.as_ref()),
                    "none of the roots match the just enacted proposal",
                );
            }
        }
    }

    // Generate an arbitrary forest that yields at least one proposal. Note that forest that holds
    // proposals but don't yield them won't be generated by this.
    fn any_non_empty_proposals_forest() -> impl Strategy<Value = DebugAsDisplay<ProposalsForest>> {
        any_proposals_forest().prop_filter("forest is not empty", |DebugAsDisplay(forest)| {
            forest.new_compass().next(forest).is_some()
        })
    }

    // Generate an arbitrary forest, that has at least one Some(..) root.
    fn any_grown_proposals_forest(
    ) -> impl Strategy<Value = (DebugAsDisplay<ProposalsForest>, ComparableProposalId)> {
        (any::<u8>(), any_proposals_forest()).prop_filter_map(
            "forest is immaculate",
            |(ix, DebugAsDisplay(forest))| {
                let roots = forest.roots();

                let non_empty_roots: Vec<Rc<ComparableProposalId>> = [
                    roots.protocol_parameters,
                    roots.constitution,
                    roots.constitutional_committee,
                    roots.hard_fork,
                ]
                .into_iter()
                .flatten()
                .collect();

                let len = non_empty_roots.len();

                if len == 0 {
                    None
                } else {
                    Some((
                        DebugAsDisplay(forest),
                        non_empty_roots[ix as usize % len].as_ref().clone(),
                    ))
                }
            },
        )
    }

    // Generate a *somewhat meaningful* proposal forest, with relationships and links between
    // proposals.
    fn any_proposals_forest() -> impl Strategy<Value = DebugAsDisplay<ProposalsForest>> {
        let any_ids = collection::btree_set(
            any_comparable_proposal_id().prop_map(Rc::new),
            4 * (MAX_TREE_SIZE + 2),
        )
        .prop_map(|ids| ids.into_iter().collect::<Vec<_>>());

        any_ids.prop_flat_map(|ids: Vec<Rc<ComparableProposalId>>| {
            let (lo, hi) = (0, MAX_TREE_SIZE + 1);
            let any_protocol_parameters_tree = any_proposals_tree(
                ids[lo..hi].into(),
                any_protocol_params_update(),
                |parent, update| {
                    GovAction::ParameterChange(parent, Box::new(update), Nullable::Null)
                },
            );

            let (lo, hi) = (hi + 1, hi + MAX_TREE_SIZE + 2);
            let any_hard_fork_tree = any_proposals_tree(
                ids[lo..hi].into(),
                any_protocol_version(),
                GovAction::HardForkInitiation,
            );

            let (lo, hi) = (hi + 1, hi + MAX_TREE_SIZE + 2);
            let any_constitution_tree = any_proposals_tree(
                ids[lo..hi].into(),
                any_constitution(),
                GovAction::NewConstitution,
            );

            let (lo, hi) = (hi + 1, hi + MAX_TREE_SIZE + 2);
            let any_constitutional_committee_tree = any_proposals_tree(
                ids[lo..hi].into(),
                any_committee_update(
                    (MIN_ARBITRARY_EPOCH..MAX_ARBITRARY_EPOCH).prop_map(Epoch::from),
                ),
                |parent, update| match update {
                    CommitteeUpdate::NoConfidence => GovAction::NoConfidence(parent),
                    CommitteeUpdate::ChangeMembers {
                        threshold,
                        added,
                        removed,
                    } => GovAction::UpdateCommittee(
                        parent,
                        Set::from(removed.into_iter().collect::<Vec<_>>()),
                        KeyValuePairs::from(
                            added
                                .into_iter()
                                .map(|(k, v)| (k, u64::from(v)))
                                .collect::<Vec<(_, _)>>(),
                        ),
                        #[allow(clippy::unwrap_used)]
                        RationalNumber {
                            numerator: threshold.numer().try_into().unwrap(),
                            denominator: threshold.denom().try_into().unwrap(),
                        },
                    ),
                },
            );

            (
                any_protocol_parameters_tree,
                any_hard_fork_tree,
                any_constitutional_committee_tree,
                any_constitution_tree,
            )
                .prop_map(
                    |(protocol_parameters, hard_fork, constitutional_committee, constitution)| {
                        let mut forest = ProposalsForest::new(
                            // Leave one epoch for proposals that are fresh but not ready for ratification
                            // yet.
                            Epoch::from(MIN_ARBITRARY_EPOCH + 1),
                            &ProposalsRootsRc {
                                protocol_parameters: protocol_parameters.0,
                                hard_fork: hard_fork.0,
                                constitutional_committee: constitutional_committee.0,
                                constitution: constitution.0,
                            },
                        );

                        std::iter::empty()
                            .chain(protocol_parameters.1)
                            .chain(hard_fork.1)
                            .chain(constitutional_committee.1)
                            .chain(constitution.1)
                            .for_each(|(id, pointer, action)| {
                                #[allow(clippy::unwrap_used)]
                                forest
                                    .insert(&ERA_HISTORY, id.clone(), pointer, action.clone())
                                    .unwrap();
                            });

                        DebugAsDisplay(forest)
                    },
                )
        })
    }

    // Generate an tree of proposals with valid parents in the tree. This gets rapidly tricky, as
    // we have generators depending on generators. To simplify a bit the generation process, we
    // mostly operate on indices; which we use to lookup already generated data.
    //
    // The `ids` in argument also allows us to make sure that ids are unique across all trees (we
    // generate the sequence as a BTreeSet outside of this generator).
    //
    // We strive for the sequence to be as arbitrary as possible. We return
    fn any_proposals_tree<Arg: 'static>(
        ids: Vec<Rc<ComparableProposalId>>,
        any_action_arg: impl Strategy<Value = Arg>,
        into_action: impl Fn(Nullable<ProposalId>, Arg) -> GovAction,
    ) -> impl Strategy<
        Value = (
            // An optional root
            Option<Rc<ComparableProposalId>>,
            // A sequence of proposals (a.k.a GovAction) and the epoch in which they've been
            // proposed.
            Vec<(ComparableProposalId, ProposalPointer, GovAction)>,
        ),
    > {
        // We generate indices for the
        let any_root = prop_oneof![Just(None), Just(Some(0))];
        let any_parents = collection::vec(any::<u8>(), 0..MAX_TREE_SIZE);
        let any_action_args = collection::vec(any_action_arg, MAX_TREE_SIZE);
        let any_pointers = collection::vec(any_proposal_pointer(u64::MAX), MAX_TREE_SIZE);

        (
            Just(ids),
            any_root,
            any_parents,
            any_pointers,
            any_action_args,
        )
            .prop_map(move |(ids, root, parents, mut pointers, mut args)| {
                let mut next = 1;

                let root = root.map(|ix| ids[ix].clone());

                let mut known_parents = vec![root.clone()];

                let mut sequence = Vec::new();

                for parent in parents {
                    let sibling = ids[next].clone();

                    let action = into_action(select(&known_parents, parent), args.remove(0));

                    let pointer = pointers.remove(0);

                    sequence.push((sibling.as_ref().clone(), pointer, action));

                    known_parents.push(Some(sibling));

                    next += 1;
                }

                (root.clone(), sequence)
            })
    }

    // Test Helpers
    // ----------------------------------------------------------------------------

    fn possible_parents(
        forest: &ProposalsForest,
        action: &GovAction,
    ) -> Vec<Option<Rc<ComparableProposalId>>> {
        use super::{GovAction::*, ProposalEnum::*};

        let root = match action {
            ParameterChange(..) => vec![forest.roots().protocol_parameters],

            HardForkInitiation(..) => vec![forest.roots().hard_fork],

            UpdateCommittee(..) | NoConfidence(..) => {
                vec![forest.roots().constitutional_committee]
            }

            NewConstitution(..) => vec![forest.roots().constitution],

            TreasuryWithdrawals(..) | Information => vec![],
        };

        forest
            .sequence
            .iter()
            .filter_map(|id| {
                let keep = || Some(Some(id.clone()));
                match (&forest.proposals.get(id)?.proposal, action) {
                    (ProtocolParameters(..), ParameterChange(..)) => keep(),
                    (Constitution(..), NewConstitution(..)) => keep(),
                    (HardFork(..), HardForkInitiation(..)) => keep(),
                    (ConstitutionalCommittee(..), UpdateCommittee(..)) => keep(),
                    (_, _) => None,
                }
            })
            .chain(root)
            .collect()
    }

    // Overwrite the parent of the given governance action
    fn set_parent(action: GovAction, parent: Nullable<ProposalId>) -> GovAction {
        use GovAction::*;
        match action {
            Information | TreasuryWithdrawals(..) => action,
            NoConfidence(_) => NoConfidence(parent),
            ParameterChange(_, params, guardrails) => ParameterChange(parent, params, guardrails),
            UpdateCommittee(_, removed, added, threshold) => {
                UpdateCommittee(parent, removed, added, threshold)
            }
            HardForkInitiation(_, version) => HardForkInitiation(parent, version),
            NewConstitution(_, constitution) => NewConstitution(parent, constitution),
        }
    }

    // Select an element from a list by its position, wrapping the position around if it overflows
    // the list. For a non empty list, this ensures to return an element from the list.
    fn select(list: &[Option<Rc<ComparableProposalId>>], ix: u8) -> Nullable<ProposalId> {
        list.get(ix as usize % list.len())
            .unwrap_or_else(|| unreachable!("out of bound"))
            .as_ref()
            .map(|id| Nullable::Some(ProposalId::from(id.as_ref().clone())))
            .unwrap_or(Nullable::Null)
    }

    /// A type helper to ease counterexamples display.
    struct DebugAsDisplay<T>(T);
    impl<T: std::fmt::Display> std::fmt::Debug for DebugAsDisplay<T> {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(&self.0.to_string())
        }
    }
}
