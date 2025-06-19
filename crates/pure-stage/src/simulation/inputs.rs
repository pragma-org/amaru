use crate::{Name, SendData, Sender, StageRef};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub struct Envelope {
    pub name: Name,
    pub msg: Box<dyn SendData>,
    pub tx: oneshot::Sender<()>,
}

impl Envelope {
    fn new(name: Name, msg: Box<dyn SendData>, tx: oneshot::Sender<()>) -> Self {
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

    pub fn sender<Msg: SendData, St>(&self, stage: &StageRef<Msg, St>) -> Sender<Msg> {
        let tx_main = self.tx.clone();
        let stage_name = stage.name();
        Sender::new(Arc::new(move |msg| {
            let tx_main = tx_main.clone();
            let stage_name = stage_name.clone();
            Box::pin(async move {
                let (tx, rx) = oneshot::channel();
                tx_main
                    .send(Envelope::new(stage_name.clone(), Box::new(msg), tx))
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

    pub fn peek_name(&mut self) -> Option<&Name> {
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
