// Copyright 2026 PRAGMA
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

use std::{borrow::Cow, fmt, sync::Arc};

use amaru_kernel::{
    CertificatePointer, Epoch, MAINNET_DEFAULT_PROTOCOL_PARAMETERS, Pots, ProposalPointer, StakeCredential,
};
use amaru_ledger::{
    context::PreparationContext,
    epoch_transition::GovernanceActivity,
    state::volatile::{AnchoredVolatileFragment, VolatileDB, VolatileFragment, VolatileSequence},
    store::{self, ReadStore},
};
use rand::Rng;

use crate::common::{fixture, scale::BenchScale};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Scenario {
    Utxo,
    Pools,
    Accounts,
    Withdrawals,
    Committee,
    DReps,
    Proposals,
    Votes,
    Mixed,
}

impl Scenario {
    // Cycle through the non-mixed scenarios in a round-robin fashion.
    pub fn round_robin() -> &'static [Self] {
        &[
            Self::Utxo,
            Self::Pools,
            Self::Withdrawals,
            Self::Accounts,
            Self::Committee,
            Self::DReps,
            Self::Proposals,
            Self::Votes,
        ]
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Utxo => "utxo",
            Self::Pools => "pools",
            Self::Accounts => "accounts",
            Self::Withdrawals => "withdrawals",
            Self::Committee => "committee",
            Self::DReps => "dreps",
            Self::Proposals => "proposals",
            Self::Votes => "votes",
            Self::Mixed => "mixed",
        }
    }

    pub const fn seed(self) -> u64 {
        match self {
            Self::Utxo => 0xA11C_E001,
            Self::Pools => 0xA11C_E002,
            Self::Accounts => 0xA11C_E003,
            Self::Withdrawals => 0xA11C_E004,
            Self::Committee => 0xA11C_E005,
            Self::DReps => 0xA11C_E006,
            Self::Proposals => 0xA11C_E007,
            Self::Votes => 0xA11C_E008,
            Self::Mixed => 0xA11C_E009,
        }
    }

    pub fn fmt(self, f: &mut fmt::Formatter<'_>, scale: &BenchScale) -> fmt::Result {
        let per_item = self.per_item_size();
        write!(
            f,
            "{} [block_size={}, volatile_size={}, item_size={}, total_items={}]",
            self.name(),
            scale.block_size,
            scale.volatile_size,
            per_item,
            scale.block_size * scale.volatile_size / per_item,
        )
    }

    pub fn mock_store(self) -> impl ReadStore {
        struct MockStore(Scenario);

        impl ReadStore for MockStore {
            // The context will always reach to the stable store for committee members, since the
            // volatile can only know they're gone after a an epoch transition, while everything
            // else requires a stable store view.
            fn cc_member(
                &self,
                credential: &StakeCredential,
            ) -> store::Result<Option<store::columns::cc_members::Row>> {
                match self.0 {
                    Scenario::Committee | Scenario::Mixed => Ok(None),
                    Scenario::Utxo
                    | Scenario::Pools
                    | Scenario::Accounts
                    | Scenario::Withdrawals
                    | Scenario::DReps
                    | Scenario::Proposals
                    | Scenario::Votes => unimplemented!("ReadStore.cc_member({credential:?}"),
                }
            }

            // The context will always reach to the stable store for withdrawals, since they are marked as
            // required but we have no information on the account.
            fn account(&self, credential: &StakeCredential) -> store::Result<Option<store::columns::accounts::Row>> {
                match self.0 {
                    Scenario::Withdrawals | Scenario::Mixed => Ok(None),
                    Scenario::Utxo
                    | Scenario::Pools
                    | Scenario::Accounts
                    | Scenario::Committee
                    | Scenario::DReps
                    | Scenario::Proposals
                    | Scenario::Votes => unimplemented!("ReadStore.account({credential:?}"),
                }
            }

            // Every scenario reaches the stable store for the pots, since resolving the treasury at
            // the tip folds the pending boundary delta on top of the stable value.
            fn pots(&self) -> store::Result<Pots> {
                Ok(Pots::default())
            }
        }

        MockStore(self)
    }

    pub fn per_item_size(self) -> usize {
        match self {
            // Considering a "worse" case UTxO with one input = transaction id + (small) index, and
            // one output that's just an address with no delegation and an minimum ada amount. The
            // size we consider is the average between consumed and produced inputs.
            Self::Utxo => 55,

            // Some average between pool registrations and pool de-registrations, taking into
            // account the impact of pool deposits as well.
            Self::Pools => 100,

            // For accounts, we take the average between all cases:
            //
            // - registration only (~35 bytes)
            // - registration and pool delegation (~65 bytes)
            // - registration and drep delegation (~65 bytes)
            // - registration and both delegation (~95 bytes)
            // - de-registration (~35 bytes)
            //
            // Authentication is required, but can be squeezed into tiny native scripts that take
            // little space.
            Self::Accounts => 65,

            // While we only store a stake credential, withdrawals do need an amount as well. In the
            // worse case, they also require a witness that's just a handful of bytes; with no
            // signature whatsoever.
            Self::Withdrawals => 40,

            // Committee member is either delegating to hot (2 credentials), or resigning (one
            // credential). So we take the average.
            Self::Committee => 45,

            // DReps have a 500 deposits, which is what becomes the bottleneck. The actual item size
            // is only around 35 bytes, but if you consider this size, for k=2160 and a block size
            // of 90KB, that'd be about 5.5M dreps, totaling nearly 3B ADA in deposit.
            //
            // So we use a size of ~100 which would still require around 1B ADA to pull out.
            Self::DReps => 100,

            // Proposals have a significant deposit associated with them. So if we remains
            // consistent with the DRep upper-bound of 1B ADA, that makes a size of 20K for a
            // proposal. However, this would greatly skew the mixed scenario; so we pick something
            // lower, even though it's a pessimistic upper bound that can never be reached in
            // practice.
            Self::Proposals => 2_000,

            // A vote contains a voter, the vote itself and the governance action it targets. Plus
            // the overhead.
            Self::Votes => 70,

            Self::Mixed => {
                let all = Self::round_robin();
                let len = all.len();
                all.iter().map(|entity| entity.per_item_size()).sum::<usize>() / len
            }
        }
    }

    /// Populate a fragment with random content corresponding to the scenario; up to its size.
    pub fn mut_fragment(self, fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, max_size: usize) {
        match self {
            Self::Utxo => fill(max_size, |ix| step_fragment_utxo(fragment, rng, ix)),
            Self::Pools => fill(max_size, |ix| step_fragment_pools(fragment, rng, ix)),
            Self::Accounts => fill(max_size, |ix| step_fragment_accounts(fragment, rng, ix)),
            Self::Withdrawals => fill(max_size, |ix| step_fragment_withdrawals(fragment, rng, ix)),
            Self::Committee => fill(max_size, |ix| step_fragment_committee(fragment, rng, ix)),
            Self::DReps => fill(max_size, |ix| step_fragment_dreps(fragment, rng, ix)),
            Self::Proposals => fill(max_size, |ix| step_fragment_proposals(fragment, rng, ix)),
            Self::Votes => fill(max_size, |ix| step_fragment_votes(fragment, rng, ix)),
            Self::Mixed => fill(max_size, |ix| {
                let all = Self::round_robin();
                let len = all.len();
                match all[ix % len] {
                    Self::Utxo => step_fragment_utxo(fragment, rng, ix / len),
                    Self::Pools => step_fragment_pools(fragment, rng, ix / len),
                    Self::Accounts => step_fragment_accounts(fragment, rng, ix / len),
                    Self::Withdrawals => step_fragment_withdrawals(fragment, rng, ix / len),
                    Self::Committee => step_fragment_committee(fragment, rng, ix / len),
                    Self::DReps => step_fragment_dreps(fragment, rng, ix / len),
                    Self::Proposals => step_fragment_proposals(fragment, rng, ix / len),
                    Self::Votes => step_fragment_votes(fragment, rng, ix / len),
                    Self::Mixed => unreachable!("Mixed scenario showed up in .round_robin()?"),
                }
            }),
        }
    }

    // Use a fragment to populate a preparation context
    pub fn prepare_fragment<'a>(self, ctx: &mut impl PreparationContext<'a>, fragment: &'a VolatileFragment) {
        match self {
            Self::Utxo => {
                fragment.utxo.produced.keys().for_each(|input| ctx.require_input(input));
            }
            Self::Pools => {
                fragment.pools.registered.keys().for_each(|pool| ctx.require_pool(pool));
            }
            Self::Accounts => {
                std::iter::empty()
                    .chain(fragment.accounts.registered.keys())
                    .chain(fragment.accounts.unregistered.iter())
                    .for_each(|account| ctx.require_account(Cow::Borrowed(account)));
            }
            Self::Withdrawals => {
                fragment.withdrawals.iter().for_each(|account| ctx.require_account(Cow::Borrowed(account)));
            }
            Self::Committee => {
                fragment.committee.registered.keys().for_each(|cc_member| ctx.require_committee_member(cc_member));
            }
            Self::DReps => {
                std::iter::empty()
                    .chain(fragment.dreps.registered.keys())
                    .chain(fragment.dreps.unregistered.iter())
                    .for_each(|drep| ctx.require_drep(Cow::Borrowed(drep)));
            }
            Self::Proposals => { /* Nothing to do, because we generate Information proposals */ }
            Self::Votes => {
                /* Nothing to do *for now*, because we don't currently resolve voter. But
                 * eventually, we should require CC members, DReps or accounts as needed  */
            }
            Self::Mixed => Self::round_robin().iter().for_each(|scenario| {
                assert_ne!(scenario, &Scenario::Mixed);
                scenario.prepare_fragment(ctx, fragment);
            }),
        }
    }

    // Create a volatile database, pre-filled with data along the given dimension
    pub fn new_volatile_db(self, rng: &mut impl Rng, scale: &BenchScale) -> VolatileDB {
        let mut db = VolatileDB::new(
            Epoch::from(0),
            MAINNET_DEFAULT_PROTOCOL_PARAMETERS.clone(),
            GovernanceActivity::default(),
            None,
        );

        (0..scale.volatile_size).for_each(|ix| {
            let mut fragment = VolatileFragment::default();
            self.mut_fragment(&mut fragment, rng, scale.block_size);
            db.push_back(fragment.anchor(fixture::tip(rng, ix as u64), fixture::default_pool_id()));
        });

        db
    }

    // Generate a fat fragment, also filled with entities of the scenario's kind
    pub fn new_fragment(self, rng: &mut impl Rng, scale: &BenchScale) -> AnchoredVolatileFragment {
        let mut fragment = VolatileFragment::default();
        self.mut_fragment(&mut fragment, rng, scale.block_size);
        fragment.anchor(fixture::tip(rng, scale.volatile_size as u64), fixture::default_pool_id())
    }
}

// ----------------------------------------------------------------------------------------- Helpers

/// Repeat a sized operation until a max_size is reached.
fn fill(max_size: usize, mut next: impl FnMut(usize) -> usize) {
    let mut size = 0;
    let mut ix = 0;
    loop {
        if size >= max_size {
            break;
        }

        size += next(ix);

        ix += 1;
    }
}

// ------------------------------------------------------------------------------------------- Steps

fn step_fragment_utxo(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, ix: usize) -> usize {
    let input = fixture::input(rng);

    if ix.is_multiple_of(2) {
        let output = Arc::new(fixture::output(rng));
        fragment.utxo.produce(input, output);
    } else {
        fragment.utxo.consume(input);
    }

    Scenario::Utxo.per_item_size()
}

fn step_fragment_pools(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, ix: usize) -> usize {
    let pool_id = fixture::pool_id(rng);

    if ix.is_multiple_of(2) {
        let params = fixture::pool_params(rng);
        let deposit = rng.random();
        fragment.pools.register(pool_id, Arc::new((params, CertificatePointer::default(), deposit)));
    } else {
        fragment.pools.unregister(pool_id, Epoch::default() + 1);
    }

    Scenario::Pools.per_item_size()
}

fn step_fragment_accounts(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, ix: usize) -> usize {
    let stake_credential = fixture::stake_credential(rng);

    let round = ix % 5;
    let is_registration = round <= 3;

    let has_pool_delegation = round == 1 || round == 2;
    let pool_delegation =
        if has_pool_delegation { Some((fixture::pool_id(rng), CertificatePointer::default())) } else { None };

    let has_drep_delegation = round == 1 || round == 3;
    let drep_delegation =
        if has_drep_delegation { Some((fixture::drep(rng), CertificatePointer::default())) } else { None };

    if is_registration {
        let balance = rng.random();
        assert!(fragment.accounts.register(stake_credential, balance, pool_delegation, drep_delegation,).is_ok())
    } else {
        fragment.accounts.unregister(stake_credential);
    }

    Scenario::Accounts.per_item_size()
}

fn step_fragment_withdrawals(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, _ix: usize) -> usize {
    let stake_credential = fixture::stake_credential(rng);

    fragment.withdrawals.insert(stake_credential);

    Scenario::Withdrawals.per_item_size()
}

#[allow(clippy::unwrap_used)]
fn step_fragment_committee(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, ix: usize) -> usize {
    let cold_credential = fixture::stake_credential(rng);

    if ix.is_multiple_of(2) {
        let hot_credential = fixture::stake_credential(rng);
        fragment.committee.bind_left(cold_credential, Some(hot_credential.into())).unwrap();
    } else {
        fragment.committee.bind_left(cold_credential, None).unwrap();
    }

    Scenario::Committee.per_item_size()
}

fn step_fragment_dreps(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, ix: usize) -> usize {
    let stake_credential = fixture::stake_credential(rng);

    if ix.is_multiple_of(2) {
        let registration = fixture::drep_registration(rng);
        assert!(fragment.dreps.register(stake_credential, registration, None, None).is_ok());
    } else {
        fragment.dreps.unregister(stake_credential);
    }

    Scenario::DReps.per_item_size()
}

fn step_fragment_proposals(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, _ix: usize) -> usize {
    let proposal_id = fixture::comparable_proposal_id(rng);

    fragment.proposals.insert(proposal_id, Arc::new((fixture::proposal(rng), ProposalPointer::default())));

    Scenario::Proposals.per_item_size()
}

fn step_fragment_votes(fragment: &mut VolatileFragment, rng: &mut impl rand::Rng, _ix: usize) -> usize {
    let ballot_id = fixture::ballot_id(rng);
    let ballot = fixture::ballot(rng);

    fragment.votes.produce(ballot_id, ballot);

    Scenario::Votes.per_item_size()
}
