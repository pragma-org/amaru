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
use crate::summary::{safe_ratio, SafeRatio};
use amaru_kernel::{Epoch, ProtocolVersion, StakeCredential, Vote, PROTOCOL_VERSION_9};
use num::Zero;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::LazyLock,
};

static ZERO: LazyLock<SafeRatio> = LazyLock::new(SafeRatio::zero);

#[derive(Debug)]
pub struct ConstitutionalCommittee {
    /// Threshold (i.e. ratio of yes over no votes) necessary to reach agreement.
    threshold: SafeRatio,

    /// Members cold key hashes mapped to their hot key credential, if any and their expiry epoch.
    members: BTreeMap<StakeCredential, (Option<StakeCredential>, Epoch)>,

    /// Active members for a given ongoing epoch. We memoized the result to avoid re-computing it
    /// over and over.
    active_members: RefCell<Option<(Epoch, Rc<BTreeMap<StakeCredential, StakeCredential>>)>>,
}

impl ConstitutionalCommittee {
    pub fn new(
        threshold: SafeRatio,
        members: BTreeMap<StakeCredential, (Option<StakeCredential>, Epoch)>,
    ) -> Self {
        Self {
            threshold,
            members,
            active_members: RefCell::new(None),
        }
    }

    /// Add & remove members from a committee, following some constitutional committee update.
    pub fn update(
        &mut self,
        threshold: SafeRatio,
        added: BTreeMap<StakeCredential, Epoch>,
        removed: BTreeSet<StakeCredential>,
    ) {
        self.threshold = threshold;
        // NOTE: members must be removed BEFORE new ones are added. It's possible that
        // 'added' re-add members that are removed; so we need to land on the same outcome across
        // all nodes.
        self.members.retain(|k, _| !removed.contains(k));
        for (cold_cred, valid_until) in added.into_iter() {
            self.members.insert(cold_cred, (None, valid_until));
        }
        *self.active_members.borrow_mut() = None;
    }

    /// Get the subset of cc member that is current active, as per the current epoch. A member is active iif:
    ///
    /// - it exists
    /// - its cold key is delegated to a hot key
    /// - it hasn't expired yet
    fn active_members(
        &self,
        current_epoch: Epoch,
    ) -> Rc<BTreeMap<StakeCredential, StakeCredential>> {
        match self.active_members.borrow().as_ref() {
            Some((memoized_epoch, active_members)) if memoized_epoch == &current_epoch => {
                active_members.clone()
            }
            Some(..) | None => {
                let active_members = Rc::new(
                    self.members
                        .iter()
                        .filter_map(|(cold_cred, (hot_cred, valid_until))| {
                            if valid_until >= &current_epoch {
                                Some((cold_cred.clone(), hot_cred.as_ref()?.clone()))
                            } else {
                                None
                            }
                        })
                        .collect::<BTreeMap<_, _>>(),
                );

                let rc = active_members.clone();

                *self.active_members.borrow_mut() = Some((current_epoch, active_members));

                rc
            }
        }
    }

    pub fn voting_threshold(
        &self,
        current_epoch: Epoch,
        protocol_version: ProtocolVersion,
        min_committee_size: u16,
        proposal: &ProposalEnum,
    ) -> Option<&SafeRatio> {
        match proposal {
            ProposalEnum::Orphan(OrphanProposal::NicePoll) => None,

            ProposalEnum::ConstitutionalCommittee(..) => Some(&ZERO),

            ProposalEnum::ProtocolParameters(..)
            | ProposalEnum::HardFork(..)
            | ProposalEnum::Constitution(..)
            | ProposalEnum::Orphan(OrphanProposal::TreasuryWithdrawal { .. }) => {
                // The minimum committee size has no effect during the bootstrap phase (i.e. v9). The
                // committee is always allowed to vote during v9.
                if self.active_members(current_epoch).len() < (min_committee_size as usize)
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
        let active_members = self.active_members(epoch);

        let total_active_members = active_members.len() as u64;

        let (yes, no, abstain) =
            votes
                .iter()
                .fold((0, 0, 0), |(yes, no, abstain), (hot_cred, vote)| {
                    if active_members.contains_key(hot_cred) {
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
