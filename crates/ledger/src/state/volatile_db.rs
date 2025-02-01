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

use super::{diff_bind::DiffBind, diff_epoch_reg::DiffEpochReg, diff_set::DiffSet};
use crate::{
    kernel::{
        epoch_from_slot, Epoch, Lovelace, Point, PoolId, PoolParams, StakeCredential,
        TransactionInput, TransactionOutput,
    },
    store::{self, columns::*},
};
use std::collections::{BTreeSet, VecDeque};

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
    sequence: VecDeque<VolatileState<(Point, PoolId)>>,
}

impl VolatileDB {
    pub fn is_empty(&self) -> bool {
        self.sequence.is_empty()
    }

    pub fn len(&self) -> usize {
        self.sequence.len()
    }

    pub fn view_back(&self) -> Option<&VolatileState<(Point, PoolId)>> {
        self.sequence.back()
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
        self.cache.utxo.produced.get(input)
    }

    pub fn iter_pools(&self) -> impl Iterator<Item = (Epoch, &DiffEpochReg<PoolId, PoolParams>)> {
        self.sequence
            .iter()
            .map(|st| (epoch_from_slot(st.anchor.0.slot_or_default()), &st.pools))
    }

    pub fn pop_front(&mut self) -> Option<VolatileState<(Point, PoolId)>> {
        self.sequence.pop_front().inspect(|state| {
            // NOTE: It is imperative to remove consumed and produced UTxOs from the cache as we
            // remove them from the sequence to prevent the cache from growing out of proportion.
            for k in state.utxo.consumed.iter() {
                self.cache.utxo.consumed.remove(k);
            }

            for (k, _) in state.utxo.produced.iter() {
                self.cache.utxo.produced.remove(k);
            }
        })
    }

    pub fn push_back(&mut self, state: VolatileState<(Point, PoolId)>) {
        // TODO: See NOTE on VolatileDB regarding the .clone()
        self.cache.merge(state.utxo.clone());
        self.sequence.push_back(state);
    }

    pub fn rollback_to<E>(&mut self, point: &Point, on_unknown_point: E) -> Result<(), E> {
        self.cache = VolatileCache::default();

        let mut ix = 0;
        for diff in self.sequence.iter() {
            if diff.anchor.0.slot_or_default() <= point.slot_or_default() {
                // TODO: See NOTE on VolatileDB regarding the .clone()
                self.cache.merge(diff.utxo.clone());
                ix += 1;
            }
        }

        if ix >= self.sequence.len() {
            Err(on_unknown_point)
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
    pub utxo: DiffSet<TransactionInput, TransactionOutput>,
}

impl VolatileCache {
    pub fn merge(&mut self, utxo: DiffSet<TransactionInput, TransactionOutput>) {
        self.utxo.merge(utxo);
    }
}

// VolatileState
// ----------------------------------------------------------------------------

pub struct VolatileState<A> {
    pub anchor: A,
    pub utxo: DiffSet<TransactionInput, TransactionOutput>,
    pub pools: DiffEpochReg<PoolId, PoolParams>,
    pub accounts: DiffBind<StakeCredential, PoolId, Lovelace>,
    pub withdrawals: BTreeSet<StakeCredential>,
    pub fees: Lovelace,
}

impl Default for VolatileState<()> {
    fn default() -> Self {
        Self {
            anchor: (),
            utxo: Default::default(),
            pools: Default::default(),
            accounts: Default::default(),
            withdrawals: Default::default(),
            fees: 0,
        }
    }
}

impl VolatileState<()> {
    pub fn anchor(self, point: &Point, issuer: PoolId) -> VolatileState<(Point, PoolId)> {
        VolatileState {
            anchor: (point.clone(), issuer),
            utxo: self.utxo,
            pools: self.pools,
            accounts: self.accounts,
            withdrawals: self.withdrawals,
            fees: self.fees,
        }
    }

    pub fn resolve_input(&self, input: &TransactionInput) -> Option<&TransactionOutput> {
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

impl VolatileState<(Point, PoolId)> {
    #[allow(clippy::type_complexity)]
    pub fn into_store_update(
        self,
    ) -> StoreUpdate<
        impl Iterator<Item = accounts::Key>,
        store::Columns<
            impl Iterator<Item = (utxo::Key, utxo::Value)>,
            impl Iterator<Item = pools::Value>,
            impl Iterator<Item = (accounts::Key, accounts::Value)>,
        >,
        store::Columns<
            impl Iterator<Item = utxo::Key>,
            impl Iterator<Item = (pools::Key, Epoch)>,
            impl Iterator<Item = accounts::Key>,
        >,
    > {
        let epoch = epoch_from_slot(self.anchor.0.slot_or_default());
        StoreUpdate {
            point: self.anchor.0,
            issuer: self.anchor.1,
            fees: self.fees,
            withdrawals: self.withdrawals.into_iter(),
            add: store::Columns {
                utxo: self.utxo.produced.into_iter(),
                pools: self
                    .pools
                    .registered
                    .into_iter()
                    .flat_map(move |(_, registrations)| {
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
                    }),
                accounts: self
                    .accounts
                    .registered
                    .into_iter()
                    .map(|(credential, (pool, deposit))| (credential, (pool, deposit, 0))),
            },
            remove: store::Columns {
                utxo: self.utxo.consumed.into_iter(),
                pools: self.pools.unregistered.into_iter(),
                accounts: self.accounts.unregistered.into_iter(),
            },
        }
    }
}
