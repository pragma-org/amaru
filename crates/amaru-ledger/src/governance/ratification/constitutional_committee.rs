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

use super::{OrphanProposal, ProposalEnum};
use amaru_kernel::{
    Epoch, ProtocolVersion, RationalNumber, StakeCredential, Vote, PROTOCOL_VERSION_9,
};
use num::Rational64;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug)]
pub struct ConstitutionalCommittee {
    /// Threshold (i.e. ratio of yes over no votes) necessary to reach agreement.
    pub threshold: Rational64,

    /// Members cold key hashes mapped to their hot key credential, if any and their expiry epoch.
    pub members: BTreeMap<StakeCredential, (Option<StakeCredential>, Epoch)>,
}

impl ConstitutionalCommittee {
    pub fn new(
        threshold: RationalNumber,
        members: BTreeMap<StakeCredential, (Option<StakeCredential>, Epoch)>,
    ) -> Self {
        Self {
            threshold: Rational64::new(threshold.numerator as i64, threshold.denominator as i64),
            members,
        }
    }

    /// Get the subset of cc member that is current active, as per the current epoch. A member is active iif:
    ///
    /// - it exists
    /// - its cold key is delegated to a hot key
    /// - it hasn't expired yet
    pub fn active_members<'a>(
        &'a self,
        // The epoch that just ended.
        current_epoch: Epoch,
        // A selector for accessing either the cold or hot credential.
        select: impl Fn(&'a StakeCredential, &'a StakeCredential) -> &'a StakeCredential,
    ) -> BTreeSet<&'a StakeCredential> {
        self.members
            .iter()
            .filter_map(|(cold_cred, (hot_cred, valid_until))| {
                if valid_until >= &current_epoch {
                    Some(select(cold_cred, hot_cred.as_ref()?))
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn voting_threshold(
        &self,
        current_epoch: Epoch,
        protocol_version: ProtocolVersion,
        min_committee_size: usize,
        proposal: &ProposalEnum,
    ) -> Option<&Rational64> {
        match proposal {
            ProposalEnum::ConstitutionalCommittee(..)
            | ProposalEnum::Orphan(OrphanProposal::NicePoll) => Some(&Rational64::ZERO),

            ProposalEnum::ProtocolParameters(..)
            | ProposalEnum::HardFork(..)
            | ProposalEnum::Constitution(..)
            | ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => {
                let active_members = self.active_members(current_epoch, |cold_cred, _| cold_cred);

                // The minimum committee size has no effect during the bootstrap phase (i.e. v9). The
                // committee is always allowed to vote during v9.
                if active_members.len() < min_committee_size
                    && protocol_version > PROTOCOL_VERSION_9
                {
                    return None;
                }

                Some(&self.threshold)
            }
        }
    }

    /// Count the ratio of yes votes amongst the active cc members.
    ///
    /// - Members that do not vote will count as a default "no" (i.e. increases the denominator);
    /// - Members that expired are excluded entirely (also from the denominator);
    /// - Members that have resigned (i.e. no hot keys) are also excluded;
    pub fn tally(&self, epoch: Epoch, votes: BTreeMap<StakeCredential, &Vote>) -> Rational64 {
        // TODO: Avoid re-computing this on each tally? The set of active members is fixed per
        // epoch.
        let active_members = self.active_members(epoch, |_, hot_cred| hot_cred);

        let total_active_members = active_members.len() as i64;

        let (yes, no, abstain) =
            votes
                .iter()
                .fold((0, 0, 0), |(yes, no, abstain), (hot_cred, vote)| {
                    if active_members.contains(hot_cred) {
                        match vote {
                            Vote::Yes => (yes + 1, no, abstain),
                            Vote::No => (yes, no + 1, abstain),
                            Vote::Abstain => (yes, no, abstain + 1),
                        }
                    } else {
                        (yes, no, abstain)
                    }
                });

        let span = tracing::Span::current();
        span.record("votes.committee.yes", yes);
        span.record("votes.committee.no", no);
        span.record("votes.committee.abstain", abstain);

        if abstain >= total_active_members {
            Rational64::ZERO
        } else {
            Rational64::new(yes, total_active_members - abstain)
        }
    }
}
