use crate::BoxFuture;
use std::{fmt, sync::Arc};

/// A handle for sending messages to a stage from outside the simulation.
///
/// Such a handle is obtained using [`StageGraph::input`](crate::StageGraph::input).
pub struct Sender<Msg> {
    tx: Arc<dyn Fn(Msg) -> BoxFuture<'static, Result<(), Msg>> + Send + Sync>,
}

impl<Msg> Clone for Sender<Msg> {
    fn clone(&self) -> Self {
        Self {
            tx: self.tx.clone(),
        }
    }
}

impl<Msg> fmt::Debug for Sender<Msg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sender")
            .field("Msg", &std::any::type_name::<Msg>())
            .finish()
    }
}

impl<Msg> Sender<Msg> {
    pub(crate) fn new(
        tx: Arc<dyn Fn(Msg) -> BoxFuture<'static, Result<(), Msg>> + Send + Sync>,
    ) -> Self {
        Self { tx }
    }

    pub fn send(&self, msg: Msg) -> BoxFuture<'static, Result<(), Msg>> {
        (self.tx)(msg)
    }
}
