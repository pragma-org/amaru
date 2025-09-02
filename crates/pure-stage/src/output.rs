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

use crate::{ExternalEffect, ExternalEffectAPI, Resources, SendData, StageName, types::MpscSender};
use std::fmt;
use tokio::sync::mpsc;

/// An effect that sends a message to an output channel.
///
/// This is used to send messages to the output stage, which is used to collect the results of the simulation.
///
/// The [`OutputEffect`] is created by [`StageGraph::output`](crate::StageGraph::output).
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct OutputEffect<Msg> {
    pub name: StageName,
    pub msg: Msg,
    sender: MpscSender<Msg>,
}

impl<Msg> OutputEffect<Msg> {
    pub fn new(name: StageName, msg: Msg, sender: mpsc::Sender<Msg>) -> Self {
        Self {
            name,
            msg,
            sender: MpscSender { sender },
        }
    }

    /// Create a fake output effect for testing.
    pub fn fake(name: StageName, msg: Msg) -> (Self, mpsc::Receiver<Msg>) {
        let (tx, rx) = mpsc::channel(1);
        (
            Self {
                name,
                msg,
                sender: MpscSender { sender: tx },
            },
            rx,
        )
    }
}

impl<Msg: SendData> fmt::Debug for OutputEffect<Msg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputEffect")
            .field("name", &self.name)
            .field("msg", &self.msg)
            .field("type", &self.msg.typetag_name())
            .finish()
    }
}

impl<Msg: SendData + PartialEq> PartialEq for OutputEffect<Msg> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name && self.msg == other.msg
    }
}

impl<Msg> ExternalEffect for OutputEffect<Msg>
where
    Msg: SendData + PartialEq + serde::Serialize + serde::de::DeserializeOwned,
{
    fn run(self: Box<Self>, _resources: Resources) -> crate::BoxFuture<'static, Box<dyn SendData>> {
        Box::pin(async move {
            if let Err(e) = self.sender.send(self.msg).await {
                tracing::debug!("output `{}` failed to send message: {:?}", self.name, e.0);
            }
            Box::new(()) as Box<dyn SendData>
        })
    }
}

impl<Msg> ExternalEffectAPI for OutputEffect<Msg>
where
    Msg: SendData + PartialEq + serde::Serialize + serde::de::DeserializeOwned,
{
    type Response = ();
}
