use crate::{BoxFuture, Name};
use std::{fmt, marker::PhantomData, sync::Arc};

/// A handle to a stage during the building phase of a [`StageGraph`](crate::StageGraph).
pub struct StageBuildRef<Msg, St, RefAux> {
    pub(crate) name: Name,
    pub(crate) state: St,
    pub(crate) network: RefAux,
    pub(crate) send: Arc<dyn Fn(Msg) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>,
}

impl<Msg, State, RefAux> StageBuildRef<Msg, State, RefAux> {
    /// Derive the handle that can later be used for sending messages to this stage.
    pub fn sender(&self) -> StageRef<Msg, ()> {
        StageRef {
            name: self.name.clone(),
            send: self.send.clone(),
            _ph: PhantomData,
        }
    }
}

/// A handle for sending messages to a stage.
#[derive(Clone)]
pub struct StageRef<Msg, State> {
    pub name: Name,
    pub(crate) send: Arc<dyn Fn(Msg) -> BoxFuture<'static, anyhow::Result<()>> + Send + Sync>,
    pub(crate) _ph: PhantomData<State>,
}

impl<Msg: fmt::Debug, State: fmt::Debug> fmt::Debug for StageRef<Msg, State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StageRef")
            .field("name", &self.name)
            .finish()
    }
}

impl<Msg: fmt::Debug, State> StageRef<Msg, State> {
    /// Send a message to this stage as soon as buffer space is available in the stage's mailbox.
    /// The future resolves as soon as the message has been enqueued, it does not wait until the
    /// message is processed.
    ///
    /// **IMPORTANT:** This method only works when executed from within a stage transition function.
    /// The returned future will never resolve if called from outside a stage transition function.
    pub fn send(&self, msg: Msg) -> BoxFuture<'static, anyhow::Result<()>> {
        (self.send)(msg)
    }

    /// Create a placeholder handle during the stage creation phase of [`StageGraph`](crate::StageGraph)
    /// construction, to initialize the stage state with a value that will later be replaced during
    /// the wiring phase of StageGraph construction.
    pub fn noop() -> Self {
        let send =
            Arc::new(|_| Box::pin(async { Ok(()) }) as BoxFuture<'static, anyhow::Result<()>>);
        Self {
            name: Name::from("noop"),
            send,
            _ph: PhantomData,
        }
    }
}
