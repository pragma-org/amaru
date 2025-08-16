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
    store::{columns::proposals, StoreError, TransactionalContext},
    summary::{stake_distribution::StakeDistribution, SafeRatio},
};
use amaru_kernel::{
    protocol_parameters::ProtocolParameters, Ballot, ComparableProposalId, Constitution, Epoch,
    Lovelace, PoolId, ProtocolParamUpdate, ProtocolVersion, StakeCredential, UnitInterval, Vote,
    Voter,
};
use num::Zero;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::{debug_span, field, info, info_span, trace_span, Span};

mod constitutional_committee;
pub use constitutional_committee::ConstitutionalCommittee;

mod stake_pools;

mod proposals_forest;
use proposals_forest::{ProposalsEnactError, ProposalsForest, ProposalsInsertError};

mod proposals_roots;
pub use proposals_roots::{ProposalsRoots, ProposalsRootsRc};

mod proposals_tree;

// Top-level logic
// ----------------------------------------------------------------------------

/// All informations needed to ratify votes.
pub struct RatificationContext {
    /// The epoch that just ended.
    pub epoch: Epoch,

    /// The protocol version at the moment the epoch ended.
    pub protocol_version: ProtocolVersion,

    /// The *current* (live! not from the stake distr snapshot) value of the treasury
    pub treasury: Lovelace,

    /// The total amount of Lovelace withdrawn during this ratification. This is used to track the
    /// amount in-between withdrawals and ensures we down underflow the treasury.
    pub total_withdrawn: Lovelace,

    /// The computed stake distribution for the epoch
    pub stake_distributions: Arc<Mutex<VecDeque<StakeDistribution>>>,

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
    #[error("failed to acquire stake distribution shared lock")]
    FailedToAcquireStakeDistrLock,

    #[error("no suitable stake distribution for the ratification")]
    NoSuitableStakeDistribution,

    #[error("invalid operation while creating the proposals forest: {0}")]
    InternalForestCreationError(#[from] ProposalsInsertError<ComparableProposalId>),

    #[error("invalid operation while enacting a proposal: {0}")]
    InternalForestEnactmentError(#[from] ProposalsEnactError<ComparableProposalId>),
}

pub struct RatificationResult<S> {
    pub context: RatificationContext,
    pub store_updates: Vec<StoreUpdate<S>>,
    pub pruned_proposals: BTreeSet<Rc<ComparableProposalId>>,
}

impl RatificationContext {
    pub fn ratify_proposals<'store, S: TransactionalContext<'store>>(
        mut self,
        mut proposals: Vec<(ComparableProposalId, proposals::Row)>,
        roots: ProposalsRootsRc,
    ) -> Result<RatificationResult<S>, RatificationInternalError> {
        proposals.sort_by(|a, b| a.1.proposed_in.cmp(&b.1.proposed_in));

        info_roots(&roots);

        let mut forest = proposals
            .drain(..)
            .try_fold::<_, _, Result<_, RatificationInternalError>>(
                ProposalsForest::new(&roots),
                |mut forest, (id, row)| {
                    forest.insert(id, row.proposal.gov_action)?;
                    Ok(forest)
                },
            )?;

        let stake_distributions = self.stake_distributions.clone();

        let stake_distributions = stake_distributions
            .lock()
            .map_err(|_| RatificationInternalError::FailedToAcquireStakeDistrLock)?;

        let stake_distribution = stake_distributions
            .iter()
            .find(|stake_distribution| stake_distribution.epoch == self.epoch)
            .ok_or(RatificationInternalError::NoSuitableStakeDistribution)?;

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
        let mut compass = forest.new_compass();

        // We collect updates to be done on the store while enacting proposals. The updates aren't
        // executed, but stashed for later. This allows processing proposals in a pure fasion,
        // while accumulating changes to be done on the store.
        let mut store_updates: Vec<StoreUpdate<S>> = Vec::new();

        // We also accumulate proposals that get pruned due to other proposals being enacted. Those
        // proposals are obsolete, and must be removed from the database.
        let mut pruned_proposals: BTreeSet<Rc<ComparableProposalId>> = BTreeSet::new();

        loop {
            // The inner block limits the lifetime of the immutable borrow(s) on forest; so that we
            // can then borrow the forest as immutable when a proposal gets ratified.
            let ratified: Option<(Rc<ComparableProposalId>, ProposalEnum)> = {
                let Some((id, proposal)) = compass.next(&forest) else {
                    break;
                };

                Self::new_ratify_span(&id, proposal).in_scope(|| {
                    if self.is_accepted_by_everyone(&id, proposal, stake_distribution)
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
        if self.total_withdrawn > 0 {
            store_updates.push(Box::new(move |db, _ctx| {
                db.with_pots(|mut pots| {
                    pots.borrow_mut().treasury -= self.total_withdrawn;
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
        store_updates: &mut Vec<StoreUpdate<S>>,
    ) -> Result<(), RatificationInternalError> {
        Self::new_enact_span(&id, &proposal).in_scope(
            || -> Result<(), RatificationInternalError> {
                let now_obsolete = &mut forest.enact(id, &proposal, compass)?;

                tracing::Span::current().record(
                    "proposals.pruned",
                    field::valuable(
                        &now_obsolete
                            .iter()
                            .map(|id| id.to_compact_string())
                            .collect::<Vec<_>>(),
                    ),
                );

                pruned_proposals.append(now_obsolete);

                match proposal {
                    ProposalEnum::ProtocolParameters(params_update, _parent) => {
                        self.protocol_parameters.update(params_update);
                        store_updates.push(Box::new(|db, ctx| {
                            db.set_protocol_parameters(&ctx.protocol_parameters)
                        }));
                    }

                    ProposalEnum::HardFork(protocol_version, _parent) => {
                        self.protocol_version = protocol_version;
                        self.protocol_parameters.protocol_version = protocol_version;
                        store_updates.push(Box::new(|db, ctx| {
                            db.set_protocol_version(&ctx.protocol_version)?;
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

                        if let Some(committee) = &mut self.constitutional_committee {
                            committee.threshold = threshold;
                            committee.members.retain(|k, _| !removed.contains(k));
                            for (cold_cred, valid_until) in added.into_iter() {
                                committee.members.insert(cold_cred, (None, valid_until));
                            }
                        } else {
                            self.constitutional_committee = Some(ConstitutionalCommittee {
                                threshold,
                                members: added
                                    .into_iter()
                                    .map(|(cold_cred, valid_until)| {
                                        (cold_cred, (None, valid_until))
                                    })
                                    .collect(),
                            });
                        }

                        store_updates.push(Box::new(move |db, _ctx| {
                            db.set_constitutional_committee(&committee)
                        }))
                    }

                    ProposalEnum::Constitution(constitution, _parent) => store_updates
                        .push(Box::new(move |db, _ctx| db.set_constitution(&constitution))),

                    ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal(withdrawals)) => {
                        withdrawals.iter().for_each(|(_, magic_internet_money)| {
                            self.total_withdrawn += magic_internet_money;
                        });

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
            | ProposalEnum::Orphan(OrphanProposal::NicePoll) => true,

            ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal(withdrawals)) => {
                self.total_withdrawn + withdrawals.values().sum::<Lovelace>() <= self.treasury
            }

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

        let (_dreps_votes, cc_votes, pool_votes) =
            partition_votes(self.votes.get(id).unwrap_or(&empty));

        let cc_approved = self.is_accepted_by_constitutional_committee(proposal, cc_votes);
        span.record("approved.committee", cc_approved);

        let spos_approved =
            self.is_accepted_by_stake_pool_operators(proposal, pool_votes, stake_distribution);
        span.record("approved.pools", spos_approved);

        let dreps_approved = self.is_accepted_by_delegate_representatives(proposal);
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
                    self.protocol_version,
                    self.protocol_parameters.min_committee_size,
                    proposal,
                )?;

                tracing::Span::current()
                    .record("required_threshold.committee", threshold.to_string());

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
                tracing::Span::current().record("required_threshold.pools", threshold.to_string());

                let tally = || {
                    stake_pools::tally(self.protocol_version, proposal, votes, stake_distribution)
                };

                threshold == SafeRatio::zero() || tally() >= threshold
            }
        }
    }

    fn is_accepted_by_delegate_representatives(&self, _proposal: &ProposalEnum) -> bool {
        // FIXME
        true
    }
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
#[derive(Debug, Clone)]
pub enum ProposalEnum {
    ProtocolParameters(ProtocolParamUpdate, Option<Rc<ComparableProposalId>>),
    HardFork(ProtocolVersion, Option<Rc<ComparableProposalId>>),
    ConstitutionalCommittee(CommitteeUpdate, Option<Rc<ComparableProposalId>>),
    Constitution(Constitution, Option<Rc<ComparableProposalId>>),
    Orphan(OrphanProposal),
}

impl ProposalEnum {
    pub fn display_kind(&self) -> String {
        match self {
            Self::ProtocolParameters(..) => "protocol-parameters",
            Self::HardFork(..) => "hard-fork",
            Self::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, _) => {
                "motion-of-no-confidence"
            }
            Self::ConstitutionalCommittee(CommitteeUpdate::ChangeMembers { .. }, _) => {
                "constitutional-committee"
            }
            Self::Constitution(..) => "constitution",
            Self::Orphan(OrphanProposal::NicePoll) => "nice-poll",
            Self::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => "treasury-withdrawal",
        }
        .to_string()
    }

    pub fn is_hardfork(&self) -> bool {
        matches!(self, Self::HardFork { .. })
    }

    pub fn is_no_confidence(&self) -> bool {
        matches!(
            self,
            Self::ConstitutionalCommittee(CommitteeUpdate::NoConfidence, _)
        )
    }

    pub fn is_committee_member_update(&self) -> bool {
        matches!(
            self,
            Self::ConstitutionalCommittee(CommitteeUpdate::ChangeMembers { .. }, _)
        )
    }
}

// CommitteeUpdate
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum CommitteeUpdate {
    NoConfidence,
    ChangeMembers {
        removed: BTreeSet<StakeCredential>,
        added: BTreeMap<StakeCredential, Epoch>,
        threshold: SafeRatio,
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
                    threshold.numer(),
                    threshold.denom(),
                )
            }
        }
    }
}

// OrphanProposal
// ----------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum OrphanProposal {
    TreasuryWithdrawal(BTreeMap<StakeCredential, Lovelace>),
    NicePoll,
}

impl fmt::Display for OrphanProposal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OrphanProposal::NicePoll => write!(f, "nice poll"),
            OrphanProposal::TreasuryWithdrawal(withdrawals) => {
                let total = withdrawals
                    .iter()
                    .fold(0, |total, (_, single)| total + single)
                    / 1_000_000;
                write!(f, "withdrawal={total}₳")
            }
        }
    }
}

// Enactments
// ----------------------------------------------------------------------------

pub type StoreUpdate<S> = Box<dyn FnOnce(&S, &RatificationContext) -> Result<(), StoreError>>;

// Helpers
// ----------------------------------------------------------------------------

/// Split all the ballots into sub-maps that are specific to each voter types; so that we ease the
/// processing of each category down the line.
fn partition_votes(
    votes: &BTreeMap<Voter, Ballot>,
) -> (
    BTreeMap<StakeCredential, &Vote>,
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
                    dreps.insert(StakeCredential::AddrKeyhash(*hash), &ballot.vote);
                }
                Voter::DRepScript(hash) => {
                    dreps.insert(StakeCredential::ScriptHash(*hash), &ballot.vote);
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

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::{CommitteeUpdate, OrphanProposal, ProposalEnum};
    use crate::{store::columns::accounts::tests::any_stake_credential, summary::SafeRatio};
    use amaru_kernel::tests::{
        any_comparable_proposal_id, any_constitution, any_epoch, any_protocol_params_update,
        any_protocol_version,
    };
    use num::{BigUint, One};
    use proptest::{collection, option, prelude::*};
    use std::rc::Rc;

    pub fn any_proposal_enum() -> impl Strategy<Value = ProposalEnum> {
        let any_protocol_parameters = (
            option::of(any_comparable_proposal_id()),
            any_protocol_params_update(),
        )
            .prop_map(|(parent, params_update)| {
                ProposalEnum::ProtocolParameters(params_update, parent.map(Rc::new))
            });

        let any_hard_fork = (
            option::of(any_comparable_proposal_id()),
            any_protocol_version(),
        )
            .prop_map(|(parent, protocol_version)| {
                ProposalEnum::HardFork(protocol_version, parent.map(Rc::new))
            });

        let any_constitutional_committee = (
            option::of(any_comparable_proposal_id()),
            any_committee_update(),
        )
            .prop_map(|(parent, committee)| {
                ProposalEnum::ConstitutionalCommittee(committee, parent.map(Rc::new))
            });

        let any_constitution = (option::of(any_comparable_proposal_id()), any_constitution())
            .prop_map(|(parent, constitution)| {
                ProposalEnum::Constitution(constitution, parent.map(Rc::new))
            });

        let any_orphan = any_orphan_proposal().prop_map(ProposalEnum::Orphan);

        prop_oneof![
            any_protocol_parameters,
            any_hard_fork,
            any_constitutional_committee,
            any_constitution,
            any_orphan,
        ]
    }

    pub fn any_orphan_proposal() -> impl Strategy<Value = OrphanProposal> {
        let any_nice_poll = Just(OrphanProposal::NicePoll);

        let any_treasury_withdrawal = collection::btree_map(any_stake_credential(), 1_u64.., 1..3)
            .prop_map(OrphanProposal::TreasuryWithdrawal);

        prop_oneof![any_nice_poll, any_treasury_withdrawal]
    }

    pub fn any_committee_update() -> impl Strategy<Value = CommitteeUpdate> {
        let any_no_confidence = Just(CommitteeUpdate::NoConfidence);

        let any_change_members = (
            any::<u8>(),
            collection::btree_set(any_stake_credential(), 0..3),
            collection::btree_map(any_stake_credential(), any_epoch(), 0..3),
        )
            .prop_map(
                |(numerator, removed, added)| CommitteeUpdate::ChangeMembers {
                    removed,
                    added,
                    threshold: SafeRatio::new(BigUint::from(numerator), BigUint::one()),
                },
            );

        prop_oneof![any_no_confidence, any_change_members]
    }
}
