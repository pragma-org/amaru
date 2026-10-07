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

//! Typed services for the node's existing transaction pool.
//!
//! ```no_run
//! # async fn example(running: amaru_node::NodeRunning, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
//! let mempool = running.mempool();
//! let (snapshot, mut changes) = mempool.reader().subscribe()?;
//! let accepted = mempool.submitter().submit(bytes).await?;
//! let next_change = changes.recv().await?;
//! running.shutdown().await?;
//! # Ok(())
//! # }
//! ```

use std::{
    fmt,
    sync::{
        Arc, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use amaru_kernel::{Transaction, TransactionId, cbor::WithOriginalBytes};
use amaru_mempool::{
    InMemoryMempool,
    inspection::{PoolChange, PoolObserverError, PoolReceiver, PoolSnapshot},
};
use amaru_ouroboros::{MempoolMsg, MempoolSeqNo, TxInsertResult, TxOrigin, TxRejectReason};
use amaru_protocols::tx_submission::DEFAULT_MEMPOOL_INSERT_TIMEOUT;
use amaru_pure_stage::{BoxFuture, CallError, Sender};
use futures_util::{FutureExt, future::Shared};
use tokio_util::sync::CancellationToken;

type Pool = InMemoryMempool<WithOriginalBytes<Transaction>>;

/// Identity of one node run, independent of store paths and membership generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct NodeRunId([u8; 16]);

impl NodeRunId {
    pub(crate) fn new() -> anyhow::Result<Self> {
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

/// Complete stored membership at one instant; validity against a newer tip is not implied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MempoolSnapshot {
    pub run_id: NodeRunId,
    pub generation: u64,
    pub entries: Vec<MempoolSnapshotEntry>,
    pub transaction_count: u64,
    pub total_size_bytes: u64,
    pub capacity_bytes: u64,
}

pub use amaru_mempool::inspection::MempoolSnapshotEntry;

impl MempoolSnapshot {
    fn from_pool(run_id: NodeRunId, snapshot: PoolSnapshot) -> Self {
        Self {
            run_id,
            generation: snapshot.generation,
            entries: snapshot.entries,
            transaction_count: snapshot.transaction_count,
            total_size_bytes: snapshot.total_size_bytes,
            capacity_bytes: snapshot.capacity_bytes,
        }
    }
}

/// One complete membership generation within a node run.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum MempoolEvent {
    Inserted { run_id: NodeRunId, generation: u64, entry: MempoolSnapshotEntry },
    Removed { run_id: NodeRunId, generation: u64, transaction_ids: Vec<TransactionId> },
}

/// Inspection, subscription admission, or delivery failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MempoolAccessError {
    #[error("node is closing")]
    Closing,
    #[error("node stopped")]
    Stopped,
    #[error("mempool subscriber limit reached")]
    SubscriberLimit,
    #[error("mempool changes were lost in run {run_id}: expected {expected_generation}, reached {current_generation}")]
    Gap { run_id: NodeRunId, expected_generation: u64, current_generation: u64 },
}

/// Existing stage-call failure, independent of ledger validation rejection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum MempoolUnavailableReason {
    #[error("send failed")]
    SendFailed,
    #[error("response dropped")]
    ResponseDropped,
    #[error("response deserialization failed")]
    ResponseDeserializeFailed,
}

/// Submission failure. After dispatch, timeout, unavailability, and shutdown do not
/// establish whether insertion happened. There is no retained attempt history.
#[derive(Debug, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MempoolSubmitError {
    #[error("Invalid CBOR transaction: {reason}")]
    InvalidCbor { reason: String },
    #[error("{reason}")]
    Rejected { transaction_id: TransactionId, reason: TxRejectReason },
    #[error("mempool timed out; insertion outcome is unknown")]
    Timeout { transaction_id: TransactionId },
    #[error("mempool unavailable: {reason}; insertion outcome may be unknown")]
    Unavailable { transaction_id: TransactionId, reason: MempoolUnavailableReason },
    #[error("node is closing; dispatched insertion outcome may be unknown")]
    Closing { transaction_id: Option<TransactionId> },
    #[error("node stopped; dispatched insertion outcome may be unknown")]
    Stopped { transaction_id: Option<TransactionId> },
}

/// Successful insertion, which does not promise continued membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MempoolAccepted {
    pub transaction_id: TransactionId,
    pub sequence: MempoolSeqNo,
}

struct MempoolStatus {
    closing: CancellationToken,
    stopped: AtomicBool,
    termination: Shared<BoxFuture<'static, ()>>,
}

impl MempoolStatus {
    fn closing_error(&self) -> MempoolAccessError {
        if self.stopped.load(Ordering::Acquire) { MempoolAccessError::Stopped } else { MempoolAccessError::Closing }
    }

    fn check(&self) -> Result<(), MempoolAccessError> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(MempoolAccessError::Stopped);
        }
        if self.closing.is_cancelled() || self.termination.clone().now_or_never().is_some() {
            self.closing.cancel();
            return Err(MempoolAccessError::Closing);
        }
        Ok(())
    }

    async fn wait_for_closing(&self) {
        tokio::select! {
            biased;
            () = self.closing.cancelled() => {},
            () = self.termination.clone() => self.closing.cancel(),
        }
    }

    fn submission_error(&self, transaction_id: Option<TransactionId>) -> MempoolSubmitError {
        if self.stopped.load(Ordering::Acquire) {
            MempoolSubmitError::Stopped { transaction_id }
        } else {
            MempoolSubmitError::Closing { transaction_id }
        }
    }
}

pub(crate) struct MempoolRuntime {
    run_id: NodeRunId,
    pool: Arc<Pool>,
    sender: Sender<MempoolMsg>,
    status: Arc<MempoolStatus>,
}

impl MempoolRuntime {
    pub(crate) fn new(
        run_id: NodeRunId,
        pool: Arc<Pool>,
        sender: Sender<MempoolMsg>,
        termination: BoxFuture<'static, ()>,
    ) -> Arc<Self> {
        Arc::new(Self {
            run_id,
            pool,
            sender,
            status: Arc::new(MempoolStatus {
                closing: CancellationToken::new(),
                stopped: AtomicBool::new(false),
                termination: termination.shared(),
            }),
        })
    }

    pub(crate) fn services(self: &Arc<Self>) -> MempoolServices {
        MempoolServices { reader: self.reader(), submitter: self.submitter() }
    }

    pub(crate) fn reader(self: &Arc<Self>) -> MempoolReader {
        MempoolReader { runtime: Arc::downgrade(self), status: self.status.clone() }
    }

    pub(crate) fn submitter(self: &Arc<Self>) -> MempoolSubmitter {
        MempoolSubmitter { runtime: Arc::downgrade(self), status: self.status.clone() }
    }

    pub(crate) fn close(&self) {
        self.status.closing.cancel();
    }

    pub(crate) fn stop(&self) {
        self.status.stopped.store(true, Ordering::Release);
        self.close();
    }
}

impl Drop for MempoolRuntime {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Access to the read-only and submission services for one node's mempool.
///
/// Retaining these services does not retain the runtime or stores. Inspection and
/// updates are available through [`Self::reader`]; submission through [`Self::submitter`].
#[derive(Clone)]
pub struct MempoolServices {
    reader: MempoolReader,
    submitter: MempoolSubmitter,
}

impl MempoolServices {
    /// Obtain read-only inspection and subscription access.
    pub fn reader(&self) -> MempoolReader {
        self.reader.clone()
    }

    /// Obtain submission access without inspection.
    pub fn submitter(&self) -> MempoolSubmitter {
        self.submitter.clone()
    }
}

/// Read-only service. Retaining it does not retain the runtime or stores.
#[derive(Clone)]
pub struct MempoolReader {
    runtime: Weak<MempoolRuntime>,
    status: Arc<MempoolStatus>,
}

impl MempoolReader {
    /// Capture an owned, capacity-bounded snapshot of stored membership.
    pub fn snapshot(&self) -> Result<MempoolSnapshot, MempoolAccessError> {
        self.status.check()?;
        let runtime = self.runtime.upgrade().ok_or(MempoolAccessError::Stopped)?;
        let snapshot = MempoolSnapshot::from_pool(runtime.run_id, runtime.pool.snapshot());
        drop(runtime);
        self.status.check()?;
        Ok(snapshot)
    }

    /// Atomically register and capture the initial snapshot. The receiver delivers
    /// every later generation or an explicit gap; recover by subscribing again.
    ///
    /// At most 64 subscribers are admitted. Each retains at most 64 changes and
    /// 1 MiB of original transaction bytes and change metadata.
    pub fn subscribe(&self) -> Result<(MempoolSnapshot, MempoolReceiver), MempoolAccessError> {
        self.status.check()?;
        let runtime = self.runtime.upgrade().ok_or(MempoolAccessError::Stopped)?;
        let run_id = runtime.run_id;
        let subscription = runtime.pool.subscribe();
        self.status.check()?;
        let (snapshot, receiver) = subscription.map_err(|error| observer_error(run_id, error))?;
        let snapshot = MempoolSnapshot::from_pool(run_id, snapshot);
        drop(runtime);
        self.status.check()?;
        Ok((snapshot, MempoolReceiver { run_id, receiver, status: self.status.clone() }))
    }
}

fn observer_error(run_id: NodeRunId, error: PoolObserverError) -> MempoolAccessError {
    match error {
        PoolObserverError::SubscriberLimit => MempoolAccessError::SubscriberLimit,
        PoolObserverError::Stopped => MempoolAccessError::Stopped,
        PoolObserverError::Gap { expected_generation, current_generation } => {
            MempoolAccessError::Gap { run_id, expected_generation, current_generation }
        }
    }
}

/// Bounded membership receiver owning only queued data and lifecycle notification.
pub struct MempoolReceiver {
    run_id: NodeRunId,
    receiver: PoolReceiver,
    status: Arc<MempoolStatus>,
}

impl MempoolReceiver {
    /// Receive a complete committed generation. Shutdown takes precedence over queued changes.
    pub async fn recv(&mut self) -> Result<MempoolEvent, MempoolAccessError> {
        self.status.check()?;
        let change = tokio::select! {
            biased;
            () = self.status.wait_for_closing() => return Err(self.status.closing_error()),
            change = self.receiver.recv() => change,
        };
        self.status.check()?;
        let change = change.map_err(|error| observer_error(self.run_id, error))?;
        let event = match change.as_ref() {
            PoolChange::Inserted { generation, entry } => {
                MempoolEvent::Inserted { run_id: self.run_id, generation: *generation, entry: entry.clone() }
            }
            PoolChange::Removed { generation, transaction_ids } => MempoolEvent::Removed {
                run_id: self.run_id,
                generation: *generation,
                transaction_ids: transaction_ids.clone(),
            },
        };
        self.status.check()?;
        Ok(event)
    }
}

/// Typed submission service with weak runtime ownership.
#[derive(Clone)]
pub struct MempoolSubmitter {
    runtime: Weak<MempoolRuntime>,
    status: Arc<MempoolStatus>,
}

impl MempoolSubmitter {
    /// Submit original CBOR using the same timeout as the HTTP submit API.
    pub async fn submit(&self, bytes: &[u8]) -> Result<MempoolAccepted, MempoolSubmitError> {
        self.submit_with_timeout(bytes, DEFAULT_MEMPOOL_INSERT_TIMEOUT.as_duration()).await
    }

    /// Submit with a deadline covering dispatch and response waiting. Timeout is
    /// not cancellation and does not establish that insertion failed.
    pub async fn submit_with_timeout(
        &self,
        bytes: &[u8],
        timeout: Duration,
    ) -> Result<MempoolAccepted, MempoolSubmitError> {
        if self.status.check().is_err() {
            return Err(self.status.submission_error(None));
        }
        let decoded = minicbor::decode::<WithOriginalBytes<Transaction>>(bytes);
        if self.status.check().is_err() {
            return Err(self.status.submission_error(decoded.as_ref().ok().map(|tx| tx.tx_id())));
        }
        let tx = decoded.map_err(|error| MempoolSubmitError::InvalidCbor { reason: error.to_string() })?;
        let transaction_id = tx.tx_id();
        let sender = self
            .runtime
            .upgrade()
            .ok_or(MempoolSubmitError::Stopped { transaction_id: Some(transaction_id) })?
            .sender
            .clone();
        let result = tokio::select! {
            biased;
            () = self.status.wait_for_closing() => return Err(self.status.submission_error(Some(transaction_id))),
            result = sender.call(|caller| MempoolMsg::Insert { tx: Box::new(tx), origin: TxOrigin::Local, caller }, timeout) => result,
        };
        if self.status.check().is_err() {
            return Err(self.status.submission_error(Some(transaction_id)));
        }
        let unavailable = |reason| MempoolSubmitError::Unavailable { transaction_id, reason };
        match result {
            Ok(TxInsertResult::Accepted { tx_id, seq_no }) => {
                Ok(MempoolAccepted { transaction_id: tx_id, sequence: seq_no })
            }
            Ok(TxInsertResult::Rejected { tx_id, reason }) => {
                Err(MempoolSubmitError::Rejected { transaction_id: tx_id, reason })
            }
            Err(CallError::TimedOut) => Err(MempoolSubmitError::Timeout { transaction_id }),
            Err(CallError::SendFailed) => Err(unavailable(MempoolUnavailableReason::SendFailed)),
            Err(CallError::ResponseDropped) => Err(unavailable(MempoolUnavailableReason::ResponseDropped)),
            Err(CallError::ResponseDeserializeFailed) => {
                Err(unavailable(MempoolUnavailableReason::ResponseDeserializeFailed))
            }
        }
    }
}

#[cfg(test)]
mod tests;
