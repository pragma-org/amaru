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

use crate::Name;
use std::{fmt, marker::PhantomData, ops::Deref};

/// A handle to a stage during the building phase of a [`StageGraph`](crate::StageGraph).
pub struct StageBuildRef<Msg, St, RefAux> {
    pub name: Name,
    pub(crate) network: RefAux,
    pub(crate) _ph: PhantomData<(Msg, St)>,
}

impl<Msg, State, RefAux> StageBuildRef<Msg, State, RefAux> {
    /// Derive the handle that can later be used for sending messages to this stage.
    pub fn sender(&self) -> StageRef<Msg> {
        StageRef {
            name: self.name.clone(),
            _ph: PhantomData,
        }
    }
}

/// A handle for sending messages to a stage via the [`Effects`](crate::Effects) argument to the stage transition function.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct StageRef<Msg> {
    name: Name,
    #[serde(skip)]
    pub(crate) _ph: PhantomData<Msg>,
}

impl<Msg> PartialEq for StageRef<Msg> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<Msg> Eq for StageRef<Msg> {}

impl<Msg> Clone for StageRef<Msg> {
    fn clone(&self) -> Self {
        Self {
            name: self.name.clone(),
            _ph: PhantomData,
        }
    }
}

impl<Msg> fmt::Debug for StageRef<Msg> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("StageRef")
            .field("name", &self.name)
            .finish()
    }
}

impl<Msg> AsRef<StageRef<Msg>> for StageRef<Msg> {
    fn as_ref(&self) -> &StageRef<Msg> {
        self
    }
}

impl<Msg> AsRef<Name> for StageRef<Msg> {
    fn as_ref(&self) -> &Name {
        &self.name
    }
}

impl<Msg> StageRef<Msg> {
    pub(crate) fn new(name: Name) -> Self {
        Self {
            name,
            _ph: PhantomData,
        }
    }

    pub fn named_for_tests(name: &str) -> StageRef<Msg> {
        StageRef::new(Name::from(name))
    }

    pub fn blackhole() -> StageRef<Msg> {
        StageRef::new(Name::from(""))
    }

    pub fn name(&self) -> &Name {
        &self.name
    }
}

/// A handle for sending messages to a stage via the [`Effects`](crate::Effects) argument to the stage transition function.
///
/// This is a variant that is mostly useful in tests because it allows extracting the current state of the stage.
#[derive(PartialEq, serde::Serialize, serde::Deserialize)]
pub struct StageStateRef<Msg, St> {
    stage_ref: StageRef<Msg>,
    #[serde(skip)]
    pub(crate) _ph: PhantomData<St>,
}

impl<Msg, St> Clone for StageStateRef<Msg, St> {
    fn clone(&self) -> Self {
        Self {
            stage_ref: self.stage_ref.clone(),
            _ph: self._ph,
        }
    }
}

impl<Msg, St> StageStateRef<Msg, St> {
    pub(crate) fn new(name: Name) -> Self {
        Self {
            stage_ref: StageRef::new(name),
            _ph: PhantomData,
        }
    }
}

impl<Msg, St> fmt::Debug for StageStateRef<Msg, St> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.stage_ref.fmt(f)
    }
}

impl<Msg, St> StageStateRef<Msg, St> {
    pub fn without_state(self) -> StageRef<Msg> {
        self.stage_ref
    }
}

impl<Msg, St> Deref for StageStateRef<Msg, St> {
    type Target = StageRef<Msg>;

    fn deref(&self) -> &Self::Target {
        &self.stage_ref
    }
}

impl<Msg, St> AsRef<StageRef<Msg>> for StageStateRef<Msg, St> {
    fn as_ref(&self) -> &StageRef<Msg> {
        &self.stage_ref
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
