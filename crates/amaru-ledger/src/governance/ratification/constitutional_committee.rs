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
use crate::summary::{into_safe_ratio, safe_ratio, SafeRatio};
use amaru_kernel::{
    Epoch, ProtocolVersion, RationalNumber, StakeCredential, Vote, PROTOCOL_VERSION_9,
};
use num::Zero;
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::LazyLock,
};

static ZERO: LazyLock<SafeRatio> = LazyLock::new(|| SafeRatio::zero());

#[derive(Debug)]
pub struct ConstitutionalCommittee {
    /// Threshold (i.e. ratio of yes over no votes) necessary to reach agreement.
    pub threshold: SafeRatio,

    /// Members cold key hashes mapped to their hot key credential, if any and their expiry epoch.
    pub members: BTreeMap<StakeCredential, (Option<StakeCredential>, Epoch)>,
}

impl ConstitutionalCommittee {
    pub fn new(
        threshold: RationalNumber,
        members: BTreeMap<StakeCredential, (Option<StakeCredential>, Epoch)>,
    ) -> Self {
        Self {
            threshold: into_safe_ratio(&threshold),
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
        min_committee_size: u16,
        proposal: &ProposalEnum,
    ) -> Option<&SafeRatio> {
        match proposal {
            ProposalEnum::ConstitutionalCommittee(..)
            | ProposalEnum::Orphan(OrphanProposal::NicePoll) => Some(&ZERO),

            ProposalEnum::ProtocolParameters(..)
            | ProposalEnum::HardFork(..)
            | ProposalEnum::Constitution(..)
            | ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => {
                let active_members = self.active_members(current_epoch, |cold_cred, _| cold_cred);

                // The minimum committee size has no effect during the bootstrap phase (i.e. v9). The
                // committee is always allowed to vote during v9.
                if active_members.len() < (min_committee_size as usize)
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
    pub fn tally(&self, epoch: Epoch, votes: BTreeMap<StakeCredential, &Vote>) -> SafeRatio {
        // TODO: Avoid re-computing this on each tally? The set of active members is fixed per
        // epoch.
        let active_members = self.active_members(epoch, |_, hot_cred| hot_cred);

        let total_active_members = active_members.len() as u64;

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
            SafeRatio::zero()
        } else {
            safe_ratio(yes, total_active_members - abstain)
        }
    }
}
