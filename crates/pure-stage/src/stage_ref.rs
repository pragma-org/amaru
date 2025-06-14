use crate::{Name, Void};
use std::{fmt, marker::PhantomData};

/// A handle to a stage during the building phase of a [`StageGraph`](crate::StageGraph).
pub struct StageBuildRef<Msg, St, RefAux> {
    pub name: Name,
    pub(crate) network: RefAux,
    pub(crate) _ph: PhantomData<(Msg, St)>,
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
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StageRef<Msg, State> {
    pub name: Name,
    #[serde(skip)]
    pub(crate) _ph: PhantomData<(Msg, State)>,
}

impl<Msg, State> PartialEq for StageRef<Msg, State> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

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

#[test]
fn stage_ref() {
    let stage = StageRef {
        name: "test".into(),
        _ph: PhantomData::<(u32, u64)>,
    };

    fn send<T: Send>(_t: &T) {}
    fn sync<T: Sync>(_t: &T) {}

    send(&stage);
    sync(&stage);
}
