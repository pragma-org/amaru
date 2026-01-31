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
    Epoch, Lovelace, MemoizedTransactionOutput, Point, PoolId, PoolParams, Proposal,
    ProposalPointer, ProtocolParameters, StakeCredential, TransactionInput,
};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

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

    pub fn view_front(&self) -> Option<&AnchoredVolatileState> {
        self.sequence.front()
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
        let target_slot = point.slot_or_default();

        // Check if the target point is beyond the sequence
        // In this case we simply return Ok since it this would not change the volatile state.
        if let Some(last) = self.sequence.back()
            && last.anchor.0.slot_or_default() < target_slot
        {
            tracing::warn!(
                %target_slot,
                last_slot = ?last.anchor.0.slot_or_default(),
                "Attempting to rollback to a point beyond the last known volatile state"
            );
            return Ok(());
        }

        // Check if the target point is before the sequence
        // In this case we return an error since it means rolling back the stable DB
        if let Some(first) = self.sequence.front()
            && target_slot < first.anchor.0.slot_or_default()
        {
            tracing::error!(
                %target_slot,
                first_slot = ?first.anchor.0.slot_or_default(),
                "Attempting to rollback to a point before the first point of the volatile state"
            );
            return Err(on_unknown_point(point));
        }

        // Now we know the target point is within the sequence
        // Rebuild the cache up to that point
        self.cache = VolatileCache::default();

        // Keep all elements with slot <= target_slot
        let mut ix = 0;
        for diff in self.sequence.iter() {
            if diff.anchor.0.slot_or_default() <= target_slot {
                // TODO: See NOTE on VolatileDB regarding the .clone()
                self.cache.merge(diff.state.utxo.clone());
                ix += 1;
            } else {
                break;
            }
        }

        self.sequence.resize_with(ix, || {
            unreachable!("ix cannot exceed sequence length due to the loop break")
        });
        Ok(())
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
    pub pools: DiffEpochReg<PoolId, (PoolParams, CertificatePointer)>,
    pub accounts: DiffBind<
        StakeCredential,
        (PoolId, CertificatePointer),
        (DRep, CertificatePointer),
        Lovelace,
    >,
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
            anchor: (*point, issuer),
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
    #[expect(clippy::type_complexity)]
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

// ----------------------------------------------------------------- Accounts
// --------------------------------------------------------------------------

fn add_accounts(
    iterator: impl Iterator<
        Item = (
            StakeCredential,
            Bind<(PoolId, CertificatePointer), (DRep, CertificatePointer), Lovelace>,
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
        #[expect(clippy::expect_used)]
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
                    tracing::error!(
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

#[cfg(test)]
mod tests {
    use super::*;
    use amaru_kernel::{Hash, Point, Slot};

    #[test]
    fn test_rollback_to_point_before_sequence_fails() {
        // Create a VolatileDB with three states at slots 10, 20, 30
        let mut db = create_volatile_db();

        // Rollback to slot 5 (before the first element at slot 10)
        // This represents rolling back to a point in the stable DB
        let rollback_point = Point::Specific(Slot::from(5), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point, |_| "Point not found");

        // This should fail
        // (rolling back to a point inside the stable DB is not allowed)
        assert!(result.is_err());
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_exact_last_element_should_succeed() {
        // Create a VolatileDB with three states at slots 10, 20, 30
        let mut db = create_volatile_db();

        // Rollback to slot 30 (the last element)
        let rollback_point = Point::Specific(Slot::from(30), Hash::new([0u8; 32]));

        // This should succeed, keeping all 3 elements
        let result = db.rollback_to(&rollback_point, |_| "Point not found");

        assert!(
            result.is_ok(),
            "Rolling back to the exact slot of the last element should succeed"
        );
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_middle_element_succeeds() {
        // Create a VolatileDB with three states at slots 10, 20, 30
        let mut db = create_volatile_db();

        // Rollback to slot 20 (middle element)
        let rollback_point = Point::Specific(Slot::from(20), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point, |_| "Point not found");

        // This should succeed
        assert!(result.is_ok());
        assert_eq!(db.len(), 2, "Should keep elements at slots 10 and 20");
    }

    #[test]
    fn test_rollback_to_point_after_sequence_succeeds() {
        // Create a VolatileDB with three states at slots 10, 20, 30
        let mut db = create_volatile_db();

        // Try to rollback to slot 40 (after the sequence)
        let rollback_point = Point::Specific(Slot::from(40), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point, |_| "Point not found");

        // This should succeed
        assert!(
            result.is_ok(),
            "Rolling back to a point after the sequence should succeed"
        );
        assert_eq!(db.len(), 3, "All elements should be retained");
    }

    #[test]
    fn test_rollback_to_slot_between_elements_succeeds() {
        // Create a VolatileDB with three states at slots 10, 20, 30
        let mut db = create_volatile_db();

        // Rollback to slot 25 (between 20 and 30)
        let rollback_point = Point::Specific(Slot::from(25), Hash::new([0u8; 32]));

        let result = db.rollback_to(&rollback_point, |_| "Point not found");

        // This should succeed and keep elements at slots 10 and 20
        assert!(result.is_ok());
        assert_eq!(db.len(), 2, "Should keep elements at slots 10 and 20");
    }

    // HELPERS

    fn create_test_state(slot: u64, pool_id: u8) -> AnchoredVolatileState {
        let point = Point::Specific(Slot::from(slot), Hash::new([0u8; 32]));
        let pool = Hash::new([pool_id; 28]);

        AnchoredVolatileState {
            anchor: (point, pool),
            state: VolatileState::default(),
        }
    }

    fn create_volatile_db() -> VolatileDB {
        let mut db = VolatileDB::default();
        db.push_back(create_test_state(10, 1));
        db.push_back(create_test_state(20, 2));
        db.push_back(create_test_state(30, 3));

        assert_eq!(db.len(), 3);
        db
    }
}
