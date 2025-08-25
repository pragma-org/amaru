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

use super::{
    diff_bind::{Bind, DiffBind, Empty},
    diff_epoch_reg::DiffEpochReg,
    diff_set::DiffSet,
};
use crate::{
    state::{diff_bind::Resettable, diff_epoch_reg::Registrations},
    store::{self, columns::*},
};
use amaru_kernel::{
    Anchor, Ballot, BallotId, CertificatePointer, ComparableProposalId, DRep, DRepRegistration,
    Lovelace, MemoizedTransactionOutput, Point, PoolId, PoolParams, Proposal, ProposalPointer,
    StakeCredential, TransactionInput, protocol_parameters::ProtocolParameters,
};
use amaru_slot_arithmetic::Epoch;
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use tracing::error;

pub const EVENT_TARGET: &str = "amaru::ledger::state::volatile_db";

// VolatileDB
// ----------------------------------------------------------------------------

// TODO: Currently, the cache owns data that are also available in the sequence. We could
// potentially avoid cloning and re-allocation altogether by sharing an allocator and having them
// both reference from within that allocator (e.g. an arena allocator like bumpalo)
//
// Ideally, we would just have the struct be self-referenced, but that isn't possible in Rust and
// we cannot introduce a lifetime to the VolatileDB (which would bubble up to the State).
//
// Another option is to have the cache not own data, but indices onto the sequence. This may
// require to switch the sequence back to a Vec to allow fast random lookups.
#[derive(Default)]
pub struct VolatileDB {
    cache: VolatileCache,
    sequence: VecDeque<AnchoredVolatileState>,
}

impl VolatileDB {
    pub fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    pub fn view_back(&self) -> Option<&AnchoredVolatileState> {
        self.sequence.back()
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.cache.utxo.produced.get(input)
    }

    pub fn pop_front(&mut self) -> Option<AnchoredVolatileState> {
        self.sequence.pop_front().inspect(|state| {
            // NOTE: It is imperative to remove consumed and produced UTxOs from the cache as we
            // remove them from the sequence to prevent the cache from growing out of proportion.
            for k in state.state.utxo.consumed.iter() {
                self.cache.utxo.consumed.remove(k);
            }

            for (k, _) in state.state.utxo.produced.iter() {
                self.cache.utxo.produced.remove(k);
            }
        })
    }

    pub fn push_back(&mut self, state: AnchoredVolatileState) {
        // TODO: See NOTE on VolatileDB regarding the .clone()
        self.cache.merge(state.state.utxo.clone());
        self.sequence.push_back(state);
    }

    pub fn rollback_to<E>(
        &mut self,
        point: &Point,
        on_unknown_point: impl Fn(&Point) -> E,
    ) -> Result<(), E> {
        self.cache = VolatileCache::default();

        let mut ix = 0;
        for diff in self.sequence.iter() {
            if diff.anchor.0.slot_or_default() <= point.slot_or_default() {
                // TODO: See NOTE on VolatileDB regarding the .clone()
                self.cache.merge(diff.state.utxo.clone());
                ix += 1;
            }
        }

        if ix >= self.sequence.len() {
            Err(on_unknown_point(point))
        } else {
            self.sequence.resize_with(ix, || {
                unreachable!("ix is necessarly strictly smaller than the length")
            });
            Ok(())
        }
    }
}

// VolatileCache
// ----------------------------------------------------------------------------

// TODO: At this point, we only need to lookup UTxOs, so the aggregated cache is limited to those.
// It would be relatively easy to extend to accounts, but it is trickier for pools since
// DiffEpochReg aren't meant to be mergeable across epochs.
#[derive(Default)]
struct VolatileCache {
    pub utxo: DiffSet<TransactionInput, MemoizedTransactionOutput>,
}

impl VolatileCache {
    pub fn merge(&mut self, utxo: DiffSet<TransactionInput, MemoizedTransactionOutput>) {
        self.utxo.merge(utxo);
    }
}

// VolatileState
// ----------------------------------------------------------------------------

#[derive(Debug, Default)]
pub struct VolatileState {
    pub utxo: DiffSet<TransactionInput, MemoizedTransactionOutput>,
    pub pools: DiffEpochReg<PoolId, PoolParams>,
    pub accounts: DiffBind<StakeCredential, PoolId, (DRep, CertificatePointer), Lovelace>,
    pub dreps: DiffBind<StakeCredential, Anchor, Empty, DRepRegistration>,
    pub dreps_deregistrations: BTreeMap<StakeCredential, CertificatePointer>,
    pub committee: DiffBind<StakeCredential, StakeCredential, Empty, Empty>,
    pub withdrawals: BTreeSet<StakeCredential>,
    pub proposals: DiffBind<ComparableProposalId, Empty, Empty, (Proposal, ProposalPointer)>,
    pub votes: DiffSet<BallotId, Ballot>,
    pub fees: Lovelace,
}

pub struct AnchoredVolatileState {
    pub anchor: (Point, PoolId),
    pub state: VolatileState,
}

impl VolatileState {
    pub fn anchor(self, point: &Point, issuer: PoolId) -> AnchoredVolatileState {
        AnchoredVolatileState {
            anchor: (point.clone(), issuer),
            state: self,
        }
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&MemoizedTransactionOutput> {
        self.utxo.produced.get(input)
    }
}

// StoreUpdate
// ----------------------------------------------------------------------------

pub struct StoreUpdate<W, A, R> {
    pub point: Point,
    pub issuer: PoolId,
    pub fees: Lovelace,
    pub withdrawals: W,
    pub add: A,
    pub remove: R,
}

impl AnchoredVolatileState {
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

        StoreUpdate {
            point: self.anchor.0,
            issuer: self.anchor.1,
            fees: self.state.fees,
            withdrawals: self.state.withdrawals.into_iter(),
            add: store::Columns {
                utxo: self.state.utxo.produced.into_iter(),
                pools: add_pools(self.state.pools.registered.into_iter(), epoch),
                accounts: add_accounts(self.state.accounts.registered.into_iter()),
                dreps: add_dreps(self.state.dreps.registered.into_iter()),
                cc_members: add_committee(self.state.committee.registered.into_iter()),
                proposals: add_proposals(
                    self.state.proposals.registered.into_iter(),
                    epoch + gov_action_lifetime,
                ),
                votes: self.state.votes.produced.into_iter(),
            },
            remove: store::Columns {
                utxo: self.state.utxo.consumed.into_iter(),
                pools: self.state.pools.unregistered.into_iter(),
                accounts: self.state.accounts.unregistered.into_iter(),
                dreps: remove_dreps(
                    self.state.dreps.unregistered.into_iter(),
                    self.state.dreps_deregistrations,
                ),
                cc_members: self.state.committee.unregistered.into_iter(),
                proposals: {
                    debug_assert!(self.state.proposals.unregistered.is_empty());
                    std::iter::empty()
                },
                votes: {
                    debug_assert!(self.state.votes.consumed.is_empty());
                    std::iter::empty()
                },
            },
        }
    }
}

// -------------------------------------------------------------------- Pools
// --------------------------------------------------------------------------

fn add_pools(
    iterator: impl Iterator<Item = (PoolId, Registrations<PoolParams>)>,
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
            .map(|pool| (pool, epoch + 1))
            .collect::<Vec<_>>()
    })
}

// ----------------------------------------------------------------- Accounts
// --------------------------------------------------------------------------

fn add_accounts(
    iterator: impl Iterator<
        Item = (
            StakeCredential,
            Bind<PoolId, (DRep, CertificatePointer), Lovelace>,
        ),
    >,
) -> impl Iterator<Item = (accounts::Key, accounts::Value)> {
    iterator.map(
        |(
            credential,
            Bind {
                left: pool,
                right: drep,
                value: deposit,
            },
        )| { (credential, (pool, drep, deposit, 0)) },
    )
}

// -------------------------------------------------------------------- DReps
// --------------------------------------------------------------------------

fn add_dreps(
    iterator: impl Iterator<Item = (StakeCredential, Bind<Anchor, Empty, DRepRegistration>)>,
) -> impl Iterator<Item = (dreps::Key, dreps::Value)> {
    iterator.map(
        move |(
            credential,
            Bind {
                left: anchor,
                right: _,
                value: registration,
            },
        ): (_, Bind<_, Empty, _>)| { (credential, (anchor, registration)) },
    )
}

fn remove_dreps(
    iterator: impl Iterator<Item = StakeCredential>,
    mut deregistrations: BTreeMap<StakeCredential, CertificatePointer>,
) -> impl Iterator<Item = (dreps::Key, CertificatePointer)> {
    iterator.map(move |credential| {
        #[allow(clippy::expect_used)]
        let pointer = deregistrations
            .remove(&credential)
            .expect("every 'unregistered' drep must have a matching deregistration");

        (credential, pointer)
    })
}

// --------------------------------------------------------------- CC Members
// --------------------------------------------------------------------------

fn add_committee(
    iterator: impl Iterator<Item = (StakeCredential, Bind<StakeCredential, Empty, Empty>)>,
) -> impl Iterator<Item = (cc_members::Key, cc_members::Value)> {
    iterator.map(
        |(
            credential,
            Bind {
                left: hot_credential,
                right: _,
                value: _,
            },
        )| { (credential, (hot_credential, Resettable::Unchanged)) },
    )
}

// ---------------------------------------------------------------- Proposals
// --------------------------------------------------------------------------

fn add_proposals(
    iterator: impl Iterator<
        Item = (
            ComparableProposalId,
            Bind<Empty, Empty, (Proposal, ProposalPointer)>,
        ),
    >,
    expiration: Epoch,
) -> impl Iterator<Item = (proposals::Key, proposals::Value)> {
    iterator.enumerate().filter_map(
        move |(
            index,
            (
                proposal_id,
                Bind {
                    left: _,
                    right: _,
                    value,
                },
            ),
        ): (usize, (_, Bind<_, Empty, _>))| {
            match value {
                Some((proposal, proposed_in)) => Some((
                    proposal_id,
                    proposals::Value {
                        proposed_in,
                        valid_until: expiration,
                        proposal,
                    },
                )),
                None => {
                    error!(
                        target: EVENT_TARGET,
                        index,
                        "add.proposals.no_proposal",
                    );
                    None
                }
            }
        },
    )
}
