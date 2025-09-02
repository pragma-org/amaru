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

use crate::{SendData, Sender, StageName, StageRef};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub struct Envelope {
    pub name: StageName,
    pub msg: Box<dyn SendData>,
    pub tx: oneshot::Sender<()>,
}

impl Envelope {
    fn new(name: StageName, msg: Box<dyn SendData>, tx: oneshot::Sender<()>) -> Self {
        Self { name, msg, tx }
    }
}

pub struct Inputs {
    tx: mpsc::Sender<Envelope>,
    rx: mpsc::Receiver<Envelope>,
    peeked: Option<Envelope>,
}

impl Inputs {
    pub fn new(buffer_size: usize) -> Self {
        let (tx, rx) = mpsc::channel(buffer_size);
        Self {
            tx,
            rx,
            peeked: None,
        }
    }

    pub fn sender<Msg: SendData>(&self, stage: &StageRef<Msg>) -> Sender<Msg> {
        let tx_main = self.tx.clone();
        let stage_name = stage.name();
        Sender::new(Arc::new(move |msg| {
            let tx_main = tx_main.clone();
            let stage_name = stage_name.clone();
            Box::pin(async move {
                let (tx, rx) = oneshot::channel();
                tx_main
                    .send(Envelope::new(stage_name, Box::new(msg), tx))
                    .await
                    .map_err(|e| {
                        #[allow(clippy::expect_used)]
                        *e.0.msg.cast::<Msg>().expect("message was just boxed")
                    })?;
                rx.await.ok();
                Ok(())
            })
        }))
    }

    pub fn peek_name(&mut self) -> Option<&StageName> {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }
        self.peeked.as_ref().map(|envelope| &envelope.name)
    }

    pub fn try_next(&mut self) -> Option<Envelope> {
        if self.peeked.is_none() {
            self.peeked = self.rx.try_recv().ok();
        }
        self.peeked.take()
    }

    pub fn put_back(&mut self, envelope: Envelope) {
        assert!(self.peeked.is_none(), "cannot put back twice");
        self.peeked = Some(envelope);
    }
}
