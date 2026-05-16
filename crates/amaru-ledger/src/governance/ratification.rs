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

use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};

use amaru_kernel::{
    Ballot, ComparableProposalId, Constitution, ConstitutionalCommitteeStatus, DRep, Epoch, EraHistory, Lovelace,
    PoolId, ProtocolParameters, StakeCredential, Vote, Voter,
};
use amaru_observability::info_span;
use num::Zero;
use tracing::{Span, field};

use crate::{
    state::StakeDistributionView,
    store::{Snapshot, StoreError},
    summary::{SafeRatio, into_safe_ratio, stake_distribution::StakeDistribution},
};

mod constitutional_committee;
pub use constitutional_committee::ConstitutionalCommittee;

mod dreps;

mod stake_pools;

mod proposal_enum;
pub use proposal_enum::*;

mod proposals_forest;
pub use proposals_forest::*;

mod proposals_roots;
pub use proposals_roots::*;

mod proposals_tree;

/// All informations needed to ratify votes.
pub struct RatificationContext<'distr> {
    /// The epoch for which this context is for.
    pub epoch: Epoch,

    /// The *current* (live! not from the stake distr snapshot) value of the treasury
    pub treasury: Lovelace,

    /// The computed stake distribution for the epoch
    pub stake_distribution: StakeDistributionView<'distr>,

    /// Last enacted protocol parameters for this epoch.
    pub protocol_parameters: ProtocolParameters,

    /// All proposals that have been pruned due to ratification or conflict.
    pub pruned_proposals: BTreeSet<Rc<ComparableProposalId>>,

    /// Enacted withdrawals during this round of ratification.
    pub withdrawals: BTreeMap<StakeCredential, Lovelace>,

    /// The current constitutional committee, if any. No committee signals a state of
    /// no-confidence.
    pub constitutional_committee: Option<ConstitutionalCommittee>,

    /// An update that occured on the constitutional committee
    pub constitutional_committee_update: Option<CommitteeUpdate>,

    /// A new constitution that has been voted and approved, if any.
    pub new_constitution: Option<Constitution>,

    /// All latest votes indexed by proposals and voters.
    pub votes: BTreeMap<ComparableProposalId, Vec<(Voter, Ballot)>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RatificationInternalError {
    #[error("invalid operation while creating the proposals forest: {0}")]
    InternalForestCreationError(#[from] ProposalsInsertError<ComparableProposalId>),

    #[error("invalid operation while enacting a proposal: {0}")]
    InternalForestEnactmentError(#[from] ProposalsEnactError<ComparableProposalId>),
}

impl<'distr> RatificationContext<'distr> {
    pub fn new(
        snapshot: impl Snapshot,
        stake_distribution: StakeDistributionView<'distr>,
        protocol_parameters: ProtocolParameters,
        treasury: Lovelace,
    ) -> Result<Self, StoreError> {
        info_span!(amaru_observability::amaru::ledger::state::RATIFICATION_CONTEXT_NEW).in_scope(|| {
            let constitutional_committee = match snapshot.constitutional_committee()? {
                ConstitutionalCommitteeStatus::NoConfidence => None,
                ConstitutionalCommitteeStatus::Trusted { threshold } => {
                    let members = snapshot
                        .iter_cc_members()?
                        .filter_map(|(cold_credential, row)| {
                            row.valid_until.map(|valid_until| (cold_credential, (row.hot_credential, valid_until)))
                        })
                        .collect();

                    Some(ConstitutionalCommittee::new(into_safe_ratio(&threshold), members))
                }
            };

            // FIXME: votes entirely stored in-memory
            //
            // This isn't ideal , as we collect all votes in memory here. This is okay-ish on most
            // networks because the number of votes is rather small. Even with 1M+ votes, this shouldn't
            // require much memory; but it becomes a potential attack vector.
            //
            // We must avoid loading ALL votes in memory at once. Especially since we do not prune
            // votes at the moment...
            let votes = snapshot.iter_votes()?.fold(BTreeMap::new(), |mut votes, (k, v)| {
                votes.entry(k.proposal).or_insert_with(Vec::new).push((k.voter, v));
                votes
            });

            Ok(RatificationContext {
                // Ratification happens with one epoch of delay, and at the next epoch transition. So,
                // if we ratify votes that happened in epoch `e`, the ratification is done during the
                // transition from `e + 1` to `e + 2`; but it is done "as if" it was happening at the
                // beginning of epoch `e + 1`. So, the epoch we consider for DRep mandates and proposal
                // expiry is the one from after the snapshot.
                epoch: snapshot.epoch() + 1,
                treasury,
                stake_distribution,
                protocol_parameters,
                pruned_proposals: BTreeSet::new(),
                withdrawals: BTreeMap::new(),
                constitutional_committee,
                constitutional_committee_update: None,
                new_constitution: None,
                votes,
            })
        })
    }

    pub fn ratify_proposals(
        &mut self,
        era_history: &EraHistory,
        proposals: Vec<(Rc<ComparableProposalId>, CandidateProposal)>,
        roots: ProposalsRootsRc,
    ) -> Result<ProposalsRootsRc, RatificationInternalError> {
        info_span!(
            amaru_observability::amaru::ledger::governance::RATIFY_PROPOSALS,
            roots_protocol_parameters = opt_root(roots.protocol_parameters.as_deref()),
            roots_hard_fork = opt_root(roots.hard_fork.as_deref()),
            roots_constitutional_committee = opt_root(roots.constitutional_committee.as_deref()),
            roots_constitution = opt_root(roots.constitution.as_deref())
        )
        .in_scope(|| {
            // A forest (i.e. a multitude of trees) that tracks what proposals needs to be ratified,
            // in what order and what are the relationships between proposals; such that, when a
            // proposal is enacted, conflicting proposals (those pointing at the same parent) are
            // pruned.
            let mut forest = ProposalsForest::new(self.epoch, &roots, self.treasury).drain(era_history, proposals)?;

            // A mutable compass to navigate the forest. This compass holds the tiny bit of mutable
            // state we need to iterate over the forest; but without introducing a mutable borrow on
            // the forest. This allows to interleave updates on the forest when needed.
            //
            // Any update on the forest invalidate the compass, which must be replaced to continue
            // iterating.
            let mut compass = forest.new_compass();

            loop {
                // The inner block limits the lifetime of the immutable borrow(s) on forest; so that we
                // can then borrow the forest as immutable when a proposal gets ratified.
                let ratified: Option<(Rc<ComparableProposalId>, ProposalEnum)> = {
                    let Some((id, (proposal, _))) = compass.next(&forest, &self.protocol_parameters) else {
                        break;
                    };

                    Self::new_ratify_span(&id, proposal).in_scope(|| {
                        if self.is_accepted_by_everyone(&id, proposal, &self.stake_distribution) {
                            // NOTE: The .clone() on the proposal is necessary so we can drop the
                            // immutable borrow on the forest. It only happens when a proposal is
                            // ratified, though.
                            //
                            // The .clone() on the id is cheap, because it's an Rc.
                            Some((id.clone(), proposal.clone()))
                        } else {
                            None
                        }
                    })
                }; // <-- immutable borrows of `forest` end here

                if let Some((id, proposal)) = ratified {
                    self.enact_proposal(id, proposal, &mut forest, &mut compass)?;
                }
            }

            // Finally, replace the roots in the store. Note that this is pretty much a no-op when no
            // proposals are ratified; but it's just one db key/value update.
            Ok(forest.roots())
        })
    }

    /// Apply the effect of a (now-approved) proposal to both the current state (i.e. self), and
    /// the persistent store. Note that updates to the store are actually happening *after*, once
    /// the ratification procedure is fully done. So we only stash updates, in order (as some
    /// updates may override some previous update in the same ratification).
    fn enact_proposal(
        &mut self,
        id: Rc<ComparableProposalId>,
        proposal: ProposalEnum,
        forest: &mut ProposalsForest,
        compass: &mut ProposalsForestCompass,
    ) -> Result<(), RatificationInternalError> {
        Self::new_enact_span(&id, &proposal).in_scope(|| -> Result<(), RatificationInternalError> {
            let mut now_obsolete = forest.enact(id, &proposal, compass)?;

            tracing::Span::current().record(
                "proposals.pruned",
                now_obsolete.iter().map(|id| id.to_compact_string()).collect::<Vec<_>>().join(", "),
            );

            self.pruned_proposals.append(&mut now_obsolete);

            match proposal {
                ProposalEnum::ProtocolParameters(params_update, _parent) => {
                    self.protocol_parameters.update(params_update);
                }

                ProposalEnum::HardFork(protocol_version, _parent) => {
                    self.protocol_parameters.protocol_version = protocol_version;
                }

                ProposalEnum::ConstitutionalCommittee(update, _parent) => {
                    match update.clone() {
                        CommitteeUpdate::NoConfidence => {
                            self.constitutional_committee = None;
                        }
                        CommitteeUpdate::ChangeMembers { removed, added, threshold } => {
                            let added_as_inactive = added
                                .iter()
                                .map(|(cold_cred, valid_until)| (cold_cred.clone(), (None, *valid_until)))
                                .collect();

                            if let Some(committee) = &mut self.constitutional_committee {
                                committee.update(threshold, added_as_inactive, removed.iter().collect());
                            } else {
                                self.constitutional_committee =
                                    Some(ConstitutionalCommittee::new(threshold, added_as_inactive));
                            }
                        }
                    }

                    self.constitutional_committee_update = Some(update);
                }

                ProposalEnum::Constitution(constitution, _parent) => {
                    self.new_constitution = Some(constitution);
                }

                ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal(withdrawals)) => {
                    for (credential, amount) in withdrawals.into_iter() {
                        self.withdrawals.entry(credential).and_modify(|balance| *balance += amount).or_insert(amount);
                    }
                }

                ProposalEnum::Orphan(OrphanProposal::NicePoll) => {
                    unreachable!("enacted an un-enactable proposal kind ?!")
                }
            }

            Ok(())
        })
    }

    fn new_enact_span(id: &ComparableProposalId, proposal: &ProposalEnum) -> Span {
        tracing::info_span!(
            "enacting",
            "proposal.id" = id.to_compact_string(),
            "proposal.kind" = proposal.display_kind(),
        )
    }

    fn new_ratify_span(id: &ComparableProposalId, proposal: &ProposalEnum) -> Span {
        info_span!(
            amaru_observability::amaru::ledger::governance::RATIFYING,
            proposal_id = id.to_compact_string(),
            proposal_kind = proposal.display_kind().to_string(),
        )
    }

    fn is_accepted_by_everyone(
        &self,
        id: &ComparableProposalId,
        proposal: &ProposalEnum,
        stake_distribution: &StakeDistribution,
    ) -> bool {
        let span = tracing::Span::current();

        let (dreps_votes, cc_votes, pool_votes) =
            partition_votes(self.votes.get(id).map(|v| v.as_slice()).unwrap_or(&[]));

        // NOTE: because ratification is an expensive operation, and something that we *may* have
        // to replay due to rollbacks, it's important to do the least amount of work possible.
        //
        // If the CC doesn't approve, then the majority of actions can be considered invalid and
        // there's no need to check for DReps nor SPO votes. The following code is thus written in
        // a way that the next governance body is only consulted should the previous one be "no".

        let cc_approved = self.is_accepted_by_constitutional_committee(proposal, cc_votes);

        span.record("approved.committee", cc_approved);

        if cc_approved {
            let spos_approved = self.is_accepted_by_stake_pool_operators(proposal, pool_votes, stake_distribution);

            span.record("approved.pools", spos_approved);

            if spos_approved {
                let dreps_approved =
                    self.is_accepted_by_delegate_representatives(proposal, dreps_votes, stake_distribution);

                span.record("approved.dreps", dreps_approved);

                return dreps_approved;
            }
        }

        false
    }

    fn is_accepted_by_constitutional_committee(
        &self,
        proposal: &ProposalEnum,
        votes: BTreeMap<StakeCredential, &Vote>,
    ) -> bool {
        self.constitutional_committee
            .as_ref()
            .and_then(|committee| {
                let threshold = committee.voting_threshold(
                    self.epoch,
                    self.protocol_parameters.protocol_version,
                    self.protocol_parameters.min_committee_size,
                    proposal,
                )?;

                tracing::Span::current().record("required_threshold.committee", field::display(&threshold));

                let tally = || committee.tally(self.epoch, votes);

                Some(threshold == &SafeRatio::zero() || &tally() >= threshold)
            })
            .unwrap_or(false)
    }

    fn is_accepted_by_stake_pool_operators(
        &self,
        proposal: &ProposalEnum,
        votes: BTreeMap<&PoolId, &Vote>,
        stake_distribution: &StakeDistribution,
    ) -> bool {
        match stake_pools::voting_threshold(
            self.constitutional_committee.is_none(),
            &self.protocol_parameters.pool_voting_thresholds,
            proposal,
        ) {
            None => false,
            Some(threshold) => {
                tracing::Span::current().record("required_threshold.pools", field::display(&threshold));

                let tally = || {
                    stake_pools::tally(self.protocol_parameters.protocol_version, proposal, votes, stake_distribution)
                };

                threshold == SafeRatio::zero() || tally() >= threshold
            }
        }
    }

    fn is_accepted_by_delegate_representatives(
        &self,
        proposal: &ProposalEnum,
        votes: BTreeMap<DRep, &Vote>,
        stake_distribution: &StakeDistribution,
    ) -> bool {
        match dreps::voting_threshold(
            self.protocol_parameters.protocol_version,
            self.constitutional_committee.is_none(),
            &self.protocol_parameters.drep_voting_thresholds,
            proposal,
        ) {
            None => false,
            Some(threshold) => {
                tracing::Span::current().record("required_threshold.dreps", field::display(&threshold));

                let tally = || -> SafeRatio { dreps::tally(self.epoch, proposal, votes, stake_distribution) };

                threshold == SafeRatio::zero() || tally() >= threshold
            }
        }
    }
}

// Helpers
// ----------------------------------------------------------------------------

/// Split all the ballots into sub-maps that are specific to each voter types; so that we ease the
/// processing of each category down the line.
fn partition_votes(
    votes: &[(Voter, Ballot)],
) -> (BTreeMap<DRep, &Vote>, BTreeMap<StakeCredential, &Vote>, BTreeMap<&PoolId, &Vote>) {
    votes.iter().fold(
        (BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        |(mut dreps, mut committee, mut pools), (voter, ballot)| {
            match voter {
                Voter::ConstitutionalCommitteeKey(hash) => {
                    committee.insert(StakeCredential::AddrKeyhash(*hash), ballot.vote());
                }
                Voter::ConstitutionalCommitteeScript(hash) => {
                    committee.insert(StakeCredential::ScriptHash(*hash), ballot.vote());
                }
                Voter::DRepKey(hash) => {
                    dreps.insert(DRep::Key(*hash), ballot.vote());
                }
                Voter::DRepScript(hash) => {
                    dreps.insert(DRep::Script(*hash), ballot.vote());
                }
                Voter::StakePoolKey(pool_id) => {
                    pools.insert(pool_id, ballot.vote());
                }
            };

            (dreps, committee, pools)
        },
    )
}

fn opt_root(opt: Option<&ComparableProposalId>) -> String {
    opt.map(|r| r.to_compact_string()).unwrap_or_else(|| "none".to_string())
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(any(all(test, not(target_os = "windows")), feature = "test-utils"))]
pub use tests::*;

#[cfg(any(all(test, not(target_os = "windows")), feature = "test-utils"))]
mod tests {
    use std::{sync::LazyLock, time::Duration};

    use amaru_kernel::{Epoch, EraBound, EraHistory, EraName, EraParams, EraSummary, Slot};

    // Technically higher than the actual gap we may see in 'real life', but, why not.
    pub const MAX_ARBITRARY_EPOCH: u64 = 10;
    pub const MIN_ARBITRARY_EPOCH: u64 = 0;

    pub static ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
        EraHistory::new(
            &[EraSummary {
                start: EraBound { time: Duration::from_secs(0), slot: Slot::from(0), epoch: Epoch::from(0) },
                end: None,
                params: EraParams {
                    // Pick an epoch length such that epochs falls within the min and max bounds;
                    // knowing that slots ranges across all u64.
                    epoch_size_slots: u64::MAX / (MAX_ARBITRARY_EPOCH - MIN_ARBITRARY_EPOCH + 1),
                    slot_length: Duration::from_secs(1),
                    era_name: EraName::Conway,
                },
            }],
            Slot::from(0),
        )
    });

    #[cfg(all(test, not(target_os = "windows")))]
    mod internal {
        use amaru_kernel::any_proposal_pointer;
        use proptest::{prelude::*, test_runner::RngSeed};

        use super::*;

        proptest! {
            #[test]
            fn prop_era_history_yields_within_epoch_bounds(pointer in any_proposal_pointer(u64::MAX)) {
                let epoch = ERA_HISTORY.slot_to_epoch(pointer.slot(), pointer.slot()).unwrap();
                prop_assert!(
                    epoch >= Epoch::from(MIN_ARBITRARY_EPOCH) && epoch <= Epoch::from(MAX_ARBITRARY_EPOCH),
                    "generated a pointer outside of the configured epoch range; epoch = {epoch}"
                );
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig { rng_seed: RngSeed::Fixed(42), ..ProptestConfig::default() })]
            #[test]
            #[should_panic]
            fn prop_proposal_pointer_sometimes_min_epoch(pointer in any_proposal_pointer(u64::MAX)) {
                let epoch = ERA_HISTORY.slot_to_epoch(pointer.slot(), pointer.slot()).unwrap();
                prop_assert!(epoch != Epoch::from(MIN_ARBITRARY_EPOCH));
            }
        }

        proptest! {
            #![proptest_config(ProptestConfig { rng_seed: RngSeed::Fixed(42), ..ProptestConfig::default() })]
            #[test]
            #[should_panic]
            fn prop_proposal_pointer_sometimes_max_epoch(pointer in any_proposal_pointer(u64::MAX)) {
                let epoch = ERA_HISTORY.slot_to_epoch(pointer.slot(), pointer.slot()).unwrap();
                prop_assert!(epoch != Epoch::from(MAX_ARBITRARY_EPOCH));
            }
        }
    }
}
