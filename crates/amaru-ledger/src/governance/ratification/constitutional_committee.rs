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
use crate::summary::{SafeRatio, safe_ratio};
use amaru_kernel::{Epoch, PROTOCOL_VERSION_9, ProtocolVersion, StakeCredential, Vote};
use num::Zero;
use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::LazyLock,
};
use tracing::warn;

static ZERO: LazyLock<SafeRatio> = LazyLock::new(SafeRatio::zero);

#[derive(Debug, Clone)]
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

    /// View the current threshold for that committee.
    pub fn threshold(&self) -> &SafeRatio {
        &self.threshold
    }

    /// Obtain a set of all the hot credentials of known members, even inactive ones. Those are all
    /// potential voters (although their votes may be invalid).
    pub fn voters(&self) -> BTreeSet<&StakeCredential> {
        self.members
            .values()
            .filter_map(|(hot_credential, _)| hot_credential.as_ref())
            .collect()
    }

    /// Add & remove members from a committee, following some constitutional committee update.
    pub fn update(
        &mut self,
        threshold: SafeRatio,
        added: BTreeMap<StakeCredential, (Option<StakeCredential>, Epoch)>,
        removed: BTreeSet<&StakeCredential>,
    ) {
        self.threshold = threshold;

        // NOTE: members must be removed BEFORE new ones are added. It's possible that
        // 'added' re-add members that are removed; so we need to land on the same outcome across
        // all nodes.
        self.members.retain(|k, _| !removed.contains(k));

        for (cold_cred, (hot_cred, valid_until)) in added.into_iter() {
            self.members.insert(cold_cred, (hot_cred, valid_until));
        }

        *self.active_members.borrow_mut() = None;
    }

    /// Get the subset of cc member that is current active, as per the current epoch. A member is active iif:
    ///
    /// - it exists
    /// - its cold key is delegated to a hot key
    /// - it hasn't expired yet
    pub fn active_members(
        &self,
        current_epoch: Epoch,
    ) -> Rc<BTreeMap<StakeCredential, StakeCredential>> {
        if let Some((memoized_epoch, active_members)) = self.active_members.borrow().as_ref()
            && memoized_epoch == &current_epoch
        {
            return active_members.clone();
        }

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

        *self.active_members.borrow_mut() = Some((current_epoch, active_members.clone()));

        active_members
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
                    warn!(
                        members.active = self.active_members(current_epoch).len(),
                        min_committee_size = min_committee_size,
                        "no voting threshold because committee is too small"
                    );
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

        let hot_credentials = active_members.values().collect::<BTreeSet<_>>();

        // NOTE: do not use 'hot_credential'.len(), as there could be multiple cold credentials
        // delegated to the same hot credential. But we count cold credentials, not hot ones.
        let total_active_members = active_members.len() as u64;

        let (yes, no, abstain) =
            votes
                .iter()
                .fold((0, 0, 0), |(yes, no, abstain), (hot_cred, vote)| {
                    if hot_credentials.contains(hot_cred) {
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

// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::ConstitutionalCommittee;
    use crate::{
        governance::ratification::any_proposal_enum,
        summary::{SafeRatio, into_safe_ratio},
    };
    use amaru_kernel::{
        Epoch, Hash, PROTOCOL_VERSION_9, PROTOCOL_VERSION_10, StakeCredential, VOTE_NO, VOTE_YES,
        Vote, any_rational_number, any_stake_credential, any_vote_ref,
    };
    use num::{One, Zero};
    use proptest::{collection, prelude::*, sample, test_runner::RngSeed};
    use std::{
        collections::{BTreeMap, BTreeSet},
        rc::Rc,
    };

    const MIN_ARBITRARY_EPOCH: u64 = 10;
    const MAX_COMMITTEE_SIZE: usize = 10;

    const NULL_HASH: [u8; 28] = [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ];

    proptest! {
        #[test]
        fn prop_updating_members_invalidate_active((epoch, _, committee) in any_tally()) {
            let mut committee = (*committee).clone();

            let active_members = committee.active_members(epoch).clone();

            // If no active members, let's add one.
            if active_members.is_empty() {
                let cold_credential = StakeCredential::AddrKeyhash(Hash::from(NULL_HASH));
                let hot_credential = cold_credential.clone();
                committee.update(
                    committee.threshold().clone(),
                    BTreeMap::from([
                        (cold_credential, (Some(hot_credential), epoch))
                    ]),
                    BTreeSet::new(),

                );
            // If some active members, let's remove one.
            } else {
                let any_active_member = active_members
                    .keys()
                    // Pick a somewhat arbitrary member.
                    .nth(MAX_COMMITTEE_SIZE % active_members.len())
                    .unwrap();

                committee.update(
                    committee.threshold().clone(),
                    BTreeMap::new(),
                    BTreeSet::from([any_active_member]),
                );
            }

            let new_active_members = committee.active_members(epoch);

            prop_assert!(active_members != new_active_members);
        }
    }

    proptest! {
        #[test]
        fn prop_update_remove_then_add((epoch, _, committee) in any_tally()) {
            let mut committee = (*committee).clone();

            let active_members = committee.active_members(epoch);

            if !active_members.is_empty() {
                let any_active_member = active_members
                    .keys()
                    // Pick a somewhat arbitrary member.
                    .nth(MAX_COMMITTEE_SIZE % active_members.len())
                    .unwrap();

                let any_member_state = (None, Epoch::from(999));

                committee.update(
                    committee.threshold().clone(),
                    BTreeMap::from([(
                        any_active_member.clone(),
                        any_member_state.clone(),
                    )]),
                    BTreeSet::from([any_active_member]),
                );


                prop_assert_eq!(
                    committee.members.get(any_active_member),
                    Some(&any_member_state),
                );
            }
        }
    }

    proptest! {
        #[test]
        fn prop_tally_is_never_greater_than_1((epoch, votes, committee) in any_tally()) {
            let result = committee.tally(
                epoch,
                votes,
            );
            prop_assert!(result <= SafeRatio::one());
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { rng_seed: RngSeed::Fixed(42), ..ProptestConfig::default() })]
        #[test]
        #[should_panic]
        fn prop_tally_is_sometimes_greater_than_0((epoch, votes, committee) in any_tally()) {
            let result = committee.tally(
                epoch,
                votes,
            );
            prop_assert!(result == SafeRatio::zero());
        }
    }

    proptest! {
        #[test]
        fn prop_tally_changing_votes_to_yes_increases_result((epoch, votes, committee) in any_tally()) {
            let result_before = committee.tally(
                epoch,
                votes.clone(),
            );

            let active_members = committee.active_members(epoch);
            let active_voters = active_members.values().collect::<BTreeSet<_>>();
            let mut some_voted_no = false;

            let votes = votes.into_iter().map(|(k, v)| {
                if active_voters.contains(&k) && v == &VOTE_NO {
                    some_voted_no = true;
                }
                (k, &VOTE_YES)
            }).collect();

            let result_after = committee.tally(
                epoch,
                votes,
            );

            prop_assert!(
                result_before < result_after || !some_voted_no,
                "before = {result_before:?}\nafter =  {result_after:?}",
            );
        }
    }

    proptest! {
        #[test]
        fn prop_tally_changing_votes_to_no_decreases_result((epoch, votes, committee) in any_tally()) {
            let result_before = committee.tally(
                epoch,
                votes.clone(),
            );

            let active_members = committee.active_members(epoch);
            let active_voters = active_members.values().collect::<BTreeSet<_>>();
            let mut some_voted_yes = false;

            let votes = votes.into_iter().map(|(k, v)| {
                if active_voters.contains(&k) && v == &VOTE_YES {
                    some_voted_yes = true;
                }
                (k, &VOTE_NO)
            }).collect();

            let result_after = committee.tally(
                epoch,
                votes,
            );

            prop_assert!(
                result_before > result_after || !some_voted_yes,
                "before = {result_before:?}\nafter =  {result_after:?}",
            );
        }
    }

    proptest! {
        #![proptest_config(ProptestConfig { rng_seed: RngSeed::Fixed(42), ..ProptestConfig::default() })]
        #[test]
        #[should_panic]
        fn prop_tally_sometimes_see_inactive_members((epoch, _, committee) in any_tally()) {
            let active_members = committee.active_members(epoch);
            prop_assert_eq!(
                active_members.values().collect::<BTreeSet<_>>(),
                committee.voters(),
            );
        }
    }

    proptest! {
        #[test]
        #[should_panic]
        #[cfg(not(target_os = "windows"))]
        fn prop_min_size_has_no_effect_in_v9(
            committee in any_constitutional_committee(),
            min_committee_size in 0..MAX_COMMITTEE_SIZE,
            proposal in any_proposal_enum(),
        ) {
            let threshold_any = committee.voting_threshold(
                    Epoch::from(MIN_ARBITRARY_EPOCH + 1),
                    PROTOCOL_VERSION_9,
                    min_committee_size as u16,
                    &proposal
            );

            let threshold_min = committee.voting_threshold(
                    Epoch::from(MIN_ARBITRARY_EPOCH + 1),
                    PROTOCOL_VERSION_10,
                    0,
                    &proposal
            );

            prop_assert!(
                threshold_any != threshold_min,
                "threshold_any={threshold_any:?}\nthreshold_min={threshold_min:?}",
            );
        }
    }

    pub fn any_tally() -> impl Strategy<
        Value = (
            Epoch,
            BTreeMap<StakeCredential, &'static Vote>,
            Rc<ConstitutionalCommittee>,
        ),
    > {
        any_constitutional_committee().prop_flat_map(|committee| {
            (
                // We fix the epoch arbitrarily, but in knowledge of 'any_epoch'; so that
                // most cc are active, but some may be expired.
                Just(Epoch::from(MIN_ARBITRARY_EPOCH + 1)),
                any_votes(&committee),
                Just(Rc::new(committee)),
            )
        })
    }

    // A not-so-arbitrary generator, where epochs are *interesting* (i.e. some generated members
    // may be active, some may not).
    prop_compose! {
        fn any_epoch()(epoch in MIN_ARBITRARY_EPOCH..MIN_ARBITRARY_EPOCH + 3) -> Epoch {
            Epoch::from(epoch)
        }
    }

    prop_compose! {
        pub fn any_constitutional_committee()(
            threshold in any_rational_number(),
            members in collection::btree_map(
                any_stake_credential(),
                (
                    prop_oneof![3 => any_stake_credential().prop_map(Some), 1 => Just(None)],
                    any_epoch()
                ),
                0..MAX_COMMITTEE_SIZE,
            ),
        ) -> ConstitutionalCommittee {
            ConstitutionalCommittee::new(into_safe_ratio(&threshold), members)
        }
    }

    pub fn any_votes(
        committee: &ConstitutionalCommittee,
    ) -> impl Strategy<Value = BTreeMap<StakeCredential, &'static Vote>> + use<> {
        let potential_voters = committee.voters().into_iter().cloned().collect::<Vec<_>>();

        let upper_bound = potential_voters.len();

        let actual_voters = if upper_bound <= 1 {
            prop_oneof![Just(potential_voters), Just(Vec::new()),].boxed()
        } else {
            sample::subsequence(potential_voters, 0..upper_bound).boxed()
        };

        actual_voters
            .prop_flat_map(|voters| {
                collection::vec(any_vote_ref(), voters.len())
                    .prop_map(move |votes| voters.clone().into_iter().zip(votes))
            })
            .prop_map(|kvs| kvs.into_iter().collect())
    }
}
