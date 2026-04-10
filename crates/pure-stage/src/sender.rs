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

use std::{any::Any, fmt, sync::Arc, time::Duration};

use parking_lot::Mutex;
use serde::de::DeserializeOwned;
use tokio::sync::oneshot;

use crate::{BoxFuture, Name, SendData, stage_ref::StageRef};

/// A handle for sending messages to a stage from outside the simulation.
///
/// Such a handle is obtained using [`StageGraph::input`](crate::StageGraph::input).
pub struct Sender<Msg> {
    tx: Arc<dyn Fn(Msg) -> BoxFuture<'static, Result<(), Msg>> + Send + Sync>,
}

impl<Msg> Clone for Sender<Msg> {
    fn clone(&self) -> Self {
        Self { tx: self.tx.clone() }
    }
}

impl<Msg> fmt::Debug for Sender<Msg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender").field("Msg", &std::any::type_name::<Msg>()).finish()
    }
}

impl<Msg> Sender<Msg> {
    pub(crate) fn new(tx: Arc<dyn Fn(Msg) -> BoxFuture<'static, Result<(), Msg>> + Send + Sync>) -> Self {
        Self { tx }
    }

    pub fn send(&self, msg: Msg) -> BoxFuture<'static, Result<(), Msg>> {
        (self.tx)(msg)
    }
}

impl<Msg: SendData> Sender<Msg> {
    /// Send a message to the stage and wait for a response.
    ///
    /// The `message` function receives `StageRef<Resp>` that the target stage must use to send its reply.
    pub fn call<Resp: SendData + DeserializeOwned>(
        &self,
        message: impl FnOnce(StageRef<Resp>) -> Msg + Send + 'static,
        timeout: Duration,
    ) -> BoxFuture<'static, Result<Resp, CallError>> {
        let (tx, rx) = oneshot::channel::<Box<dyn SendData>>();
        let extra: Arc<StageRefExtra> = Arc::new(Mutex::new(Some(tx)));
        let reply_ref =
            StageRef::<Resp>::new(Name::from("sender-call")).with_extra(extra as Arc<dyn Any + Send + Sync>);
        let msg = message(reply_ref);
        let send_fut = self.send(msg);
        Box::pin(async move {
            let resp = tokio::time::timeout(timeout, async {
                send_fut.await.map_err(|_| CallError::SendFailed)?;
                rx.await.map_err(|_| CallError::ResponseDropped)
            })
            .await
            .map_err(|_| CallError::TimedOut)??;

            resp.cast_deserialize::<Resp>().map_err(|e| {
                tracing::warn!(error = ?e, "Failed to deserialize response");
                CallError::ResponseDeserializeFailed
            })
        })
    }
}

/// The channel type used to send a response from a stage back to an external [`Sender::call`] caller.
///
/// This is the same mechanism used internally by [`Effects::call`](crate::Effects::call), allowing
/// the Tokio and simulation runtimes to handle both patterns uniformly.
pub(crate) type StageRefExtra = Mutex<Option<oneshot::Sender<Box<dyn SendData>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CallError {
    #[error("message send failed")]
    SendFailed,
    #[error("timed out waiting for response")]
    TimedOut,
    #[error("response channel closed")]
    ResponseDropped,
    #[error("failed to deserialize response")]
    ResponseDeserializeFailed,
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Instant};

    use super::*;

    #[tokio::test]
    async fn call_times_out_while_enqueue_is_blocked() {
        let sender = Sender::new(Arc::new(|_msg: u8| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(())
            })
        }));

        let start = Instant::now();
        let result = sender.call::<u8>(|_caller| 1, Duration::from_millis(10)).await;

        assert_eq!(result, Err(CallError::TimedOut));
        assert!(start.elapsed() < Duration::from_millis(40));
    }
}
