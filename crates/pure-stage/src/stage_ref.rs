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

use crate::{BLACKHOLE_NAME, Name};
use std::{any::Any, fmt, marker::PhantomData, ops::Deref, sync::Arc};

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
            extra: None,
            _ph: StageRefPhantom::default(),
        }
    }
}

/// A handle for sending messages to a stage via the [`Effects`](crate::Effects) argument to the stage transition function.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(bound = "")]
pub struct StageRef<Msg> {
    name: Name,
    #[serde(skip)]
    extra: Option<Arc<dyn Any + Send + Sync>>,
    #[serde(skip)]
    _ph: StageRefPhantom<Msg>,
}

struct StageRefPhantom<Msg>(PhantomData<Msg>);
impl<Msg> Default for StageRefPhantom<Msg> {
    fn default() -> Self {
        Self(Default::default())
    }
}
unsafe impl<Msg> Send for StageRefPhantom<Msg> {}
unsafe impl<Msg> Sync for StageRefPhantom<Msg> {}

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
            extra: self.extra.clone(),
            _ph: StageRefPhantom::default(),
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
        const {
            // this needs to be in some non-dead code, no matter where
            crate::types::is_send::<Self>();
            crate::types::is_sync::<Self>();
        }
        Self {
            name,
            extra: None,
            _ph: StageRefPhantom::default(),
        }
    }

    pub(crate) fn with_extra(self, extra: Arc<dyn Any + Send + Sync>) -> Self {
        Self {
            extra: Some(extra),
            ..self
        }
    }

    pub fn named_for_tests(name: &str) -> StageRef<Msg> {
        StageRef::new(Name::from(name))
    }

    pub fn blackhole() -> StageRef<Msg> {
        StageRef::new(BLACKHOLE_NAME.clone())
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    pub(crate) fn extra(&self) -> Option<&Arc<dyn Any + Send + Sync>> {
        self.extra.as_ref()
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
