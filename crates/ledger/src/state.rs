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

pub mod diff_bind;
pub mod diff_epoch_reg;
pub mod diff_set;
pub mod transaction;
pub mod volatile_db;

use crate::{
    kernel::{
        self, epoch_from_slot, Hash, Hasher, MintedBlock, Point, PoolId, PoolParams, PoolSigma,
        TransactionInput, TransactionOutput, CONSENSUS_SECURITY_PARAM, STABILITY_WINDOW,
    },
    rewards::{RewardsSummary, StakeDistribution},
    state::volatile_db::{StoreUpdate, VolatileDB, VolatileState},
    store::{columns::*, Store, StoreError},
};
use std::{
    borrow::Cow,
    collections::{BTreeSet, VecDeque},
    sync::{Arc, Mutex},
};
use thiserror::Error;
use tracing::{info_span, trace, trace_span, Span};

const EVENT_TARGET: &str = "amaru::ledger::state";

// State
// ----------------------------------------------------------------------------

/// The state of the ledger split into two sub-components:
///
/// - A _stable_ and persistent storage, which contains the part of the state which known to be
///   final. Fundamentally, this contains the aggregated state of the ledger that is at least 'k'
///   blocks old; where 'k' is the security parameter of the protocol.
///
/// - A _volatile_ state, which is maintained as a sequence of diff operations to be applied on
///   top of the _stable_ store. It contains at most 'CONSENSUS_SECURITY_PARAM' entries; old entries
///   get persisted in the stable storage when they are popped out of the volatile state.
pub struct State<S>
where
    S: Store,
{
    /// A handle to the stable store, shared across all ledger instances.
    stable: Arc<Mutex<S>>,

    /// Our own in-memory vector of volatile deltas to apply onto the stable store in due time.
    volatile: VolatileDB,

    /// The computed rewards summary to be applied on the next epoch boundary. This is computed
    /// once in the epoch, and held until the end where it is reset.
    rewards_summary: Option<RewardsSummary>,

    /// The latest stake distributions, used to determine the leader-schedule for the ongoing epoch
    /// and as well as the rewards.
    stake_distributions: VecDeque<StakeDistribution>,
}

impl<S: Store> State<S> {
    pub fn new(stable: Arc<Mutex<S>>) -> Self {
        let db = stable.lock().unwrap();

        // NOTE: Initialize stake distribution held in-memory. The one before last is needed by the
        // consensus layer to validate the leader schedule, while the one before that will be
        // consumed for the rewards calculation.
        //
        // We store them as a bounded queue, such that we always ever have 2 stake distributions
        // available. Once we consume the oldest one, we immediately insert a new one for the epoch
        // that just passed, pushing the other forward.
        //
        // Note that a possible memory optimization could be to discard all accounts from the most
        // recent ones, since we only truly need the stake pool distribution for the leader
        // schedule whereas accounts can be quite numerous.
        let latest_epoch = db.most_recent_snapshot();
        let mut stake_distributions = VecDeque::new();
        for epoch in latest_epoch - 2..=latest_epoch - 1 {
            stake_distributions.push_front(
                db.stake_distribution(epoch)
                    .unwrap_or_else(|e| panic!("unable to get current stake distribution: {e:?}")),
            );
        }

        drop(db);

        Self {
            stable,

            // NOTE: At this point, we always restart from an empty volatile state; which means
            // that there needs to be some form of synchronization between the consensus and the
            // ledger here. Few assumptions also stems from this:
            //
            // (1) The consensus must be storing its own state, and in particular, where it has
            //     left the synchronization.
            //
            // (2) Re-applying 2160 (already synchronized) blocks is _fast-enough_ that it can be
            //     done on restart easily. To be measured; if this turns out to be too slow, we
            //     views of the volatile DB on-disk to be able to restore them quickly.
            volatile: VolatileDB::default(),

            rewards_summary: None,

            stake_distributions,
        }
    }

    /// Inspect the tip of this ledger state. This corresponds to the point of the latest block
    /// applied to the ledger.
    pub fn tip(&'_ self) -> Cow<'_, Point> {
        if let Some(st) = self.volatile.view_back() {
            return Cow::Borrowed(&st.anchor.0);
        }

        Cow::Owned(
            self.stable
                .lock()
                .unwrap()
                .tip()
                .unwrap_or_else(|e| panic!("no tip found in stable db: {e:?}")),
        )
    }

    /// Roll the ledger forward with the given block by applying transactions one by one, in
    /// sequence. The update stops at the first invalid transaction, if any. Otherwise, it updates
    /// the internal state of the ledger.
    pub fn forward(
        &mut self,
        span: &Span,
        point: &Point,
        block: MintedBlock<'_>,
    ) -> Result<(), StateError> {
        let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);
        let relative_slot = kernel::relative_slot(point.slot_or_default());

        let state = self.apply_block(span, block).map_err(StateError::Storage)?;

        if self.volatile.len() >= CONSENSUS_SECURITY_PARAM {
            let mut db = self.stable.lock().unwrap();

            let now_stable = self.volatile.pop_front().unwrap_or_else(|| {
                unreachable!("pre-condition: self.volatile.len() >= CONSENSUS_SECURITY_PARAM")
            });

            let current_epoch = epoch_from_slot(now_stable.anchor.0.slot_or_default());
            span.record("stable.epoch", current_epoch);

            // Note: the volatile sequence may contain points belonging to two epochs. We diligently
            // make snapshots at the end of each epoch. Thus, as soon as the next stable block is
            // exactly MORE than one epoch apart, it means that we've already pushed to the stable
            // db all the blocks from the previous epoch and it's time to make a snapshot before we
            // apply this new stable diff.
            //
            // However, 'current_epoch' here refers to the _ongoing_ epoch in the volatile db. So
            // we must snapshot the one _just before_.
            if current_epoch > db.most_recent_snapshot() + 1 {
                // FIXME: All operations below should technically happen in the same database
                // transaction. If we interrupt the application between any of those, we might end
                // up with a corrupted state.
                info_span!(target: EVENT_TARGET, parent: span, "snapshot", epoch = current_epoch - 1).in_scope(|| {
                    db.next_snapshot(current_epoch - 1, self.rewards_summary.take())
                        .map_err(StateError::Storage)
                })?;

                trace_span!(target: EVENT_TARGET, parent: span, "tick.pool").in_scope(|| {
                    // Then we, can tick pools to compute their new state at the epoch boundary. Notice
                    // how we tick with the _current epoch_ however, but we take the snapshot before
                    // the tick since the actions are only effective once the epoch is crossed.
                    db.with_pools(|iterator| {
                        for (_, pool) in iterator {
                            pools::Row::tick(pool, current_epoch)
                        }
                    })
                    .map_err(StateError::Storage)
                })?;
            }

            let StoreUpdate {
                point: stable_point,
                issuer: stable_issuer,
                fees,
                add,
                remove,
                withdrawals,
            } = now_stable.into_store_update();

            trace_span!(target: EVENT_TARGET, parent: span, "save").in_scope(|| {
                db.save(
                    &stable_point,
                    Some(&stable_issuer),
                    add,
                    remove,
                    withdrawals,
                )
                .and_then(|()| {
                    db.with_pots(|mut row| {
                        row.borrow_mut().fees += fees;
                    })
                })
                .map_err(StateError::Storage)
            })?;

            // Once we reach the stability window,
            if self.rewards_summary.is_none() && relative_slot >= STABILITY_WINDOW as u64 {
                self.rewards_summary = Some(
                    db.rewards_summary(
                        self.stake_distributions
                            .pop_back()
                            .unwrap_or_else(|| panic!("no readily available stake distribution?")),
                    )
                    .map_err(StateError::Storage)?,
                );

                self.stake_distributions.push_front(
                    db.stake_distribution(current_epoch - 1)
                        .map_err(StateError::Storage)?,
                );
            }
        } else {
            trace!(target: EVENT_TARGET, parent: span, size = self.volatile.len(), "volatile.warming_up",);
        }

        span.record("tip.epoch", epoch_from_slot(point.slot_or_default()));
        span.record("tip.relative_slot", relative_slot);

        self.volatile.push_back(state.anchor(point, issuer));

        Ok(())
    }

    pub fn backward(&mut self, to: &Point) -> Result<(), BackwardError> {
        // NOTE: This happens typically on start-up; The consensus layer will typically ask us to
        // rollback to the last known point, which ought to be the tip of the database.
        if self.volatile.is_empty() && self.tip().as_ref() == to {
            return Ok(());
        }

        self.volatile.rollback_to(to, |point| {
            BackwardError::UnknownRollbackPoint(point.clone())
        })
    }

    /// Fetch stake pool details from the current live view of the ledger.
    pub fn get_pool(&self, pool: &PoolId) -> Result<Option<PoolParams>, StateError> {
        let current_epoch = epoch_from_slot(self.tip().slot_or_default());

        let volatile_view = self.volatile.iter_pools();

        match diff_epoch_reg::Fold::for_epoch(current_epoch, pool, volatile_view) {
            diff_epoch_reg::Fold::Registered(pool) => Ok(Some(pool.clone())),
            diff_epoch_reg::Fold::Unregistered => Ok(None),
            diff_epoch_reg::Fold::Undetermined => {
                let db = self.stable.lock().unwrap();
                let mut row = db.pool(pool).map_err(StateError::Query)?;
                pools::Row::tick(Box::new(&mut row), current_epoch);
                Ok(row.map(|row| row.current_params))
            }
        }
    }

    fn resolve_inputs<'a>(
        &'a self,
        ongoing_state: &VolatileState,
        inputs: impl Iterator<Item = &'a TransactionInput>,
    ) -> Result<Vec<TransactionOutput>, StoreError> {
        let mut result = Vec::new();

        for input in inputs {
            let output = ongoing_state
                .resolve_input(input)
                .cloned()
                .or_else(|| self.volatile.resolve_input(input).cloned())
                .map(Ok)
                .unwrap_or_else(|| {
                    let db = self.stable.lock().unwrap();
                    db.utxo(input).map(|opt| {
                        opt.unwrap_or_else(|| {
                            panic!("unknown UTxO expected to be known: {input:?}!")
                        })
                    })
                })?;

            result.push(output);
        }

        Ok(result)
    }

    /// Process a given block into a series of ledger-state diff (a.k.a events) to apply.
    fn apply_block(
        &self,
        parent: &Span,
        block: MintedBlock<'_>,
    ) -> Result<VolatileState, StoreError> {
        let failed_transactions = FailedTransactions::from_block(&block);
        let transaction_bodies = block.transaction_bodies.to_vec();
        let total_count = transaction_bodies.len();
        let failed_count = failed_transactions.inner.len();

        let span_apply_block = trace_span!(
            target: EVENT_TARGET,
            parent: parent,
            "block.body.validate",
            block.transactions.total = total_count,
            block.transactions.failed = failed_count,
            block.transactions.success = total_count - failed_count
        )
        .entered();

        let mut state = VolatileState::default();

        for (ix, transaction_body) in transaction_bodies.into_iter().enumerate() {
            let transaction_id = Hasher::<256>::hash(transaction_body.raw_cbor());
            let transaction_body = transaction_body.unwrap();

            let resolved_collateral_inputs = match transaction_body.collateral {
                None => vec![],
                Some(ref inputs) => self.resolve_inputs(&state, inputs.iter())?,
            };

            transaction::apply(
                &mut state,
                &span_apply_block,
                failed_transactions.has(ix as u32),
                transaction_id,
                transaction_body,
                resolved_collateral_inputs,
            );
        }

        span_apply_block.exit();

        Ok(state)
    }
}

// LedgerState
// ----------------------------------------------------------------------------

// The 'LedgerState' trait materializes the interface required of the consensus layer in order to
// validate block headers. It allows to keep the ledger implementation rather abstract to the
// consensus in order to decouple both components.
impl<S: Store> amaru_ouroboros::ledger::LedgerState for State<S> {
    fn pool_id_to_sigma(
        &self,
        _pool_id: &PoolId,
    ) -> Result<PoolSigma, amaru_ouroboros::ledger::Error> {
        // FIXME: Obtain from ledger's stake distribution
        Err(amaru_ouroboros::ledger::Error::PoolIdNotFound)
    }

    // FIXME: This method most probably needs pool from the mark or set snapshots (so one or two
    // epochs in the past), and not from the live view.
    fn vrf_vkey_hash(&self, pool_id: &PoolId) -> Result<Hash<32>, amaru_ouroboros::ledger::Error> {
        self.get_pool(pool_id)
            .unwrap_or_else(|e| panic!("unable to fetch pool from database: {e:?}"))
            .map(|params| params.vrf)
            .ok_or(amaru_ouroboros::ledger::Error::PoolIdNotFound)
    }

    fn slot_to_kes_period(&self, slot: u64) -> u64 {
        // FIXME: Extract from genesis configuration.
        let slots_per_kes_period: u64 = 129600;
        slot / slots_per_kes_period
    }

    fn max_kes_evolutions(&self) -> u64 {
        // FIXME: Extract from genesis configuration.
        62
    }

    fn latest_opcert_sequence_number(&self, _issuer_vkey: &Hash<28>) -> Option<u64> {
        // FIXME: Obtain from protocol's state
        None
    }
}

// FailedTransactions
// ----------------------------------------------------------------------------

/// Failed transactions aren'y immediately available in blocks. Only indices of those transactions
/// are stored. This internal structure provides a clean(er) interface to accessing those indices.
struct FailedTransactions {
    inner: BTreeSet<u32>,
}

impl FailedTransactions {
    pub fn from_block(block: &MintedBlock<'_>) -> Self {
        FailedTransactions {
            inner: block
                .invalid_transactions
                .as_deref()
                .map(|indices| {
                    let mut tree = BTreeSet::new();
                    tree.extend(indices.to_vec().as_slice());
                    tree
                })
                .unwrap_or_default(),
        }
    }

    pub fn has(&self, ix: u32) -> bool {
        self.inner.contains(&ix)
    }
}

// Errors
// ----------------------------------------------------------------------------

#[derive(Debug, Error)]
pub enum BackwardError {
    /// The ledger has been instructed to rollback to an unknown point. This should be impossible
    /// if chain-sync messages (roll-forward and roll-backward) are all passed to the ledger.
    #[error("error rolling back to unknown {0:?}")]
    UnknownRollbackPoint(Point),
}

#[derive(Debug, Error)]
pub enum StateError {
    #[error("error processing query")]
    Query(#[source] StoreError),
    #[error("unable to open database snapshot for epoch {1}: {0}")]
    EpochAccess(u64, #[source] StoreError),
    #[error("error accessing storage")]
    Storage(#[source] StoreError),
}
