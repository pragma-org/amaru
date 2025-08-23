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

use crate::{
    governance::ratification::proposals_forest::ProposalsForestCompass,
    state::StakeDistributionView,
    store::{columns::proposals, StoreError, TransactionalContext},
    summary::{stake_distribution::StakeDistribution, SafeRatio},
};
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, Ballot, ComparableProposalId, DRep, Epoch, EraHistory,
    Lovelace, PoolId, StakeCredential, UnitInterval, Vote, Voter,
};
use num::Zero;
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
};
use tracing::{debug_span, field, info, info_span, trace_span, Span};

mod constitutional_committee;
pub use constitutional_committee::ConstitutionalCommittee;

mod dreps;

mod stake_pools;

mod proposal_enum;
use proposal_enum::{CommitteeUpdate, OrphanProposal, ProposalEnum};

mod proposals_forest;
use proposals_forest::{ProposalsEnactError, ProposalsForest, ProposalsInsertError};

mod proposals_roots;
pub use proposals_roots::{ProposalsRoots, ProposalsRootsRc};

mod proposals_tree;

/// All informations needed to ratify votes.
pub struct RatificationContext<'distr> {
    /// The epoch that just ended.
    pub epoch: Epoch,

    /// The *current* (live! not from the stake distr snapshot) value of the treasury
    pub treasury: Lovelace,

    /// The computed stake distribution for the epoch
    pub stake_distribution: StakeDistributionView<'distr>,

    /// Last enacted protocol parameters for this epoch.
    pub protocol_parameters: ProtocolParameters,

    /// The current constitutional committee, if any. No committee signals a state of
    /// no-confidence.
    pub constitutional_committee: Option<ConstitutionalCommittee>,

    /// All latest votes indexed by proposals and voters.
    pub votes: BTreeMap<ComparableProposalId, BTreeMap<Voter, Ballot>>,
}

#[derive(Debug, thiserror::Error)]
pub enum RatificationInternalError {
    #[error("invalid operation while creating the proposals forest: {0}")]
    InternalForestCreationError(#[from] ProposalsInsertError<ComparableProposalId>),

    #[error("invalid operation while enacting a proposal: {0}")]
    InternalForestEnactmentError(#[from] ProposalsEnactError<ComparableProposalId>),
}

pub struct RatificationResult<'distr, S> {
    pub context: RatificationContext<'distr>,
    pub store_updates: Vec<StoreUpdate<'distr, S>>,
    pub pruned_proposals: BTreeSet<Rc<ComparableProposalId>>,
}

pub type StoreUpdate<'distr, S> =
    Box<dyn FnOnce(&S, &RatificationContext<'distr>) -> Result<(), StoreError>>;

impl<'distr> RatificationContext<'distr> {
    pub fn ratify_proposals<'store, S: TransactionalContext<'store>>(
        mut self,
        era_history: &EraHistory,
        proposals: Vec<(ComparableProposalId, proposals::Row)>,
        roots: ProposalsRootsRc,
    ) -> Result<RatificationResult<'distr, S>, RatificationInternalError> {
        info_roots(&roots);

        // A forest (i.e. a multitude of trees) that tracks what proposals needs to be ratified,
        // in what order and what are the relationships between proposals; such that, when a
        // proposal is enacted, conflicting proposals (those pointing at the same parent) are
        // pruned.
        let mut forest = ProposalsForest::new(self.epoch, &roots, self.treasury)
            .drain(era_history, proposals)?;

        // A mutable compass to navigate the forest. This compass holds the tiny bit of mutable
        // state we need to iterate over the forest; but without introducing a mutable borrow on
        // the forest. This allows to interleave updates on the forest when needed.
        //
        // Any update on the forest invalidate the compass, which must be replaced to continue
        // iterating.
        let mut compass = forest.new_compass();

        // We collect updates to be done on the store while enacting proposals. The updates aren't
        // executed, but stashed for later. This allows processing proposals in a pure fashion,
        // while accumulating changes to be done on the store.
        let mut store_updates: Vec<StoreUpdate<'distr, S>> = Vec::new();

        // We also accumulate proposals that get pruned due to other proposals being enacted. Those
        // proposals are obsolete, and must be removed from the database.
        let mut pruned_proposals: BTreeSet<Rc<ComparableProposalId>> = BTreeSet::new();

        loop {
            // The inner block limits the lifetime of the immutable borrow(s) on forest; so that we
            // can then borrow the forest as immutable when a proposal gets ratified.
            let ratified: Option<(Rc<ComparableProposalId>, ProposalEnum)> = {
                let Some((id, (proposal, _))) = compass.next(&forest) else {
                    break;
                };

                Self::new_ratify_span(&id, proposal).in_scope(|| {
                    if self.is_accepted_by_everyone(&id, proposal, &self.stake_distribution)
                        && self.is_still_valid(proposal)
                    {
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
                self.enact_proposal(
                    id,
                    proposal,
                    &mut forest,
                    &mut compass,
                    &mut pruned_proposals,
                    &mut store_updates,
                )?;
            }
        }

        // Ensures that the treasury is properly depleted, if necessary.
        let total_withdrawn = self.treasury - forest.treasury();
        if total_withdrawn > 0 {
            store_updates.push(Box::new(move |db, _ctx| {
                db.with_pots(|mut pots| {
                    pots.borrow_mut().treasury -= total_withdrawn;
                })
            }));
        }

        // Finally, replace the roots in the store. Note that this is pretty much a no-op when no
        // proposals are ratified; but it's just one db key/value update.
        let new_roots = forest.roots();
        info_roots(&new_roots);
        store_updates.push(Box::new(move |db, _ctx| db.set_proposals_roots(&new_roots)));

        Ok(RatificationResult {
            context: self,
            store_updates,
            pruned_proposals,
        })
    }

    /// Apply the effect of a (now-approved) proposal to both the current state (i.e. self), and
    /// the persistent store. Note that updates to the store are actually happening *after*, once
    /// the ratification procedure is fully done. So we only stash updates, in order (as some
    /// updates may override some previous update in the same ratification).
    fn enact_proposal<'store, S: TransactionalContext<'store>>(
        &mut self,
        id: Rc<ComparableProposalId>,
        proposal: ProposalEnum,
        forest: &mut ProposalsForest,
        compass: &mut ProposalsForestCompass,
        pruned_proposals: &mut BTreeSet<Rc<ComparableProposalId>>,
        store_updates: &mut Vec<StoreUpdate<'distr, S>>,
    ) -> Result<(), RatificationInternalError> {
        Self::new_enact_span(&id, &proposal).in_scope(
            || -> Result<(), RatificationInternalError> {
                let mut now_obsolete = forest.enact(id, &proposal, compass)?;

                tracing::Span::current().record(
                    "proposals.pruned",
                    now_obsolete
                        .iter()
                        .map(|id| id.to_compact_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                );

                pruned_proposals.append(&mut now_obsolete);

                match proposal {
                    ProposalEnum::ProtocolParameters(params_update, _parent) => {
                        self.protocol_parameters.update(params_update);
                        store_updates.push(Box::new(|db, ctx| {
                            db.set_protocol_parameters(&ctx.protocol_parameters)
                        }));
                    }

                    ProposalEnum::HardFork(protocol_version, _parent) => {
                        self.protocol_parameters.protocol_version = protocol_version;
                        store_updates.push(Box::new(|db, ctx| {
                            db.set_protocol_parameters(&ctx.protocol_parameters)
                        }));
                    }

                    ProposalEnum::ConstitutionalCommittee(
                        CommitteeUpdate::NoConfidence,
                        _parent,
                    ) => {
                        self.constitutional_committee = None;
                        store_updates.push(Box::new(|db, _ctx| {
                            db.set_constitutional_committee(
                                &amaru_kernel::ConstitutionalCommittee::NoConfidence,
                            )
                        }))
                    }

                    ProposalEnum::ConstitutionalCommittee(
                        CommitteeUpdate::ChangeMembers {
                            removed,
                            added,
                            threshold,
                        },
                        _parent,
                    ) => {
                        let committee = amaru_kernel::ConstitutionalCommittee::Trusted {
                            threshold: UnitInterval {
                                numerator: threshold.numer().try_into().unwrap_or_else(|e| {
                                    unreachable!("threshold numerator larger than u64?!: {e}")
                                }),
                                denominator: threshold.denom().try_into().unwrap_or_else(|e| {
                                    unreachable!("threshold numerator larger than u64?!: {e}")
                                }),
                            },
                        };

                        let added_as_inactive = added
                            .into_iter()
                            .map(|(cold_cred, valid_until)| (cold_cred, (None, valid_until)))
                            .collect();

                        if let Some(committee) = &mut self.constitutional_committee {
                            committee.update(threshold, added_as_inactive, removed);
                        } else {
                            self.constitutional_committee =
                                Some(ConstitutionalCommittee::new(threshold, added_as_inactive));
                        }

                        store_updates.push(Box::new(move |db, _ctx| {
                            db.set_constitutional_committee(&committee)
                        }))
                    }

                    ProposalEnum::Constitution(constitution, _parent) => store_updates
                        .push(Box::new(move |db, _ctx| db.set_constitution(&constitution))),

                    ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal(withdrawals)) => {
                        store_updates.push(Box::new(move |db, _ctx| {
                            let leftovers = withdrawals
                                .into_iter()
                                .try_fold::<_, _, Result<_, StoreError>>(
                                    0,
                                    |leftovers, (account, magic_internet_money)| {
                                        Ok(leftovers + db.refund(&account, magic_internet_money)?)
                                    },
                                )?;

                            if leftovers > 0 {
                                info!(%leftovers, "withdrawn into treasury");
                                db.with_pots(|mut pots| pots.borrow_mut().treasury += leftovers)?;
                            }

                            Ok(())
                        }));
                    }

                    ProposalEnum::Orphan(OrphanProposal::NicePoll) => {
                        unreachable!("enacted an un-enactable proposal kind ?!")
                    }
                }

                Ok(())
            },
        )
    }

    fn new_enact_span(id: &ComparableProposalId, proposal: &ProposalEnum) -> Span {
        info_span!(
            "enacting",
            "proposal.id" = id.to_compact_string(),
            "proposal.kind" = proposal.display_kind(),
            "proposals.pruned" = field::Empty,
        )
    }

    fn new_ratify_span(id: &ComparableProposalId, proposal: &ProposalEnum) -> Span {
        if matches!(proposal, ProposalEnum::Orphan(OrphanProposal::NicePoll)) {
            trace_span!(
                "ratifying",
                "proposal.id" = id.to_compact_string(),
                "proposal.kind" = proposal.display_kind(),
                "required_threshold.committee" = field::Empty,
                "votes.committee.yes" = field::Empty,
                "votes.committee.no" = field::Empty,
                "votes.committee.abstain" = field::Empty,
                "approved.committee" = field::Empty,
                "required_threshold.pools" = field::Empty,
                "votes.pools.yes" = field::Empty,
                "votes.pools.no" = field::Empty,
                "votes.pools.abstain" = field::Empty,
                "approved.pools" = field::Empty,
                "required_threshold.dreps" = field::Empty,
                "votes.dreps.yes" = field::Empty,
                "votes.dreps.no" = field::Empty,
                "votes.dreps.abstain" = field::Empty,
                "approved.dreps" = field::Empty,
            )
        } else {
            debug_span!(
                "ratifying",
                "proposal.id" = id.to_compact_string(),
                "proposal.kind" = proposal.display_kind(),
                "required_threshold.committee" = field::Empty,
                "votes.committee.yes" = field::Empty,
                "votes.committee.no" = field::Empty,
                "votes.committee.abstain" = field::Empty,
                "approved.committee" = field::Empty,
                "required_threshold.pools" = field::Empty,
                "votes.pools.yes" = field::Empty,
                "votes.pools.no" = field::Empty,
                "votes.pools.abstain" = field::Empty,
                "approved.pools" = field::Empty,
                "required_threshold.dreps" = field::Empty,
                "votes.dreps.yes" = field::Empty,
                "votes.dreps.no" = field::Empty,
                "votes.dreps.abstain" = field::Empty,
                "approved.dreps" = field::Empty,
            )
        }
    }

    /// There are additional checks we should perform at the moment of ratification
    ///
    /// - On treasury withdrawals, we must ensure there's still enough money in the treasury.
    ///   This is necessary since there can be an arbitrary number of withdrawals that have been
    ///   ratified and enacted just before; possibly depleting the treasury.
    ///
    /// - On constitutional committee updates, we should ensure that any term limit is still
    ///   valid. This can happen if a protocol parameter change that changes the max term limit
    ///   is ratified *before* a committee update, possibly rendering it invalid.
    ///
    /// Note that either way, it doesn't *invalidate* proposals, since time and subsequent
    /// proposals may turn the tide again. They should simply be skipped, and revisited at the
    /// next epoch boundary.
    fn is_still_valid(&self, proposal: &ProposalEnum) -> bool {
        match proposal {
            ProposalEnum::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, _)
            | ProposalEnum::HardFork(..)
            | ProposalEnum::Constitution(..)
            | ProposalEnum::ProtocolParameters(..)
            | ProposalEnum::Orphan(..) => true,

            ProposalEnum::ConstitutionalCommittee(
                CommitteeUpdate::ChangeMembers { added, .. },
                _,
            ) => added.values().all(|valid_until| {
                valid_until <= &(self.epoch + self.protocol_parameters.max_committee_term_length)
            }),
        }
    }

    fn is_accepted_by_everyone(
        &self,
        id: &ComparableProposalId,
        proposal: &ProposalEnum,
        stake_distribution: &StakeDistribution,
    ) -> bool {
        let span = tracing::Span::current();

        let empty = BTreeMap::new();

        let (dreps_votes, cc_votes, pool_votes) =
            partition_votes(self.votes.get(id).unwrap_or(&empty));

        let cc_approved = self.is_accepted_by_constitutional_committee(proposal, cc_votes);
        span.record("approved.committee", cc_approved);

        let spos_approved =
            self.is_accepted_by_stake_pool_operators(proposal, pool_votes, stake_distribution);
        span.record("approved.pools", spos_approved);

        let dreps_approved =
            self.is_accepted_by_delegate_representatives(proposal, dreps_votes, stake_distribution);
        span.record("approved.dreps", dreps_approved);

        cc_approved && spos_approved && dreps_approved
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

                tracing::Span::current()
                    .record("required_threshold.committee", field::display(&threshold));

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
                tracing::Span::current()
                    .record("required_threshold.pools", field::display(&threshold));

                let tally = || {
                    stake_pools::tally(
                        self.protocol_parameters.protocol_version,
                        proposal,
                        votes,
                        stake_distribution,
                    )
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
                tracing::Span::current()
                    .record("required_threshold.dreps", field::display(&threshold));

                let tally = || -> SafeRatio {
                    dreps::tally(self.epoch, proposal, votes, stake_distribution)
                };

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
    votes: &BTreeMap<Voter, Ballot>,
) -> (
    BTreeMap<DRep, &Vote>,
    BTreeMap<StakeCredential, &Vote>,
    BTreeMap<&PoolId, &Vote>,
) {
    votes.iter().fold(
        (BTreeMap::new(), BTreeMap::new(), BTreeMap::new()),
        |(mut dreps, mut committee, mut pools), (voter, ballot)| {
            match voter {
                Voter::ConstitutionalCommitteeKey(hash) => {
                    committee.insert(StakeCredential::AddrKeyhash(*hash), &ballot.vote);
                }
                Voter::ConstitutionalCommitteeScript(hash) => {
                    committee.insert(StakeCredential::ScriptHash(*hash), &ballot.vote);
                }
                Voter::DRepKey(hash) => {
                    dreps.insert(DRep::Key(*hash), &ballot.vote);
                }
                Voter::DRepScript(hash) => {
                    dreps.insert(DRep::Script(*hash), &ballot.vote);
                }
                Voter::StakePoolKey(pool_id) => {
                    pools.insert(pool_id, &ballot.vote);
                }
            };

            (dreps, committee, pools)
        },
    )
}

fn info_roots(roots: &ProposalsRootsRc) {
    fn opt_root(opt: Option<&ComparableProposalId>) -> String {
        opt.map(|r| r.to_compact_string())
            .unwrap_or_else(|| "none".to_string())
    }

    info!(
        "roots.protocol_parameters" = opt_root(roots.protocol_parameters.as_deref()),
        "roots.hard_fork" = opt_root(roots.hard_fork.as_deref()),
        "roots.constitutional_committee" = opt_root(roots.constitutional_committee.as_deref()),
        "roots.constitution" = opt_root(roots.constitution.as_deref()),
    );
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(any(all(test, not(target_os = "windows")), feature = "test-utils"))]
pub mod tests {
    use amaru_kernel::{Bound, Epoch, EraHistory, EraParams, Slot, Summary};
    use std::sync::LazyLock;

    pub use super::proposal_enum::tests::*;

    // Technically higher than the actual gap we may see in 'real life', but, why not.
    pub const MAX_ARBITRARY_EPOCH: u64 = 10;
    pub const MIN_ARBITRARY_EPOCH: u64 = 0;

    pub static ERA_HISTORY: LazyLock<EraHistory> = LazyLock::new(|| {
        EraHistory::new(
            &[Summary {
                start: Bound {
                    time_ms: 0,
                    slot: Slot::from(0),
                    epoch: Epoch::from(0),
                },
                end: None,
                params: EraParams {
                    // Pick an epoch length such that epochs falls within the min and max bounds;
                    // knowing that slots ranges across all u64.
                    epoch_size_slots: u64::MAX / (MAX_ARBITRARY_EPOCH - MIN_ARBITRARY_EPOCH + 1),
                    slot_length: 1,
                },
            }],
            Slot::from(0),
        )
    });

    #[cfg(all(test, not(target_os = "windows")))]
    mod internal {
        use super::*;
        use amaru_kernel::tests::any_proposal_pointer;
        use proptest::prelude::*;

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
            #[test]
            #[should_panic]
            fn prop_proposal_pointer_sometimes_min_epoch(pointer in any_proposal_pointer(u64::MAX)) {
                let epoch = ERA_HISTORY.slot_to_epoch(pointer.slot(), pointer.slot()).unwrap();
                prop_assert!(epoch != Epoch::from(MIN_ARBITRARY_EPOCH));
            }
        }

        proptest! {
            #[test]
            #[should_panic]
            fn prop_proposal_pointer_sometimes_max_epoch(pointer in any_proposal_pointer(u64::MAX)) {
                let epoch = ERA_HISTORY.slot_to_epoch(pointer.slot(), pointer.slot()).unwrap();
                prop_assert!(epoch != Epoch::from(MAX_ARBITRARY_EPOCH));
            }
        }
    }
}
