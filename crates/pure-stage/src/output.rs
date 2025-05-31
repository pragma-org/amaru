use crate::{ExternalEffect, ExternalEffectAPI, Message, Name};
use std::fmt;
use tokio::sync::mpsc;

#[derive(Clone)]
pub struct OutputEffect<Msg: Message> {
    pub name: Name,
    pub msg: Msg,
    sender: mpsc::Sender<Msg>,
}

impl<Msg: Message> OutputEffect<Msg> {
    pub fn new(name: Name, msg: Msg, sender: mpsc::Sender<Msg>) -> Self {
        Self { name, msg, sender }
    }

    /// Create a fake output effect for testing.
    pub fn fake(name: Name, msg: Msg) -> (Self, mpsc::Receiver<Msg>) {
        let (tx, rx) = mpsc::channel(1);
        (
            Self {
                name,
                msg,
                sender: tx,
            },
            rx,
        )
    }
}

impl<Msg: Message + fmt::Debug> fmt::Debug for OutputEffect<Msg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutputEffect")
            .field("name", &self.name)
            .field("msg", &self.msg)
            .field("type", &self.msg.type_name())
            .finish()
    }
}

impl<Msg: Message + PartialEq> ExternalEffect for OutputEffect<Msg> {
    fn run(self: Box<Self>) -> crate::BoxFuture<'static, Box<dyn Message>> {
        Box::pin(async move {
            if let Err(e) = self.sender.send(self.msg).await {
                tracing::debug!("output `{}` failed to send message: {:?}", self.name, e.0);
            }
            Box::new(()) as Box<dyn Message>
        })
    }

    fn test_eq(&self, other: &dyn ExternalEffect) -> bool {
        other
            .cast_ref::<Self>()
            .map(|other| self.name == other.name && self.msg == other.msg)
            .unwrap_or(false)
    }
}

impl<Msg: Message + PartialEq> ExternalEffectAPI for OutputEffect<Msg> {
    type Response = ();
}
