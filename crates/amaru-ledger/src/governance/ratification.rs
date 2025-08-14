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

use crate::{store::columns::proposals, summary::stake_distribution::StakeDistribution};
use amaru_kernel::{
    protocol_parameters::PoolThresholds, Ballot, ComparableProposalId, Constitution, Epoch,
    Lovelace, PoolId, ProposalId, ProtocolParamUpdate, ProtocolVersion, ScriptHash,
    StakeCredential, UnitInterval, Vote, Voter,
};
use num::Rational64;
use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    rc::Rc,
    sync::{Arc, Mutex},
};
use tracing::debug;

mod constitutional_committee;
pub use constitutional_committee::ConstitutionalCommittee;

mod stake_pools;

mod proposals_forest;
use proposals_forest::ProposalsForest;

mod proposals_roots;
pub use proposals_roots::ProposalsRoots;

mod proposals_tree;

// Top-level logic
// ----------------------------------------------------------------------------

/// All informations needed to ratify votes.
pub struct RatificationContext {
    /// The epoch that just ended.
    pub epoch: Epoch,

    /// The protocol version at the moment the epoch ended.
    pub protocol_version: ProtocolVersion,

    /// The computed stake distribution for the epoch
    pub stake_distributions: Arc<Mutex<VecDeque<StakeDistribution>>>,

    /// The minimum constitutional committee's size, as per latest enacted protocol parameters.
    pub min_committee_size: usize,

    /// The stake pool voting thresholds, as per latest enacted protocol parameters.
    pub stake_pools_voting_thresholds: PoolThresholds,

    /// The current constitutional committee, if any. No committee signals a state of
    /// no-confidence.
    pub constitutional_committee: Option<ConstitutionalCommittee>,

    /// All latest votes indexed by proposals and voters.
    pub votes: BTreeMap<ComparableProposalId, BTreeMap<Voter, Ballot>>,

    /// The current roots (i.e. latest enacted proposal ids) for each of the
    /// relevant proposal categories.
    pub roots: ProposalsRoots,
}

impl RatificationContext {
    pub fn ratify_proposals(self, mut proposals: Vec<(ProposalId, proposals::Row)>) {
        proposals.sort_by(|a, b| a.1.proposed_in.cmp(&b.1.proposed_in));

        let mut forest =
            proposals
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

        debug!(
            "forest" = %forest,
            "ratifying"
        );

        let stake_distributions = self.stake_distributions.lock().unwrap();

        let stake_distribution = stake_distributions
            .iter()
            .find(|stake_distribution| stake_distribution.epoch == self.epoch)
            .unwrap_or_else(|| panic!("no stake distribution for target epoch"));

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
        let mut iterator = forest.iter();

        while let Some((id, proposal)) = guard(!delayed, || iterator.next()) {
            if !matches!(proposal, ProposalEnum::Orphan(OrphanProposal::NicePoll)) {
                debug!("proposal.id" = %id, "ratifying");
            }

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
            if self.roots.matching(proposal)
                && self.is_accepted_by_everyone(id, proposal, stake_distribution)
            {
                todo!("a proposal has been ratified, it must now be enacted!")
            }
        }
    }

    fn is_accepted_by_everyone(
        &self,
        id: &ComparableProposalId,
        proposal: &ProposalEnum,
        stake_distribution: &StakeDistribution,
    ) -> bool {
        let empty = BTreeMap::new();

        let (_dreps_votes, cc_votes, pool_votes) =
            partition_votes(self.votes.get(id).unwrap_or(&empty));

        self.is_accepted_by_constitutional_committee(proposal, cc_votes)
            && self.is_accepted_by_stake_pool_operators(proposal, pool_votes, stake_distribution)
            && self.is_accepted_by_delegate_representatives(proposal)
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
                    self.min_committee_size,
                    proposal,
                )?;

                let tally = || committee.tally(self.epoch, votes);

                Some(threshold == &Rational64::ZERO || &tally() >= threshold)
            })
            .unwrap_or(false)
    }

    fn is_accepted_by_stake_pool_operators(
        &self,
        proposal: &ProposalEnum,
        votes: BTreeMap<PoolId, &Vote>,
        stake_distribution: &StakeDistribution,
    ) -> bool {
        match stake_pools::voting_threshold(
            self.constitutional_committee.is_none(),
            &self.stake_pools_voting_thresholds,
            proposal,
        ) {
            None => false,
            Some(threshold) => {
                let tally = || {
                    stake_pools::tally(self.protocol_version, proposal, votes, stake_distribution)
                };
                threshold == Rational64::ZERO || tally() >= threshold
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
#[derive(Debug)]
pub enum ProposalEnum {
    ProtocolParameters(ProtocolParamUpdate, Option<Rc<ComparableProposalId>>),
    HardFork(ProtocolVersion, Option<Rc<ComparableProposalId>>),
    ConstitutionalCommittee(CommitteeUpdate, Option<Rc<ComparableProposalId>>),
    Constitution(Constitution, Option<Rc<ComparableProposalId>>),
    Orphan(OrphanProposal),
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

// Helpers
// ----------------------------------------------------------------------------

/// Split all the ballots into sub-maps that are specific to each voter types; so that we ease the
/// processing of each category down the line.
fn partition_votes(
    votes: &BTreeMap<Voter, Ballot>,
) -> (
    BTreeMap<StakeCredential, &Vote>,
    BTreeMap<StakeCredential, &Vote>,
    BTreeMap<PoolId, &Vote>,
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
                    pools.insert(*pool_id, &ballot.vote);
                }
            };

            (dreps, committee, pools)
        },
    )
}

/// Execute the guarded action if the predicate is `true`; returns `None` otherwise.
fn guard<A>(predicate: bool, mut action: impl FnMut() -> Option<A>) -> Option<A> {
    if predicate {
        action()
    } else {
        None
    }
}
