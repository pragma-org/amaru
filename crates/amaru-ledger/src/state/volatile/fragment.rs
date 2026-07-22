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

/// A stake account's accumulated binding: pool/vote delegations, plus the deposit on registration.
pub type AccountBind = Bind<(PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>;

/// A DRep's accumulated binding: metadata anchor, and the DRep registration data.
pub type DRepBind = Bind<Anchor, Empty, Arc<DRepRegistration>>;

/// A CC member's accumulated binding: the hot-key delegation. Membership and term come from below,
/// since no in-block cert establishes them.
pub type CommitteeMemberBind = Bind<StakeCredential, Empty, Empty>;

/// A volatile layer's verdict on an entity.
/// - `T` is the resolved record.
/// - `Gone` is a tombstone, so don't fall back to the stable store.
#[derive(Debug, Clone)]
pub enum Existence<T> {
    Exists(T),
    Gone,
    Unknown,
}

impl<L, R, V> Existence<Bind<L, R, V>> {
    /// Layer this verdict over an `older` one, evaluated lazily
    pub fn or_else(self, older: impl FnOnce() -> Self) -> Self {
        match self {
            Existence::Unknown => older(),

            // If this is rebinding (i.e. value is None), then we must still take into
            // account the previous value, if any.
            //
            // NOTE: superfluous 'is_none()' check actually not superfluous
            //
            // The `value.is_none()` guard may seem redundant with the implementation of `then`.
            // But it allows to only lazily get the `older` state when we truly have to. Indeed, if
            // there already exists a newer value, that means the object was entirely re-recreated
            // and there's no need to fetch the previous state for any left or right binds. It's
            // just been overidden.
            //
            // Hence, the guard doesn't fundamentally changes the logic since `older.then(newer)`
            // would simply override `older` with `newer` when the value exists; but it saves us
            // from fetching the `older` to begin with.
            Existence::Exists(newer)
                if newer.value.is_none()
                    && let Existence::Exists(mut older) = older() =>
            {
                older.then(newer);
                Existence::Exists(older)
            }

            Existence::Gone | Existence::Exists(..) => self,
        }
    }
}

/// Resulting state change coming from processing a block.
#[derive(Debug, Default)]
pub struct VolatileFragment {
    pub utxo: DiffSet<TransactionInput, Arc<MemoizedTransactionOutput>>,
    pub pools: DiffEpochReg<PoolId, Arc<(PoolParams, CertificatePointer, Lovelace)>>,
    pub accounts: DiffBind<StakeCredential, (PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>,
    pub dreps: DiffBind<StakeCredential, Anchor, Empty, Arc<DRepRegistration>>,
    pub dreps_deregistrations: BTreeMap<StakeCredential, CertificatePointer>,
    pub committee: DiffSet<StakeCredential, StakeCredential>,
    pub withdrawals: BTreeSet<StakeCredential>,
    pub proposals: BTreeMap<ComparableProposalId, Arc<(Proposal, ProposalPointer)>>,
    pub votes: DiffSet<BallotId, Ballot>,
    pub fees: Lovelace,
    pub donations: Lovelace,
}

impl VolatileFragment {
    pub fn anchor(self, tip: Tip, issuer: PoolId) -> AnchoredVolatileFragment {
        AnchoredVolatileFragment { anchor: (tip, issuer), fragment: self }
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.produced.get(input).map(|output| output.as_ref())
    }

    pub fn has_consumed_input(&self, input: &TransactionInput) -> bool {
        self.utxo.consumed.contains(input)
    }

    /// Whether this fragment registered the given pool. Unregistrations
    /// do *not* affect existence: a pool stays live until it is actually retired at the epoch boundary.
    pub fn resolve_pool(&self, pool_id: PoolId) -> bool {
        self.pools.registered.contains_key(&pool_id)
    }

    /// This fragment's verdict on a stake account. Deregistration is immediate, so an `unregistered`
    /// entry is a live tombstone.
    pub fn resolve_account(&self, credential: &StakeCredential) -> Existence<AccountBind> {
        if let Some(bind) = self.accounts.registered.get(credential) {
            Existence::Exists(bind.clone())
        } else if self.accounts.unregistered.contains(credential) {
            Existence::Gone
        } else {
            Existence::Unknown
        }
    }

    /// This fragment's verdict on a DRep. Deregistration is immediate, so an `unregistered`
    /// entry is a live tombstone.
    pub fn resolve_drep(&self, credential: &StakeCredential) -> Existence<DRepBind> {
        if let Some(bind) = self.dreps.registered.get(credential) {
            Existence::Exists(bind.clone())
        } else if self.dreps.unregistered.contains(credential) {
            Existence::Gone
        } else {
            Existence::Unknown
        }
    }

    /// This fragment's verdict on a CC member. Resignation is immediate, so a resignation entry is a
    /// live tombstone. A delegation resolves as a bind-only update (`value: None`): no in-block cert
    /// establishes membership, so existence still defers to the layer below.
    pub fn resolve_cc_member(&self, credential: &StakeCredential) -> Existence<CommitteeMemberBind> {
        if let Some(hot_credential) = self.committee.produced.get(credential) {
            Existence::Exists(Bind {
                left: Resettable::Set(hot_credential.clone()),
                right: Resettable::Unchanged,
                value: None,
            })
        } else if self.committee.consumed.contains(credential) {
            Existence::Gone
        } else {
            Existence::Unknown
        }
    }

    /// This fragment's view of a governance proposal. Proposals are add-only in a block, so this is
    /// `Exists` or `Unknown`; pruning only happens at the boundary.
    pub fn resolve_proposal(&self, id: &ComparableProposalId) -> Existence<()> {
        match self.proposals.get(id) {
            Some(_) => Existence::Exists(()),
            None => Existence::Unknown,
        }
    }

    /// Whether this fragment withdrew the account's rewards.
    pub fn withdrew(&self, credential: &StakeCredential) -> bool {
        self.withdrawals.contains(credential)
    }

    /// A best-effort cleanup of a previous fragment in the current aggreate. This is not generally
    /// possible (with our current design), for all elements in a fragment, because we loose
    /// information each time we aggregate two fragments (a little thought exercise with account
    /// registrations and delegations should be convincing enough).
    ///
    /// But it is possible for a few types such as the `DiffSet` and the various maps. Note that
    /// not cleaning up all the data is not fundamentally wrong; but it is *leaking memory*. We
    /// just keep in memory information that we should have flushed on-disk.
    ///
    /// Yet, this is counterbalanced by the frequent rollbacks happening on Cardano (once every
    /// 10-15min due to slot battles). Rollbacks are infrequent enough and frequent enough that
    /// they are the perfect opportunity to cleanup the now-stable memory (by re-computing the
    /// aggregate from scratch). Also, because we cannot *guarantee* that rollbacks happen, we
    /// still also manually perform such a cleanup every now-and-then using a counter that gets
    /// reset for every rollback.
    pub fn incremental_cleanup(&mut self, fragment: &VolatileFragment) {
        let VolatileFragment {
            utxo,
            votes,
            withdrawals,
            proposals,
            fees,
            donations,
            accounts: _,
            committee: _,
            dreps: _,
            dreps_deregistrations: _,
            pools: _,
        } = fragment;

        self.utxo.cleanup(utxo);

        self.votes.cleanup(votes);

        for credential in withdrawals {
            self.withdrawals.remove(credential);
        }

        for proposal_id in proposals.keys() {
            self.proposals.remove(proposal_id);
        }

        self.fees -= *fees;
        self.donations -= *donations;
    }

    /// Fold `more_recent` into this fragment, treating it as applied *after* `self`.
    /// This maintains the running aggregate of a [`crate::state::volatile::VolatileSeries`].
    pub fn compose(&mut self, more_recent: &VolatileFragment) {
        let VolatileFragment {
            utxo,
            votes,
            pools,
            withdrawals,
            proposals,
            fees,
            donations,
            accounts,
            dreps,
            dreps_deregistrations,
            committee,
        } = more_recent;

        self.utxo.extend(utxo);
        self.votes.extend(votes);
        self.pools.extend(pools);
        self.withdrawals.extend(withdrawals.iter().cloned());
        self.proposals.extend(proposals.iter().map(|(id, value)| (id.clone(), value.clone())));
        self.accounts.append(accounts.clone());
        self.dreps.append(dreps.clone());
        self.dreps_deregistrations.extend(dreps_deregistrations.iter().map(|(k, v)| (k.clone(), *v)));
        self.committee.extend(committee);
        self.fees += *fees;
        self.donations += *donations;
    }
}

// --------------------------------------------------------------------------- AnchoredVolatileFragment

/// A [`VolatileFragment`] anchored to a specific point and block issuer.
#[derive(Debug)]
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
                pools: add_pools(pools.registered.into_iter(), epoch),
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
            .map(|registration| {
                let (params, pointer, deposit) = Arc::unwrap_or_clone(registration);
                (params, pointer, deposit, epoch + 1)
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
    iterator: impl Iterator<Item = (StakeCredential, Bind<Anchor, Empty, Arc<DRepRegistration>>)>,
) -> impl Iterator<Item = (dreps::Key, dreps::Value)> {
    iterator.map(move |(credential, Bind { left: anchor, right: _, value: registration }): (_, Bind<_, Empty, _>)| {
        (credential, (anchor, registration.map(Arc::unwrap_or_clone)))
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
    iterator: impl Iterator<Item = (ComparableProposalId, Arc<(Proposal, ProposalPointer)>)>,
    expiration: Epoch,
) -> impl Iterator<Item = (proposals::Key, proposals::Value)> {
    iterator.map(move |(proposal_id, value)| {
        let (proposal, proposed_in) = Arc::unwrap_or_clone(value);
        (proposal_id, proposals::Value { proposed_in, valid_until: expiration, proposal })
    })
}
