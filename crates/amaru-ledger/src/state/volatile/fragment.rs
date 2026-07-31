// Copyright 2024 PRAGMA
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
    sync::Arc,
};

use amaru_kernel::{
    Anchor, Ballot, BallotId, CertificatePointer, DRep, DRepRegistration, Epoch, Lovelace, MemoizedTransactionOutput,
    Point, PoolId, PoolParams, Proposal, ProposalId, ProposalPointer, ProtocolParameters, Slot, StakeCredential, Tip,
    TransactionInput,
};

use crate::{
    state::volatile::{Bind, Empty, Resettable},
    store::{self, columns::*},
};

mod diff_bind;
pub use diff_bind::{BindError, DiffBind, RegisterError};

mod diff_epoch_reg;
pub use diff_epoch_reg::DiffEpochReg;

mod diff_set;
pub use diff_set::DiffSet;

mod registrations;
pub use registrations::Registrations;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    pub use super::{diff_bind::any_diff_bind, diff_set::any_diff_set};
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

// ----------------------------------------------------------------------------------- VolatileFragment

/// Resulting state change coming from processing a block.
#[derive(Debug, Default, Clone)]
pub struct VolatileFragment {
    pub utxo: DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>,
    pub pools: DiffEpochReg<PoolId, Arc<(PoolParams, CertificatePointer, Lovelace)>>,
    pub accounts: DiffBind<StakeCredential, (PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>,
    pub dreps: DiffBind<StakeCredential, Box<Anchor>, Empty, DRepRegistration>,
    pub dreps_deregistrations: BTreeMap<StakeCredential, CertificatePointer>,
    pub committee: DiffSet<StakeCredential, StakeCredential>,
    pub withdrawals: BTreeSet<StakeCredential>,
    pub proposals: BTreeMap<ProposalId, Arc<(Proposal, ProposalPointer)>>,
    pub votes: DiffSet<BallotId, Ballot>,
    pub fees: Lovelace,
    pub donations: Lovelace,
}

impl VolatileFragment {
    pub fn anchor(self, tip: Tip, issuer: PoolId) -> AnchoredVolatileFragment {
        AnchoredVolatileFragment { anchor: (tip, issuer), fragment: self }
    }
}

// --------------------------------------------------------------------------- AnchoredVolatileFragment

/// A [`VolatileFragment`] anchored to a specific point and block issuer.
#[derive(Debug, Clone)]
pub struct AnchoredVolatileFragment {
    pub anchor: (Tip, PoolId),
    pub fragment: VolatileFragment,
}

impl AnchoredVolatileFragment {
    pub fn tip(&self) -> Tip {
        self.anchor.0
    }

    pub fn slot(&self) -> Slot {
        self.tip().slot()
    }

    pub fn point(&self) -> Point {
        self.tip().point()
    }

    #[allow(clippy::type_complexity)]
    pub fn into_store_update(
        self,
        epoch: Epoch,
        protocol_parameters: &ProtocolParameters,
    ) -> StoreUpdate<
        impl Iterator<Item = accounts::Key>,
        store::Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
            impl Iterator<Item = (dreps::Key, dreps::Value)>,
            impl Iterator<Item = (cc_members::Key, cc_members::Value)>,
            impl Iterator<Item = (proposals::Key, proposals::Value)>,
            impl Iterator<Item = (votes::Key, votes::Value)>,
        >,
        store::Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
            impl Iterator<Item = (dreps::Key, CertificatePointer)>,
            impl Iterator<Item = cc_members::Key>,
            impl Iterator<Item = ()>,
            impl Iterator<Item = ()>,
        >,
    > {
        let gov_action_lifetime = protocol_parameters.gov_action_lifetime;

        let Self {
            fragment:
                VolatileFragment {
                    utxo,
                    pools,
                    accounts,
                    dreps,
                    dreps_deregistrations,
                    committee,
                    withdrawals,
                    proposals,
                    votes,
                    fees,
                    donations,
                },
            anchor: (tip, issuer),
        } = self;

        StoreUpdate {
            point: tip.point(),
            issuer,
            fees,
            donations,
            withdrawals: withdrawals.into_iter(),
            add: store::Columns {
                utxo: utxo.produced.into_iter().map(|(input, output)| (input, Arc::unwrap_or_clone(output))),
                pools: add_pools(pools.registered.into_iter()),
                accounts: add_accounts(accounts.registered.into_iter()),
                dreps: add_dreps(dreps.registered.into_iter()),
                cc_members: add_committee(committee.produced.into_iter()),
                proposals: add_proposals(proposals.into_iter(), epoch + gov_action_lifetime),
                votes: votes.produced.into_iter(),
            },
            remove: store::Columns {
                utxo: utxo.consumed.into_iter(),
                pools: pools.unregistered.into_iter(),
                accounts: accounts.unregistered.into_iter(),
                dreps: remove_dreps(dreps.unregistered.into_iter(), dreps_deregistrations),
                cc_members: committee.consumed.into_iter(),
                proposals: std::iter::empty(),
                votes: {
                    debug_assert!(votes.consumed.is_empty());
                    std::iter::empty()
                },
            },
        }
    }
}

#[cfg(test)]
impl AnchoredVolatileFragment {
    pub fn fixture(slot: u64, pool_id: u8) -> Self {
        use amaru_kernel::{BlockHeight, Hash};

        let point = Point::Specific(Slot::from(slot), Hash::new([0u8; 32]));
        let pool = Hash::new([pool_id; 28]);
        let tip = Tip::new(point, BlockHeight::from(slot));

        Self { anchor: (tip, pool), fragment: VolatileFragment::default() }
    }
}

// ------------------------------------------------------------------------------------------- StoreUpdate

pub struct StoreUpdate<W, A, R> {
    pub point: Point,
    pub issuer: PoolId,
    pub fees: Lovelace,
    pub donations: Lovelace,
    pub withdrawals: W,
    pub add: A,
    pub remove: R,
}

// ------------------------------------------------------------------------------------------- Pools

pub(crate) fn add_pools(
    iterator: impl Iterator<Item = (PoolId, Registrations<Arc<(PoolParams, CertificatePointer, Lovelace)>>)>,
) -> impl Iterator<Item = pools::Value> {
    iterator.flat_map(move |(_, registrations)| {
        registrations
            .into_iter()
            .map(|registration| {
                let (params, pointer, deposit) = Arc::unwrap_or_clone(registration);
                (params, pointer, deposit)
            })
            .collect::<Vec<_>>()
    })
}

// ---------------------------------------------------------------------------------------- Accounts

pub(crate) fn add_accounts(
    iterator: impl Iterator<
        Item = (StakeCredential, Bind<(PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>),
    >,
) -> impl Iterator<Item = (accounts::Key, accounts::Value)> {
    iterator.map(|(credential, Bind { left: pool, right: drep, value: deposit })| {
        // A bound deposit denotes a (re-)registration within the window (see DiffBind::register);
        // without one, only delegations changed and the account is known to exist already.
        let value = match deposit {
            Some(deposit) => accounts::Value::Create { pool, drep, deposit, rewards: 0 },
            None => accounts::Value::Update { pool, drep },
        };
        (credential, value)
    })
}

// ------------------------------------------------------------------------------------------- DReps

pub(crate) fn add_dreps(
    iterator: impl Iterator<Item = (StakeCredential, Bind<Box<Anchor>, Empty, DRepRegistration>)>,
) -> impl Iterator<Item = (dreps::Key, dreps::Value)> {
    iterator.map(move |(credential, Bind { left: anchor, right: _, value: registration }): (_, Bind<_, Empty, _>)| {
        (credential, (anchor, registration))
    })
}

pub(crate) fn remove_dreps(
    iterator: impl Iterator<Item = StakeCredential>,
    mut deregistrations: BTreeMap<StakeCredential, CertificatePointer>,
) -> impl Iterator<Item = (dreps::Key, CertificatePointer)> {
    iterator.map(move |credential| {
        #[expect(clippy::expect_used)]
        let pointer =
            deregistrations.remove(&credential).expect("every 'unregistered' drep must have a matching deregistration");

        (credential, pointer)
    })
}

// ------------------------------------------------------------------------ Constitutional Committee

pub(crate) fn add_committee(
    iterator: impl Iterator<Item = (StakeCredential, StakeCredential)>,
) -> impl Iterator<Item = (cc_members::Key, cc_members::Value)> {
    iterator.map(|(credential, hot_credential)| (credential, (Resettable::Set(hot_credential), Resettable::Unchanged)))
}

// --------------------------------------------------------------------------------------- Proposals

pub(crate) fn add_proposals(
    iterator: impl Iterator<Item = (ProposalId, Arc<(Proposal, ProposalPointer)>)>,
    expiration: Epoch,
) -> impl Iterator<Item = (proposals::Key, proposals::Value)> {
    iterator.map(move |(proposal_id, value)| {
        let (proposal, proposed_in) = Arc::unwrap_or_clone(value);
        (proposal_id, proposals::Value { proposed_in, valid_until: expiration, proposal })
    })
}
