use crate::Name;
use std::{fmt, marker::PhantomData};

/// A handle to a stage during the building phase of a [`StageGraph`](crate::StageGraph).
pub struct StageBuildRef<Msg, St, RefAux> {
    pub(crate) name: Name,
    pub(crate) state: St,
    pub(crate) network: RefAux,
    pub(crate) _ph: PhantomData<Msg>,
}

impl<Msg, State, RefAux> StageBuildRef<Msg, State, RefAux> {
    /// Derive the handle that can later be used for sending messages to this stage.
    pub fn sender(&self) -> StageRef<Msg, Void> {
        StageRef {
            name: self.name.clone(),
            _ph: PhantomData,
        }
    }
}

/// A handle for sending messages to a stage via the [`Effects`](crate::Effects) argument to the stage transition function.
pub struct StageRef<Msg, State> {
    pub(crate) name: Name,
    pub(crate) _ph: PhantomData<(Msg, State)>,
}

// A StageRef itself is just a name allowing the creation of thread-local effects.
unsafe impl<Msg: Send, State: Send> Sync for StageRef<Msg, State> {}

impl<Msg, State> Clone for StageRef<Msg, State> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            _ph: PhantomData,
        }
    }
}

impl<Msg, State> fmt::Debug for StageRef<Msg, State> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StageRef")
            .field("name", &self.name)
            .finish()
    }
}

impl<Msg, State> StageRef<Msg, State> {
    pub fn name(&self) -> Name {
        self.name.clone()
    }

    pub fn without_state(&self) -> StageRef<Msg, Void> {
        StageRef {
            name: self.name.clone(),
            _ph: PhantomData,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum Void {}

impl StageRef<(), ()> {
    /// Create a placeholder handle during the stage creation phase of [`StageGraph`](crate::StageGraph)
    /// construction, to initialize the stage state with a value that will later be replaced during
    /// the wiring phase of StageGraph construction.
    ///
    /// If you attempt to use this handle to send messages, it will panic.
    pub fn noop<Msg>() -> StageRef<Msg, Void> {
        StageRef {
            name: Name::from("noop"),
            _ph: PhantomData,
        }
    }
}
