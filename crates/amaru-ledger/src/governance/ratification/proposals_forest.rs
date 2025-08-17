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
use crate::summary::into_safe_ratio;
use amaru_kernel::{
    display_protocol_parameters_update, expect_stake_credential, ComparableProposalId,
    Constitution, Epoch, GovAction, Nullable, ProposalId, ProtocolParamUpdate, ProtocolVersion,
};
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    rc::Rc,
};
use tracing::error;

pub use super::proposals_tree::{ProposalsEnactError, ProposalsInsertError};

#[derive(Debug)]
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
        proposed_in: Epoch,
        proposal: GovAction,
    ) -> Result<(), ProposalsInsertError<ComparableProposalId>> {
        use amaru_kernel::GovAction::*;

        let id = Rc::new(id);

        // FIXME: insert in priority order
        //
        // no confidence -> 1st
        // constitutional committee -> 2nd
        // constitution -> 3rd
        // hard fork -> 4th
        // protocol parameters -> 5th
        // treasury withdrawals -> 6th
        // poll -> 7th
        self.sequence.push_back(id.clone());

        let mut insert = |proposal| {
            self.proposals.insert(
                id.clone(),
                ProposedIn {
                    epoch: proposed_in,
                    proposal,
                },
            )
        };

        match proposal {
            ParameterChange(parent, update, _guardrails_script) => {
                let parent = into_parent_id(parent);

                self.protocol_parameters
                    .insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::ProtocolParameters(*update, parent));

                Ok(())
            }

            HardForkInitiation(parent, protocol_version) => {
                let parent = into_parent_id(parent);

                self.hard_fork.insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::HardFork(protocol_version, parent));

                Ok(())
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
                )));

                Ok(())
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
                ));

                Ok(())
            }

            NoConfidence(parent) => {
                let parent = into_parent_id(parent);

                self.constitutional_committee
                    .insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::ConstitutionalCommittee(
                    CommitteeUpdate::NoConfidence,
                    parent,
                ));

                Ok(())
            }

            NewConstitution(parent, constitution) => {
                let parent = into_parent_id(parent);

                self.constitution.insert(id.clone(), parent.clone())?;

                insert(ProposalEnum::Constitution(constitution, parent));

                Ok(())
            }

            Information => {
                insert(ProposalEnum::Orphan(OrphanProposal::NicePoll));
                Ok(())
            }
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
            ProposalEnum::Orphan(..) => Ok((id, BTreeSet::new())),
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
    ) -> Option<(Rc<ComparableProposalId>, &'forest ProposalEnum)> {
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
            let result: Option<(Rc<ComparableProposalId>, &'forest ProposalEnum)> = {
                let id = forest.sequence.get(self.cursor)?.clone();

                let ProposedIn {
                    epoch: proposed_in,
                    proposal,
                } = forest.proposals.get(&id).or_else(|| {
                    error!("forest's sequence knows of the id {id:?} but it wasn't found in the lookup-table");
                    None
                })?;

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
                    Some((id, proposal))
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
                s.as_id().to_string().chars().take(12).collect::<String>(),
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

        let others = self
            .sequence
            .iter()
            .filter_map(|id| {
                if let ProposalEnum::Orphan(o) = &self.proposals.get(id).as_ref()?.proposal {
                    Some(o)
                } else {
                    None
                }
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

/// A type akin to a (Epoch, T), but with field name for readability.
#[derive(Debug, Clone)]
pub struct ProposedIn<T> {
    pub epoch: Epoch,
    pub proposal: T,
}

fn into_parent_id(nullable: Nullable<ProposalId>) -> Option<Rc<ComparableProposalId>> {
    match nullable {
        Nullable::Undefined | Nullable::Null => None,
        Nullable::Some(id) => Some(Rc::new(ComparableProposalId::from(id))),
    }
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::ProposalsForest;
    use crate::governance::ratification::ProposalsRootsRc;
    use amaru_kernel::{
        tests::{any_comparable_proposal_id, any_gov_action, any_protocol_params_update},
        ComparableProposalId, Epoch, GovAction, Nullable, ProposalId,
    };
    use proptest::{collection, prelude::*};
    use std::{collections::BTreeSet, rc::Rc};

    const TREE_MAX_DEPTH: usize = 8;
    const MIN_ARBITRARY_EPOCH: u64 = 0;
    const MAX_ARBITRARY_EPOCH: u64 = 5;

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

    // TODO:
    //
    // == Properties:
    //
    // After enacting a constitution, hard fork, or committee change; the compass always yield none;
    // Skip recently submitted proposal
    // Sequence contains no duplicate
    // All proposals in 'sequence' have a corresponding match in 'proposals'
    // Cannot insert proposal with unknown parent
    //
    // Compass throws if re-used;

    proptest! {
        #[test]
        fn prop_insert_increase_sizes_by_one(
            mut forest in any_proposals_forest(),
            id in any_comparable_proposal_id(),
            mut action in any_gov_action(),
            parent in any::<u8>()
        ) {
            let size_before = check_invariants(&forest);

            let parents = possible_parents(&forest, &action);
            if !parents.is_empty() {
                action = set_parent(action, select(&parents, parent));
            }
            forest.insert(id, forest.current_epoch, action).unwrap();

            let size_after = check_invariants(&forest);

            prop_assert_eq!(size_before + 1, size_after);
        }
    }

    proptest! {
        #[test]
        fn prop_compass_traverse_whole_forest_and_eventually_yield_none(
            forest in any_proposals_forest(),
        ) {
            use super::ProposalEnum::*;

            let mut compass = forest.new_compass();
            while let Some((_id, proposal)) = compass.next(&forest) {
                // TODO: Check that priority is lower or equal to previous priority.

                // Ensures that the yielded proposal always has a matching root.
                let roots = forest.roots();
                match proposal {
                    ProtocolParameters(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.protocol_parameters.as_ref()
                    ),
                    HardFork(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.hard_fork.as_ref()
                    ),
                    ConstitutionalCommittee(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.constitutional_committee.as_ref()
                    ),
                    Constitution(_, parent) => prop_assert_eq!(
                        parent.as_ref(),
                        roots.constitution.as_ref()
                    ),
                    Orphan(..) => ()
                }
            }
        }
    }

    fn any_proposals_forest() -> impl Strategy<Value = ProposalsForest> {
        let any_ids = collection::btree_set(
            any_comparable_proposal_id().prop_map(Rc::new),
            4 * (TREE_MAX_DEPTH + 1) + TREE_MAX_DEPTH,
        );

        any_ids.prop_flat_map(|ids| {
            let any_protocol_parameters_tree = any_protocol_parameters_tree(
                ids.clone().into_iter().collect(),
                any_protocol_params_update(),
                |parent, update| {
                    GovAction::ParameterChange(parent, Box::new(update), Nullable::Null)
                },
            );

            any_protocol_parameters_tree.prop_map(|(root_params, seq_params)| {
                let mut forest = ProposalsForest::new(
                    // Leave one epoch for proposals that are fresh but not ready for ratification
                    // yet.
                    Epoch::from(MIN_ARBITRARY_EPOCH + 1),
                    &ProposalsRootsRc {
                        protocol_parameters: root_params,
                        hard_fork: None,
                        constitutional_committee: None,
                        constitution: None,
                    },
                );

                for (id, epoch, action) in seq_params.into_iter() {
                    forest.insert(id, epoch, action).unwrap();
                }

                forest
            })
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
    fn any_protocol_parameters_tree<Arg: 'static>(
        ids: Vec<Rc<ComparableProposalId>>,
        any_action_arg: impl Strategy<Value = Arg>,
        into_action: impl Fn(Nullable<ProposalId>, Arg) -> GovAction,
    ) -> impl Strategy<
        Value = (
            // An optional root
            Option<Rc<ComparableProposalId>>,
            // A sequence of proposals (a.k.a GovAction) and the epoch in which they've been
            // proposed.
            Vec<(ComparableProposalId, Epoch, GovAction)>,
        ),
    > {
        // We generate indices for the
        let any_root = prop_oneof![Just(None), Just(Some(0))];
        let any_parents = collection::vec(any::<u8>(), 0..TREE_MAX_DEPTH);
        let any_action_args = collection::vec(any_action_arg, TREE_MAX_DEPTH);
        let any_epochs = collection::vec(MIN_ARBITRARY_EPOCH..MAX_ARBITRARY_EPOCH, TREE_MAX_DEPTH);

        (
            Just(ids),
            any_root,
            any_parents,
            any_epochs,
            any_action_args,
        )
            .prop_map(move |(ids, root, parents, mut epochs, mut args)| {
                let mut next = 1;

                let root = root.map(|ix| ids[ix].clone());

                let mut known_parents = vec![root.clone()];

                let mut sequence = Vec::new();

                for parent in parents {
                    let sibling = ids[next].clone();

                    let action = into_action(select(&known_parents, parent), args.remove(0));

                    let epoch = Epoch::from(epochs.remove(0));

                    sequence.push((sibling.as_ref().clone(), epoch, action));

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
            Information | TreasuryWithdrawals(_, _) => action,
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
            .unwrap_or_else(|| panic!("out of bound"))
            .as_ref()
            .map(|id| Nullable::Some(ProposalId::from(id.as_ref().clone())))
            .unwrap_or(Nullable::Null)
    }
}
