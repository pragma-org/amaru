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
    rewards::{RewardsSummary, StakeDistribution},
    state::volatile_db::{StoreUpdate, VolatileDB, VolatileState},
    store::{columns::*, Store, StoreError},
};
use amaru_kernel::{
    self, epoch_from_slot, DRep, Epoch, Hash, Hasher, MintedBlock, Point, PoolId, Slot,
    StakeCredential, TransactionInput, TransactionOutput, Voter, VotingProcedures,
    CONSENSUS_SECURITY_PARAM, MAX_KES_EVOLUTION, SLOTS_PER_KES_PERIOD, STABILITY_WINDOW,
};
use amaru_ouroboros_traits::{HasStakeDistribution, PoolSummary};
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashSet, VecDeque},
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
    ///
    /// It also contains the latest stake distribution computed from the previous epoch, which we
    /// hold onto the epoch boundary. In the epoch boundary, the stake distribution becomes
    /// available for the leader schedule verification, whereas the stake distribution previously
    /// used for leader schedule is moved as rewards stake.
    rewards_summary: Option<RewardsSummary>,

    /// A (shared) collection of the latest stake distributions. Those are used both during rewards
    /// calculations, and for leader schedule verification.
    ///
    /// TODO: StakeDistribution are relatively large objects that typically present a lot of
    /// duplications. We won't usually store more than 3 of them at the same time, since we get rid
    /// of them when no longer needed (after rewards calculations).
    ///
    /// Yet, we could imagine a more compact representation where keys for pool and accounts
    /// wouldn't be so much duplicated between snapshots. Instead, we could use an array of values
    /// for each key. On a distribution of 1M+ stake credentials, that's ~26MB of memory per
    /// duplicate.
    stake_distributions: Arc<Mutex<VecDeque<StakeDistribution>>>,
}

fn select_stake_credentials(voting_procedures: &VotingProcedures) -> HashSet<StakeCredential> {
    voting_procedures
        .iter()
        .filter_map(|(k, _)| match k {
            Voter::DRepKey(hash) => Some(StakeCredential::AddrKeyhash(*hash)),
            Voter::DRepScript(hash) => Some(StakeCredential::ScriptHash(*hash)),
            Voter::ConstitutionalCommitteeKey(..)
            | Voter::ConstitutionalCommitteeScript(..)
            | Voter::StakePoolKey(..) => None,
        })
        .collect()
}

fn as_stake_credential(drep: &DRep) -> Option<StakeCredential> {
    match drep {
        DRep::Key(hash) => Some(StakeCredential::AddrKeyhash(*hash)),
        DRep::Script(hash) => Some(StakeCredential::ScriptHash(*hash)),
        DRep::Abstain | DRep::NoConfidence => None,
    }
}

impl<S: Store> State<S> {
    #[allow(clippy::unwrap_used)]
    pub fn new(stable: Arc<Mutex<S>>) -> Self {
        let db = stable.lock().unwrap();

        // NOTE: Initialize stake distribution held in-memory. The one before last is needed by the
        // consensus layer to validate the leader schedule, while the one before that will be
        // consumed for the rewards calculation.
        //
        // We always hold on two stake distributions:
        //
        // - The one from an epoch `e - 1` which is used for the ongoing leader schedule at epoch `e + 1`
        // - The one from an epoch `e - 2` which is used for the rewards calculations at epoch `e + 1`
        //
        // Note that the most recent snapshot we have is necessarily `e`, since `e + 1` designates
        // the ongoing epoch, not yet finished (and so, not available as snapshot).
        let latest_epoch = db.most_recent_snapshot();

        let mut stake_distributions = VecDeque::new();
        #[allow(clippy::panic)]
        for epoch in latest_epoch - 2..=latest_epoch - 1 {
            stake_distributions.push_front(recover_stake_distribution(&*db, epoch).unwrap_or_else(
                |e| {
                    // TODO deal with error
                    panic!(
                        "unable to get stake distribution for (epoch={:?}): {e:?}",
                        epoch
                    )
                },
            ));
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

            stake_distributions: Arc::new(Mutex::new(stake_distributions)),
        }
    }

    /// Obtain a view of the stake distribution, to allow decoupling the ledger from other
    /// components that require access to it.
    pub fn view_stake_distribution(&self) -> impl HasStakeDistribution {
        StakeDistributionView {
            view: self.stake_distributions.clone(),
        }
    }

    /// Inspect the tip of this ledger state. This corresponds to the point of the latest block
    /// applied to the ledger.
    #[allow(clippy::panic)]
    #[allow(clippy::unwrap_used)]
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
    #[allow(clippy::unwrap_used)]
    pub fn forward(
        &mut self,
        span: &Span,
        point: &Point,
        block: MintedBlock<'_>,
    ) -> Result<(), StateError> {
        let issuer = Hasher::<224>::hash(&block.header.header_body.issuer_vkey[..]);
        let relative_slot = amaru_kernel::relative_slot(point.slot_or_default());

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

            let unregistered_dreps = now_stable.state.dreps.unregistered.clone();

            let StoreUpdate {
                point: stable_point,
                issuer: stable_issuer,
                fees,
                add,
                remove,
                withdrawals,
                voting_dreps,
            } = now_stable.into_store_update();

            trace_span!(target: EVENT_TARGET, parent: span, "save").in_scope(|| {
                db.save(
                    &stable_point,
                    Some(&stable_issuer),
                    add,
                    remove,
                    withdrawals,
                    voting_dreps,
                )
                .and_then(|()| {
                    db.with_pots(|mut row| {
                        row.borrow_mut().fees += fees;
                    })
                })
                .and_then(|()| {
                    // Go over all accounts and remove the DRep if it's unregistered
                    db.with_accounts(|iterator| {
                        for (_, mut row) in iterator {
                            if let Some(account) = row.borrow_mut() {
                                if let Some(Some(credential)) =
                                    account.drep.as_ref().map(as_stake_credential)
                                {
                                    if unregistered_dreps.contains(&credential) {
                                        account.drep = None;
                                    }
                                }
                            }
                        }
                    })
                })
                .map_err(StateError::Storage)
            })?;

            // Once we reach the stability window,
            if self.rewards_summary.is_none() && relative_slot >= STABILITY_WINDOW as u64 {
                let mut stake_distributions = self.stake_distributions.lock().unwrap();
                let stake_distribution = stake_distributions
                    .pop_back()
                    .ok_or(StateError::StakeDistributionNotAvailableForRewards)?;
                let epoch = stake_distribution.epoch + 2;
                self.rewards_summary = Some(
                    #[allow(clippy::panic)]
                    RewardsSummary::new(
                        &db.for_epoch(epoch).unwrap_or_else(|e| {
                            panic!(
                                "unable to open database snapshot for epoch {:?}: {:?}",
                                epoch, e
                            )
                        }),
                        stake_distribution,
                    )
                    .map_err(StateError::Storage)?,
                );

                stake_distributions.push_front(
                    recover_stake_distribution(&*db, current_epoch - 1)
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

    #[allow(clippy::panic)]
    #[allow(clippy::unwrap_used)]
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

            let voting_dreps = transaction_body
                .voting_procedures
                .as_ref()
                .map(select_stake_credentials)
                .unwrap_or_default();
            state.voting_dreps.extend(voting_dreps);

            // TODO: Calculate votes for all pending  voting procedures per block ?

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

#[allow(clippy::panic)]
fn recover_stake_distribution(
    db: &impl Store,
    epoch: Epoch,
) -> Result<StakeDistribution, StoreError> {
    let snapshot = db.for_epoch(epoch).unwrap_or_else(|e| {
        panic!(
            "unable to open database snapshot for epoch {:?}: {:?}",
            epoch, e
        )
    });
    StakeDistribution::new(&snapshot)
}

// HasStakeDistribution
// ----------------------------------------------------------------------------

// The 'LedgerState' trait materializes the interface required of the consensus layer in order to
// validate block headers. It allows to keep the ledger implementation rather abstract to the
// consensus in order to decouple both components.
pub struct StakeDistributionView {
    view: Arc<Mutex<VecDeque<StakeDistribution>>>,
}

impl HasStakeDistribution for StakeDistributionView {
    #[allow(clippy::unwrap_used)]
    fn get_pool(&self, slot: Slot, pool: &PoolId) -> Option<PoolSummary> {
        let view = self.view.lock().unwrap();
        let epoch = epoch_from_slot(slot) - 2;
        view.iter().find(|s| s.epoch == epoch).and_then(|s| {
            s.pools.get(pool).map(|st| PoolSummary {
                vrf: st.parameters.vrf,
                stake: st.stake,
                active_stake: s.active_stake,
            })
        })
    }

    fn slot_to_kes_period(&self, slot: u64) -> u64 {
        slot / SLOTS_PER_KES_PERIOD
    }

    fn max_kes_evolutions(&self) -> u64 {
        MAX_KES_EVOLUTION as u64
    }

    fn latest_opcert_sequence_number(&self, _issuer_vkey: &Hash<28>) -> Option<u64> {
        // FIXME: Move this responsibility to the consensus layer
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
    #[error("no stake distribution available for rewards calculation.")]
    StakeDistributionNotAvailableForRewards,
}
