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

use std::collections::{BTreeMap, BTreeSet};

use amaru_kernel::{
    Anchor, Ballot, BallotId, CertificatePointer, ComparableProposalId, DRep, DRepRegistration, Epoch, Lovelace,
    MemoizedTransactionOutput, Point, PoolId, PoolParams, Proposal, ProposalPointer, ProtocolParameters, Slot,
    StakeCredential, Tip, TransactionInput,
};

use crate::{
    state::{
        diff_bind::{Bind, DiffBind, Empty, Resettable},
        diff_epoch_reg::{DiffEpochReg, Registrations},
        diff_set::DiffSet,
    },
    store::{self, columns::*},
};

// ----------------------------------------------------------------------------------- VolatileFragment

/// Resulting state change coming from processing a block.
#[derive(Debug, Default)]
pub struct VolatileFragment {
    pub utxo: DiffSet<TransactionInput, MemoizedTransactionOutput>,
    pub pools: DiffEpochReg<PoolId, (PoolParams, CertificatePointer)>,
    pub accounts: DiffBind<StakeCredential, (PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>,
    pub dreps: DiffBind<StakeCredential, Anchor, Empty, DRepRegistration>,
    pub dreps_deregistrations: BTreeMap<StakeCredential, CertificatePointer>,
    pub committee: DiffBind<StakeCredential, StakeCredential, Empty, Empty>,
    pub withdrawals: BTreeSet<StakeCredential>,
    pub proposals: BTreeMap<ComparableProposalId, (Proposal, ProposalPointer)>,
    pub votes: DiffSet<BallotId, Ballot>,
    pub fees: Lovelace,
}

impl VolatileFragment {
    pub fn anchor(self, tip: Tip, issuer: PoolId) -> AnchoredVolatileFragment {
        AnchoredVolatileFragment { anchor: FragmentAnchor { tip, issuer }, fragment: self }
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.produced.get(input)
    }

    pub fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.utxo.consumed.contains(input)
    }
}

#[derive(Debug, Clone)]
pub struct FragmentAnchor {
    pub tip: Tip,
    pub issuer: PoolId,
}

// --------------------------------------------------------------------------- AnchoredVolatileFragment

/// A [`VolatileFragment`] anchored to a specific point and block issuer.
#[derive(Debug)]
pub struct AnchoredVolatileFragment {
    pub anchor: FragmentAnchor,
    pub fragment: VolatileFragment,
}

impl AnchoredVolatileFragment {
    pub fn tip(&self) -> Tip {
        self.anchor.tip
    }

    pub fn slot(&self) -> Slot {
        self.tip().slot()
    }

    pub fn point(&self) -> Point {
        self.tip().point()
    }

    pub fn issuer(&self) -> PoolId {
        self.anchor.issuer
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
                },
            anchor: FragmentAnchor { tip, issuer },
        } = self;

        StoreUpdate {
            point: tip.point(),
            issuer,
            fees,
            withdrawals: withdrawals.into_iter(),
            add: store::Columns {
                utxo: utxo.produced.into_iter(),
                pools: add_pools(pools.registered.into_iter(), epoch),
                accounts: add_accounts(accounts.registered.into_iter()),
                dreps: add_dreps(dreps.registered.into_iter()),
                cc_members: add_committee(committee.registered.into_iter()),
                proposals: add_proposals(proposals.into_iter(), epoch + gov_action_lifetime),
                votes: votes.produced.into_iter(),
            },
            remove: store::Columns {
                utxo: utxo.consumed.into_iter(),
                pools: pools.unregistered.into_iter(),
                accounts: accounts.unregistered.into_iter(),
                dreps: remove_dreps(dreps.unregistered.into_iter(), dreps_deregistrations),
                cc_members: committee.unregistered.into_iter(),
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
        let issuer: PoolId = PoolId::new(Hash::new([pool_id; 28]));
        let tip = Tip::new(point, BlockHeight::from(slot));

        Self { anchor: FragmentAnchor { tip, issuer }, fragment: VolatileFragment::default() }
    }
}

// ------------------------------------------------------------------------------------------- StoreUpdate

pub struct StoreUpdate<W, A, R> {
    pub point: Point,
    pub issuer: PoolId,
    pub fees: Lovelace,
    pub withdrawals: W,
    pub add: A,
    pub remove: R,
}

// ------------------------------------------------------------------------------------------- Pools

pub(crate) fn add_pools(
    iterator: impl Iterator<Item = (PoolId, Registrations<(PoolParams, CertificatePointer)>)>,
    epoch: Epoch,
) -> impl Iterator<Item = pools::Value> {
    iterator.flat_map(move |(_, registrations)| {
        registrations
            .into_iter()
            // NOTE/TODO: Re-registrations (a.k.a pool params updates) are always
            // happening on the following epoch. We do not explicitly store epochs
            // for registrations in the DiffEpochReg (which may be an arguable
            // choice?) so we have to artificially set it here. Note that for
            // registrations (when there's no existing entry), the epoch is wrong
            // but it is fully ignored. It's slightly ugly, but we cannot know if
            // an entry exists without querying the stable store -- and frankly, we
            // don't _have to_.
            .map(|registration| (registration.0, registration.1, epoch + 1))
            .collect::<Vec<_>>()
    })
}

// ---------------------------------------------------------------------------------------- Accounts

pub(crate) fn add_accounts(
    iterator: impl Iterator<
        Item = (StakeCredential, Bind<(PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>),
    >,
) -> impl Iterator<Item = (accounts::Key, accounts::Value)> {
    iterator
        .map(|(credential, Bind { left: pool, right: drep, value: deposit })| (credential, (pool, drep, deposit, 0)))
}

// ------------------------------------------------------------------------------------------- DReps

pub(crate) fn add_dreps(
    iterator: impl Iterator<Item = (StakeCredential, Bind<Anchor, Empty, DRepRegistration>)>,
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
    iterator: impl Iterator<Item = (StakeCredential, Bind<StakeCredential, Empty, Empty>)>,
) -> impl Iterator<Item = (cc_members::Key, cc_members::Value)> {
    iterator.map(|(credential, Bind { left: hot_credential, right: _, value: _ })| {
        (credential, (hot_credential, Resettable::Unchanged))
    })
}

// --------------------------------------------------------------------------------------- Proposals

pub(crate) fn add_proposals(
    iterator: impl Iterator<Item = (ComparableProposalId, (Proposal, ProposalPointer))>,
    expiration: Epoch,
) -> impl Iterator<Item = (proposals::Key, proposals::Value)> {
    iterator.map(move |(proposal_id, (proposal, proposed_in))| {
        (proposal_id, proposals::Value { proposed_in, valid_until: expiration, proposal })
    })
}
