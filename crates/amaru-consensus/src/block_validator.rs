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

use std::{collections::BTreeSet, sync::Arc, thread};

use amaru_kernel::{Block, PeerCandidate, Point, Transaction};
use amaru_ledger::{
    rules::block::BlockValidation,
    state::State,
    store::{HistoricalStores, Store},
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    CanValidateBlocks, CanValidateTxs, ChainStore, ForkSwitchOutcome, HasStakePools, LedgerThreadTerminated,
    PoolSummaries, TransactionValidationError, can_validate_blocks::BlockValidationError,
};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::anyhow;
use tokio::sync::{mpsc, oneshot};

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

/// Handle to the ledger state, which lives on a dedicated thread owning it exclusively.
///
/// Ledger operations are requested by sending a message to that thread and awaiting its
/// response, so all of them execute sequentially regardless of how many entities (block
/// validation, mempool, ...) hold a handle. Chain-store lookups happen on the caller side,
/// before a request is sent, so the ledger thread touches nothing but the ledger state.
/// The thread terminates once every handle is dropped.
#[derive(Clone)]
pub struct BlockValidator {
    sender: mpsc::Sender<LedgerRequest>,
    chain_store: Arc<dyn ChainStore>,
}

impl BlockValidator {
    pub fn new<S, HS>(
        state: State<S, HS>,
        vm_eval_pool: ArenaPool,
        chain_store: Arc<dyn ChainStore>,
    ) -> std::io::Result<Self>
    where
        S: Store + Send + 'static,
        HS: HistoricalStores + Send + Sync + 'static,
    {
        let (sender, mut receiver) = mpsc::channel(REQUEST_QUEUE_BOUND);
        thread::Builder::new().name("ledger".into()).spawn(move || {
            let mut ledger = LedgerThread { state, vm_eval_pool };
            while let Some(request) = receiver.blocking_recv() {
                ledger.handle(request);
            }
        })?;
        Ok(Self { sender, chain_store })
    }

    /// Set callback invoked when a new stake distribution is computed/available.
    /// The provided PoolSummaries should be used to update resources for header validation.
    pub fn set_on_stake_dist_updated(&self, callback: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        self.send(LedgerRequest::SetOnStakeDistUpdated(callback)).unwrap_or(())
    }

    fn send(&self, request: LedgerRequest) -> Result<(), LedgerThreadTerminated> {
        match self.sender.try_send(request) {
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

impl<S: Store + Send, HS: HistoricalStores + Send + Sync + 'static> LedgerThread<S, HS> {
    fn handle(&mut self, request: LedgerRequest) {
        match request {
            LedgerRequest::RollForwardBlock(block, reply) => {
                let _ = reply.send(self.roll_forward_block(&block));
            }
            LedgerRequest::SwitchToFork(fork_point, blocks, reply) => {
                let _ = reply.send(
                    self.state
                        .switch_to_fork(&fork_point, blocks, &self.vm_eval_pool)
                        .map_err(|err| BlockValidationError::from(anyhow!(err))),
                );
            }
            LedgerRequest::Tip(reply) => {
                let _ = reply.send(self.state.tip().into_owned());
            }
            LedgerRequest::VolatileTip(reply) => {
                let _ = reply.send(self.state.volatile_tip());
            }
            LedgerRequest::ValidateTx(tx, reply) => {
                let _ = reply.send(self.validate_tx(&tx));
            }
            LedgerRequest::RegisteredRelayCandidates(reply) => {
                let _ = reply
                    .send(self.state.registered_relay_candidates().map_err(|e| BlockValidationError::new(anyhow!(e))));
            }
            LedgerRequest::SetOnStakeDistUpdated(callback) => {
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
