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

use std::{
    any::Any,
    collections::BTreeSet,
    sync::{Arc, Weak},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use amaru_kernel::{Block, PeerCandidate, Point, Transaction};
use amaru_ledger::{
    rules::block::BlockValidation,
    state::State,
    store::{HistoricalStores, Store},
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_observability::warn;
use amaru_ouroboros_traits::{
    CanValidateBlocks, CanValidateTxs, ChainStore, ForkSwitchOutcome, HasStakePools, LedgerThreadTerminated,
    PoolSummaries, TransactionValidationError, can_validate_blocks::BlockValidationError,
};
use amaru_uplc::arena_pool::ArenaPool;
use anyhow::anyhow;
use parking_lot::Mutex;
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};

/// How often [`LedgerThreadStop::join_timeout`] polls [`JoinHandle::is_finished`].
const JOIN_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Upper bound on requests waiting for the ledger thread. Sized by counting the request
/// producers — block validation (serialized by `SelectChain`), fork switches, mempool
/// transaction validation, the submission API, and occasional tip or relay lookups —
/// and doubling the estimate. Exceeding it means a producer went out of bounds, which is
/// treated as a bug rather than a back-pressure condition.
const REQUEST_QUEUE_BOUND: usize = 16;

/// Requests serviced by the ledger thread, each carrying the oneshot channel through which
/// its response travels back to the requester.
enum LedgerRequest {
    RollForwardBlock(
        Box<Block>,
        oneshot::Sender<Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>>,
    ),
    SwitchToFork(Point, Vec<Block>, oneshot::Sender<Result<ForkSwitchOutcome, BlockValidationError>>),
    Tip(oneshot::Sender<Point>),
    VolatileTip(oneshot::Sender<Option<Point>>),
    ValidateTx(Box<Transaction>, oneshot::Sender<Result<(), TransactionValidationError>>),
    RegisteredRelayCandidates(oneshot::Sender<Result<BTreeSet<PeerCandidate>, BlockValidationError>>),
    SetOnStakeDistUpdated(Arc<dyn Fn(PoolSummaries) + Send + Sync>),
}

/// Worker activity reported when ledger cleanup is still pending.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LedgerThreadPhase {
    #[default]
    WaitingForRequests,
    ValidatingBlock,
    SwitchingFork,
    ReadingTip,
    ReadingVolatileTip,
    ValidatingTransaction,
    ReadingRelayCandidates,
    UpdatingStakeDistributionCallback,
    JoiningRewards,
    ReleasingStores,
}

/// Wait-handle for the ledger worker spawned by [`BlockValidator::new`].
///
/// Clones share the worker. Joining closes request intake
/// and drains accepted work; timeout errors retain a handle for a later join.
#[derive(Clone, Debug)]
#[cfg_attr(test, derive(Default))]
pub struct LedgerThreadStop {
    /// The handle and cached panic share one lock so concurrent joins see the same outcome.
    join: Arc<Mutex<(Option<JoinHandle<()>>, Option<String>)>>,
    phase: Arc<Mutex<LedgerThreadPhase>>,
    sender: Weak<Mutex<Option<mpsc::Sender<LedgerRequest>>>>,
}

/// Failure from [`LedgerThreadStop::join_timeout`].
#[derive(Clone, Debug, Error)]
pub enum LedgerThreadJoinError {
    #[error("timed out after {timeout:?} waiting for the ledger thread: {phase:?}; stores may still be active")]
    Timeout { timeout: Duration, phase: LedgerThreadPhase, ledger_thread: LedgerThreadStop },
    #[error("ledger thread panicked: {0}")]
    Panicked(String),
}

impl LedgerThreadStop {
    /// Close intake and wait up to `timeout` for accepted work and ledger cleanup.
    /// Timeouts retain ownership for retry; repeated joins return the same completion or panic.
    pub fn join_timeout(&self, timeout: Duration) -> Result<(), LedgerThreadJoinError> {
        let started = Instant::now();
        drop(self.sender.upgrade().and_then(|sender| sender.lock().take()));
        let timed_out =
            || LedgerThreadJoinError::Timeout { timeout, phase: *self.phase.lock(), ledger_thread: self.clone() };
        let mut join = self.join.try_lock_for(timeout.saturating_sub(started.elapsed())).ok_or_else(timed_out)?;
        let (handle, panic) = &mut *join;
        let mut notified = false;
        while handle.as_ref().is_some_and(|handle| !handle.is_finished()) {
            let elapsed = started.elapsed();
            if elapsed >= timeout {
                return Err(timed_out());
            }
            if !notified && elapsed >= Duration::from_secs(10) {
                warn!(
                    setup::lifecycle::LEDGER_STILL_CLOSING,
                    status = "Still closing the ledger…",
                    phase = format!("{:?}", *self.phase.lock())
                );
                notified = true;
            }
            thread::sleep(timeout.saturating_sub(started.elapsed()).min(JOIN_POLL_INTERVAL));
        }
        if let Some(handle) = handle.take() {
            *panic = handle.join().err().map(panic_message);
        }
        panic.clone().map_or(Ok(()), |message| Err(LedgerThreadJoinError::Panicked(message)))
    }
}

fn panic_message(payload: Box<dyn Any + Send>) -> String {
    match payload.downcast::<String>() {
        Ok(message) => *message,
        Err(payload) => match payload.downcast::<&'static str>() {
            Ok(message) => (*message).to_string(),
            Err(_) => "unknown panic payload".to_string(),
        },
    }
}

/// Handle to the ledger state, which lives on a dedicated thread owning it exclusively.
///
/// Ledger operations are requested by sending a message to that thread and awaiting its
/// response, so all of them execute sequentially regardless of how many entities (block
/// validation, mempool, ...) hold a handle. Chain-store lookups happen on the caller side,
/// before a request is sent, so the ledger thread touches nothing but the ledger state.
/// The thread terminates when all handles are dropped or [`LedgerThreadStop::join_timeout`] closes request intake.
#[derive(Clone)]
pub struct BlockValidator {
    sender: Arc<Mutex<Option<mpsc::Sender<LedgerRequest>>>>,
    chain_store: Arc<dyn ChainStore>,
    stop: LedgerThreadStop,
}

impl BlockValidator {
    pub fn new<S, HS>(
        state: State<S, HS>,
        vm_eval_pool: ArenaPool,
        chain_store: Arc<dyn ChainStore>,
    ) -> std::io::Result<Self>
    where
        S: Store + Send + 'static,
        HS: HistoricalStores + Send + 'static,
    {
        let (sender, mut receiver) = mpsc::channel(REQUEST_QUEUE_BOUND);
        let phase = Arc::new(Mutex::new(LedgerThreadPhase::WaitingForRequests));
        let worker_phase = phase.clone();
        // The ledger thread does not inherit a thread-local subscriber. Keep the dispatch that
        // was current when the node was built, so each simulated node keeps its own log.
        let dispatch = tracing::dispatcher::get_default(|dispatch| dispatch.clone());
        let join = thread::Builder::new().name("ledger".into()).spawn(move || {
            tracing::dispatcher::with_default(&dispatch, || {
                let mut ledger = LedgerThread { state, vm_eval_pool };
                while let Some(request) = receiver.blocking_recv() {
                    ledger.handle(request, &worker_phase);
                    *worker_phase.lock() = LedgerThreadPhase::WaitingForRequests;
                }
                *worker_phase.lock() = LedgerThreadPhase::JoiningRewards;
                ledger.state.shutdown_rewards();
                *worker_phase.lock() = LedgerThreadPhase::ReleasingStores;
            });
        })?;
        let sender = Arc::new(Mutex::new(Some(sender)));
        let stop =
            LedgerThreadStop { join: Arc::new(Mutex::new((Some(join), None))), phase, sender: Arc::downgrade(&sender) };
        Ok(Self { sender, chain_store, stop })
    }

    /// Shared stop-and-join handle for the ledger worker.
    pub fn thread_stop(&self) -> LedgerThreadStop {
        self.stop.clone()
    }

    /// Set callback invoked when a new stake distribution is computed/available.
    /// The provided PoolSummaries should be used to update resources for header validation.
    pub fn set_on_stake_dist_updated(&self, callback: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        self.send(LedgerRequest::SetOnStakeDistUpdated(callback)).unwrap_or(())
    }

    fn send(&self, request: LedgerRequest) -> Result<(), LedgerThreadTerminated> {
        let sender = self.sender.lock();
        let sender = sender.as_ref().ok_or(LedgerThreadTerminated)?;
        match sender.try_send(request) {
            Ok(()) => Ok(()),
            #[expect(clippy::panic)]
            Err(mpsc::error::TrySendError::Full(_)) => {
                panic!("the ledger request queue exceeded its bound of {REQUEST_QUEUE_BOUND}")
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(LedgerThreadTerminated),
        }
    }

    async fn request<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> LedgerRequest,
    ) -> Result<T, LedgerThreadTerminated> {
        let (reply, response) = oneshot::channel();
        self.send(make(reply))?;
        response.await.map_err(|_| LedgerThreadTerminated)
    }
}

fn terminated() -> BlockValidationError {
    BlockValidationError::new(anyhow::Error::new(LedgerThreadTerminated))
}

#[async_trait::async_trait]
impl CanValidateTxs for BlockValidator {
    async fn validate_tx(&self, tx: &Transaction) -> Result<(), TransactionValidationError> {
        self.request(|reply| LedgerRequest::ValidateTx(Box::new(tx.clone()), reply))
            .await
            .map_err(|e| TransactionValidationError::from(anyhow::Error::new(e)))?
    }
}

#[async_trait::async_trait]
impl CanValidateBlocks for BlockValidator {
    // NOTE: Requests execute once queued
    //
    // If a caller's future is dropped after the request was sent, the ledger thread still
    // processes the request and discards the result. The ledger state remains consistent;
    // consensus re-intersects from the ledger tip on the next startup.
    async fn roll_forward_block(
        &self,
        block: Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        self.request(|reply| LedgerRequest::RollForwardBlock(Box::new(block), reply)).await.map_err(|_| terminated())?
    }

    async fn switch_to_fork(&self, fork_point: &Point, to: &Point) -> Result<ForkSwitchOutcome, BlockValidationError> {
        // Get all the headers of the block to apply between the fork point and the expected new tip
        let Some(forward_tips) = self.chain_store.ancestors_between(fork_point, to.hash()) else {
            return Err(BlockValidationError::new(anyhow!(
                "the stored headers do not form a chain from {fork_point} to {to}"
            )));
        };

        // Load the blocks corresponding to the headers to apply, in order, from the chain store.
        let forward_blocks = forward_tips
            .iter()
            .map(|forward_tip| {
                self.chain_store
                    .load_block(&forward_tip.hash())
                    .map_err(|e| BlockValidationError::new(anyhow!(e)))?
                    .ok_or_else(|| BlockValidationError::new(anyhow!("block not found")))?
                    .decode()
                    .map_err(|e| BlockValidationError::new(anyhow!(e)))
            })
            .collect::<Result<Vec<_>, _>>()?;

        if forward_blocks.is_empty() {
            // We are not supposed to switch to a fork that would be a no-op for the ledger.
            return Err(BlockValidationError::new(anyhow!("block already applied to the ledger: {to}")));
        }

        self.request(|reply| LedgerRequest::SwitchToFork(*fork_point, forward_blocks, reply))
            .await
            .map_err(|_| terminated())?
    }

    #[expect(clippy::expect_used)]
    async fn tip(&self) -> Point {
        self.request(LedgerRequest::Tip).await.expect("the ledger thread has terminated")
    }

    #[expect(clippy::expect_used)]
    async fn volatile_tip(&self) -> Option<Point> {
        self.request(LedgerRequest::VolatileTip).await.expect("the ledger thread has terminated")
    }
}

#[async_trait::async_trait]
impl HasStakePools for BlockValidator {
    async fn registered_relay_candidates(&self) -> Result<BTreeSet<PeerCandidate>, BlockValidationError> {
        self.request(LedgerRequest::RegisteredRelayCandidates).await.map_err(|_| terminated())?
    }
}

struct LedgerThread<S: Store, HS: HistoricalStores> {
    state: State<S, HS>,
    vm_eval_pool: ArenaPool,
}

impl<S: Store + Send, HS: HistoricalStores + Send + 'static> LedgerThread<S, HS> {
    fn handle(&mut self, request: LedgerRequest, phase: &Mutex<LedgerThreadPhase>) {
        match request {
            LedgerRequest::RollForwardBlock(block, reply) => {
                *phase.lock() = LedgerThreadPhase::ValidatingBlock;
                let _ = reply.send(self.roll_forward_block(&block));
            }
            LedgerRequest::SwitchToFork(fork_point, blocks, reply) => {
                *phase.lock() = LedgerThreadPhase::SwitchingFork;
                let _ = reply.send(
                    self.state
                        .switch_to_fork(&fork_point, blocks, &self.vm_eval_pool)
                        .map_err(|err| BlockValidationError::from(anyhow!(err))),
                );
            }
            LedgerRequest::Tip(reply) => {
                *phase.lock() = LedgerThreadPhase::ReadingTip;
                let _ = reply.send(self.state.tip().into_owned());
            }
            LedgerRequest::VolatileTip(reply) => {
                *phase.lock() = LedgerThreadPhase::ReadingVolatileTip;
                let _ = reply.send(self.state.volatile_tip());
            }
            LedgerRequest::ValidateTx(tx, reply) => {
                *phase.lock() = LedgerThreadPhase::ValidatingTransaction;
                let _ = reply.send(self.validate_tx(&tx));
            }
            LedgerRequest::RegisteredRelayCandidates(reply) => {
                *phase.lock() = LedgerThreadPhase::ReadingRelayCandidates;
                let _ = reply
                    .send(self.state.registered_relay_candidates().map_err(|e| BlockValidationError::new(anyhow!(e))));
            }
            LedgerRequest::SetOnStakeDistUpdated(callback) => {
                *phase.lock() = LedgerThreadPhase::UpdatingStakeDistributionCallback;
                self.state.set_on_stake_dist_updated(callback);
            }
        }
    }

    fn roll_forward_block(
        &mut self,
        block: &Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        match self.state.roll_forward(block, &self.vm_eval_pool) {
            BlockValidation::Valid(metrics) => Ok(Ok(metrics)),
            BlockValidation::Invalid(_, details) => {
                Ok(Err(BlockValidationError::new(anyhow!("Invalid block: {details}"))))
            }
            BlockValidation::Err(err) => Err(BlockValidationError::new(anyhow!(err))),
        }
    }

    fn validate_tx(&self, tx: &Transaction) -> Result<(), TransactionValidationError> {
        self.state
            .validate_tx(tx, self.state.tip().slot_or_default(), &self.vm_eval_pool)
            .map_err(|error| TransactionValidationError::from(anyhow!(error)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_timeout_succeeds_when_thread_already_exited() {
        let join = thread::spawn(|| {});
        let stop = LedgerThreadStop { join: Arc::new(Mutex::new((Some(join), None))), ..Default::default() };
        stop.join_timeout(Duration::from_secs(1)).expect("join finished thread");
        stop.join_timeout(Duration::from_millis(1)).expect("second join is a no-op");
    }

    #[test]
    fn join_timeout_errors_while_thread_is_still_running() {
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let join = thread::spawn(move || {
            let _ = release_rx.recv();
        });
        let stop = LedgerThreadStop { join: Arc::new(Mutex::new((Some(join), None))), ..Default::default() };
        let err = stop.join_timeout(Duration::from_millis(30)).expect_err("thread still blocked");
        assert!(
            matches!(err, LedgerThreadJoinError::Timeout { timeout, .. } if timeout == Duration::from_millis(30)),
            "{err}"
        );
        assert!(err.to_string().contains("timed out after 30ms"), "timeout error should be explicit, got: {err}");
        drop(release_tx);
    }
}
