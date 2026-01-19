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

use crate::{Name, SendData};
use std::collections::BTreeMap;

pub struct Adapter {
    pub name: Name,
    pub target: Name,
    pub transform: Box<dyn Fn(Box<dyn SendData>) -> Box<dyn SendData> + Send + 'static>,
}

impl std::fmt::Debug for Adapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Adapter")
            .field("name", &self.name)
            .field("target", &self.target)
            .finish()
    }
}

impl Adapter {
    pub fn new<Mapped: SendData, Original: SendData>(
        name: Name,
        target: Name,
        transform: impl Fn(Original) -> Mapped + Send + 'static,
    ) -> Self {
        Self {
            name,
            target,
            transform: Box::new(move |orig| {
                #[expect(clippy::expect_used)]
                let orig = orig.cast::<Original>().expect("internal type error");
                let mapped = transform(*orig);
                Box::new(mapped) as Box<dyn SendData>
            }),
        }
    }
}

#[derive(Debug)]
pub enum StageOrAdapter<T> {
    Stage(T),
    Adapter(Adapter),
}

impl<T> StageOrAdapter<T> {
    pub fn as_stage(&self) -> Option<&T> {
        match self {
            StageOrAdapter::Stage(stage) => Some(stage),
            StageOrAdapter::Adapter(_) => None,
        }
    }
}

pub fn find_recipient<T>(
    senders: &mut BTreeMap<Name, StageOrAdapter<T>>,
    orig_name: Name,
    mut msg: Option<Box<dyn SendData>>,
) -> Option<(&mut T, Box<dyn SendData>)> {
    let mut name = &orig_name;
    loop {
        match senders.get(name) {
            Some(StageOrAdapter::Stage(_)) => {
                tracing::trace!(%orig_name, target = %name, "found stage");
                break;
            }
            Some(StageOrAdapter::Adapter(adapter)) => {
                msg = msg.map(|msg| (adapter.transform)(msg));
                name = &adapter.target;
            }
            None => {
                if let Some(msg) = &msg {
                    tracing::debug!(target = %name, msg = msg.typetag_name(), "stage terminated");
                } else {
                    tracing::debug!(target = %name, "stage terminated");
                }
                return None;
            }
        }
    }
    let name = name.clone();
    let Some(StageOrAdapter::Stage(tx)) = senders.get_mut(&name) else {
        return None;
    };
    Some((tx, msg.unwrap_or_else(|| Box::new(()))))
}
