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
    collections::BTreeSet,
    net::SocketAddr,
    sync::{Arc, mpsc::sync_channel},
    thread,
};

use amaru_kernel::{Block, Point, Transaction};
use amaru_ledger::{
    rules::block::BlockValidation,
    state::State,
    store::{HistoricalStores, Store},
};
use amaru_metrics::ledger::LedgerMetrics;
use amaru_ouroboros_traits::{
    CanValidateBlocks, CanValidateTxs, ChainStore, ForkSwitchOutcome, HasStakePools, PoolSummaries,
    TransactionValidationError, can_validate_blocks::BlockValidationError,
};
use amaru_plutus::arena_pool::ArenaPool;
use anyhow::anyhow;
use tokio::sync::{mpsc::UnboundedSender, oneshot};

/// Requests serviced by the ledger thread. Responses travel back through the channel embedded in
/// each variant: an awaitable oneshot for block application, and a rendezvous channel for the
/// operations exposed through synchronous trait methods.
enum LedgerRequest {
    RollForwardBlock(
        Box<Block>,
        oneshot::Sender<Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError>>,
    ),
    SwitchToFork(Point, Point, std::sync::mpsc::SyncSender<Result<ForkSwitchOutcome, BlockValidationError>>),
    Tip(std::sync::mpsc::SyncSender<Point>),
    VolatileTip(std::sync::mpsc::SyncSender<Option<Point>>),
    ValidateTx(Box<Transaction>, std::sync::mpsc::SyncSender<Result<(), TransactionValidationError>>),
    RegisteredRelaySocketAddrs(std::sync::mpsc::SyncSender<Result<BTreeSet<SocketAddr>, BlockValidationError>>),
    SetOnStakeDistUpdated(Arc<dyn Fn(PoolSummaries) + Send + Sync>),
}

/// Handle to the ledger state, which lives on a dedicated thread owning it exclusively.
///
/// Ledger operations are requested by sending a message to that thread and waiting for its
/// response, so all of them execute sequentially regardless of how many entities (block
/// validation, mempool, ...) hold a handle. The thread terminates once every handle is dropped.
#[derive(Clone)]
pub struct BlockValidator {
    sender: UnboundedSender<LedgerRequest>,
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
        let (sender, mut receiver) = tokio::sync::mpsc::unbounded_channel();
        thread::Builder::new().name("ledger".into()).spawn(move || {
            let mut ledger = LedgerThread { state, vm_eval_pool, chain_store };
            while let Some(request) = receiver.blocking_recv() {
                ledger.handle(request);
            }
        })?;
        Ok(Self { sender })
    }

    /// Set callback invoked when a new stake distribution is computed/available.
    /// The provided PoolSummaries should be used to update resources for header validation.
    pub fn set_on_stake_dist_updated(&self, callback: Arc<dyn Fn(PoolSummaries) + Send + Sync>) {
        let _ = self.sender.send(LedgerRequest::SetOnStakeDistUpdated(callback));
    }

    fn request<T>(&self, make: impl FnOnce(std::sync::mpsc::SyncSender<T>) -> LedgerRequest) -> Result<T, ThreadGone> {
        let (reply, response) = sync_channel(1);
        self.sender.send(make(reply)).map_err(|_| ThreadGone)?;
        response.recv().map_err(|_| ThreadGone)
    }
}

#[derive(Debug)]
struct ThreadGone;

impl From<ThreadGone> for BlockValidationError {
    fn from(_: ThreadGone) -> Self {
        BlockValidationError::new(anyhow!("the ledger thread has terminated"))
    }
}

impl From<ThreadGone> for TransactionValidationError {
    fn from(_: ThreadGone) -> Self {
        TransactionValidationError::from(anyhow!("the ledger thread has terminated"))
    }
}

impl CanValidateTxs for BlockValidator {
    fn validate_tx(&self, tx: &Transaction) -> Result<(), TransactionValidationError> {
        self.request(|reply| LedgerRequest::ValidateTx(Box::new(tx.clone()), reply))?
    }
}

#[async_trait::async_trait]
impl CanValidateBlocks for BlockValidator {
    // NOTE: Requests execute once queued
    //
    // If this future is dropped after the request was sent, the ledger thread still
    // applies the block and discards the result. The ledger state remains consistent;
    // consensus re-intersects from the ledger tip on the next startup.
    async fn roll_forward_block(
        &self,
        block: Block,
    ) -> Result<Result<LedgerMetrics, BlockValidationError>, BlockValidationError> {
        let (reply, response) = oneshot::channel();
        self.sender
            .send(LedgerRequest::RollForwardBlock(Box::new(block), reply))
            .map_err(|_| BlockValidationError::from(ThreadGone))?;
        response.await.map_err(|_| BlockValidationError::from(ThreadGone))?
    }

    fn switch_to_fork(&self, fork_point: &Point, to: &Point) -> Result<ForkSwitchOutcome, BlockValidationError> {
        self.request(|reply| LedgerRequest::SwitchToFork(*fork_point, *to, reply))?
    }

    #[expect(clippy::expect_used)]
    fn tip(&self) -> Point {
        self.request(LedgerRequest::Tip).expect("the ledger thread has terminated")
    }

    #[expect(clippy::expect_used)]
    fn volatile_tip(&self) -> Option<Point> {
        self.request(LedgerRequest::VolatileTip).expect("the ledger thread has terminated")
    }
}

impl HasStakePools for BlockValidator {
    fn registered_relay_socket_addrs(&self) -> Result<BTreeSet<SocketAddr>, BlockValidationError> {
        self.request(LedgerRequest::RegisteredRelaySocketAddrs)?
    }
}

struct LedgerThread<S: Store, HS: HistoricalStores> {
    state: State<S, HS>,
    vm_eval_pool: ArenaPool,
    chain_store: Arc<dyn ChainStore>,
}

impl<S: Store + Send, HS: HistoricalStores + Send + Sync + 'static> LedgerThread<S, HS> {
    fn handle(&mut self, request: LedgerRequest) {
        match request {
            LedgerRequest::RollForwardBlock(block, reply) => {
                let _ = reply.send(self.roll_forward_block(&block));
            }
            LedgerRequest::SwitchToFork(fork_point, to, reply) => {
                let _ = reply.send(self.switch_to_fork(&fork_point, &to));
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
            LedgerRequest::RegisteredRelaySocketAddrs(reply) => {
                let _ = reply.send(
                    self.state.registered_relay_socket_addrs().map_err(|e| BlockValidationError::new(anyhow!(e))),
                );
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

    fn switch_to_fork(&mut self, fork_point: &Point, to: &Point) -> Result<ForkSwitchOutcome, BlockValidationError> {
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

        // Now switch the fork in the ledger by rolling back to the fork point and then rolling forward the blocks to the new tip.
        self.state
            .switch_to_fork(fork_point, forward_blocks, &self.vm_eval_pool)
            .map_err(|err| BlockValidationError::from(anyhow!(err)))
    }
}
