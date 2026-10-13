// Copyright 2026 PRAGMA
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

//! Access to the node's existing transaction pool.

use std::{
    fmt,
    future::Future,
    sync::{Arc, Weak},
    time::Duration,
};

use amaru_kernel::{Transaction, cbor::WithOriginalBytes};
use amaru_mempool::{
    InMemoryMempool,
    inspection::{Generation, PoolChange, PoolReceiver, PoolSnapshot},
};
use amaru_ouroboros::{MempoolMsg, TxInsertResult, TxOrigin};
use amaru_protocols::tx_submission::DEFAULT_MEMPOOL_INSERT_TIMEOUT;
use amaru_pure_stage::{BoxFuture, CallError, Sender};
use futures_util::{FutureExt, future::Shared};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

type Pool = InMemoryMempool<WithOriginalBytes<Transaction>>;
type Closed = Shared<BoxFuture<'static, ()>>;

pub(crate) struct MempoolRuntime {
    run_id: NodeRunId,
    pool: Arc<Pool>,
    sender: Sender<MempoolMsg>,
    closing: CancellationToken,
    closed: Closed,
}

impl MempoolRuntime {
    pub(crate) fn new(
        pool: Arc<Pool>,
        sender: Sender<MempoolMsg>,
        termination: BoxFuture<'static, ()>,
    ) -> anyhow::Result<Arc<Self>> {
        let closing = CancellationToken::new();
        let cancelled = closing.clone().cancelled_owned();
        let closed = async move {
            tokio::select! {
                () = cancelled => {},
                () = termination => {},
            }
        }
        .boxed()
        .shared();
        Ok(Arc::new(Self { run_id: NodeRunId::new()?, pool, sender, closing, closed }))
    }

    pub(crate) fn services(self: &Arc<Self>) -> MempoolServices {
        MempoolServices { runtime: Arc::downgrade(self), closed: self.closed.clone() }
    }

    fn snapshot(&self) -> MempoolSnapshot {
        self.pool.snapshot().map_generation(|revision| NodeGeneration { run_id: self.run_id, revision })
    }

    pub(crate) fn close(&self) {
        self.closing.cancel();
    }
}

impl Drop for MempoolRuntime {
    fn drop(&mut self) {
        self.close();
    }
}

/// Identity of one node run, independent of store paths and membership generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct NodeRunId([u8; 16]);

impl NodeRunId {
    fn new() -> anyhow::Result<Self> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("creating node run identity: {error}"))?;
        Ok(Self(bytes))
    }
}

impl fmt::Display for NodeRunId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

/// Membership revision scoped to one node run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct NodeGeneration {
    pub run_id: NodeRunId,
    /// Revalidation without insertions or removals does not advance the revision.
    pub revision: u64,
}

impl Generation for NodeGeneration {
    fn next(&self) -> Self {
        Self { run_id: self.run_id, revision: self.revision.next() }
    }
}

pub use amaru_mempool::inspection::MempoolSnapshotEntry;

/// Atomically captured stored pool membership and counters.
///
/// Ledger changes, including fork switches with rollback, trigger asynchronous revalidation.
/// A snapshot may be taken while revalidation is pending or running and include transactions
/// that will subsequently be removed. Membership establishes "in the local mempool";
/// it does not promise validity against the latest ledger tip, future inclusion, or confirmation.
pub type MempoolSnapshot = PoolSnapshot<NodeGeneration>;

/// One complete membership generation within a node run.
pub type MempoolEvent = PoolChange<NodeGeneration>;

/// API failure. A timeout or shutdown after dispatch does not establish whether insertion happened.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MempoolError {
    #[error("node is closing or stopped")]
    Closed,
    #[error("missed {0} mempool changes; subscribe again for a fresh snapshot")]
    Lagged(u64),
    #[error("transaction input is too large: {size_bytes} bytes exceeds the {max_bytes}-byte limit")]
    InputTooLarge { size_bytes: usize, max_bytes: usize },
    #[error("Invalid CBOR transaction: {0}")]
    InvalidCbor(String),
    #[error(transparent)]
    Call(#[from] CallError),
}

/// Inspection, subscription, and submission for one node run.
/// Retaining this handle does not retain the runtime or stores.
#[derive(Clone)]
pub struct MempoolServices {
    runtime: Weak<MempoolRuntime>,
    closed: Closed,
}

impl MempoolServices {
    /// Defensive CBOR parsing ceiling, independent of the ledger transaction-size limit.
    pub const MAX_INPUT_SIZE_BYTES: usize = 64 * 1024;

    fn runtime(&self) -> Result<Arc<MempoolRuntime>, MempoolError> {
        if self.closed.clone().now_or_never().is_some() {
            return Err(MempoolError::Closed);
        }
        self.runtime.upgrade().ok_or(MempoolError::Closed)
    }

    async fn while_running<T>(&self, future: impl Future<Output = Result<T, MempoolError>>) -> Result<T, MempoolError> {
        self.runtime()?;
        let result = tokio::select! {
            biased;
            () = self.closed.clone() => return Err(MempoolError::Closed),
            result = future => result,
        };
        self.runtime()?;
        result
    }

    /// Capture stored membership without waiting for revalidation; see [`MempoolSnapshot`].
    /// Absence does not establish that a submission failed or was never accepted.
    pub fn snapshot(&self) -> Result<MempoolSnapshot, MempoolError> {
        let snapshot = self.runtime()?.snapshot();
        self.runtime()?;
        Ok(snapshot)
    }

    /// Atomically capture membership and subscribe to subsequent changes.
    /// Subscribers share a 64-event buffer. On [`MempoolError::Lagged`], subscribe again.
    /// Events identify transactions, not individual submission attempts.
    pub fn subscribe(&self) -> Result<(MempoolSnapshot, MempoolReceiver), MempoolError> {
        let runtime = self.runtime()?;
        let run_id = runtime.run_id;
        let (snapshot, receiver) = runtime.pool.subscribe();
        self.runtime()?;
        Ok((
            snapshot.map_generation(|revision| NodeGeneration { run_id, revision }),
            MempoolReceiver { run_id, receiver, services: self.clone() },
        ))
    }

    /// Submit original CBOR through the node's existing validation pipeline.
    /// Timing out or dropping this future does not necessarily cancel insertion.
    pub async fn submit(&self, bytes: &[u8]) -> Result<TxInsertResult, MempoolError> {
        self.submit_with_timeout(bytes, DEFAULT_MEMPOOL_INSERT_TIMEOUT.as_duration()).await
    }

    /// Submit with a deadline covering queue admission and response waiting.
    /// Decoding precedes the deadline and is bounded by [`Self::MAX_INPUT_SIZE_BYTES`].
    pub async fn submit_with_timeout(&self, bytes: &[u8], timeout: Duration) -> Result<TxInsertResult, MempoolError> {
        let sender = self.runtime()?.sender.clone();
        if bytes.len() > Self::MAX_INPUT_SIZE_BYTES {
            return Err(MempoolError::InputTooLarge { size_bytes: bytes.len(), max_bytes: Self::MAX_INPUT_SIZE_BYTES });
        }
        let tx = minicbor::decode::<WithOriginalBytes<Transaction>>(bytes)
            .map_err(|error| MempoolError::InvalidCbor(error.to_string()))?;
        self.while_running(async {
            sender
                .call(|caller| MempoolMsg::Insert { tx: Box::new(tx), origin: TxOrigin::Local, caller }, timeout)
                .await
                .map_err(MempoolError::from)
        })
        .await
    }
}

/// Membership receiver owning queued data and lifecycle notification, without retaining stores.
pub struct MempoolReceiver {
    run_id: NodeRunId,
    receiver: PoolReceiver,
    services: MempoolServices,
}

impl MempoolReceiver {
    /// Receive a complete committed generation. Shutdown takes precedence over queued changes.
    pub async fn recv(&mut self) -> Result<MempoolEvent, MempoolError> {
        let change = self
            .services
            .while_running(async {
                self.receiver.recv().await.map_err(|error| match error {
                    RecvError::Closed => MempoolError::Closed,
                    RecvError::Lagged(count) => MempoolError::Lagged(count),
                })
            })
            .await?;
        Ok(Arc::unwrap_or_clone(change).map_generation(|revision| NodeGeneration { run_id: self.run_id, revision }))
    }
}

#[cfg(test)]
mod tests;
