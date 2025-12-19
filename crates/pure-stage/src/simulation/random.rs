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

use crate::{Name, StageResponse};
use rand::{Rng, SeedableRng, rngs::StdRng};
use std::collections::VecDeque;

/// A strategy for making decisions within the simulation run.
///
/// While it is possible to use `Arc<Mutex<...>>` to create a shared random number generator,
/// this is not what the SimulationBuilder is meant for: if you need to share a random number
/// generator across threads, then the result will not be deterministic.
pub trait EvalStrategy: Send + 'static {
    /// pick and remove an element from the queue, which is guaranteed to be non-empty
    fn pick_runnable(
        &mut self,
        runnable: &mut VecDeque<(Name, StageResponse)>,
    ) -> (Name, StageResponse);
}

pub struct Fifo;

impl EvalStrategy for Fifo {
    fn pick_runnable(
        &mut self,
        runnable: &mut VecDeque<(Name, StageResponse)>,
    ) -> (Name, StageResponse) {
        runnable
            .pop_front()
            .expect("runnable queue is guaranteed to be non-empty")
    }
}

pub struct RandStdRng(pub StdRng);

impl RandStdRng {
    pub fn derive(&mut self) -> Self {
        RandStdRng(StdRng::seed_from_u64(self.0.random()))
    }
}

impl From<StdRng> for RandStdRng {
    fn from(rng: StdRng) -> Self {
        RandStdRng(rng)
    }
}

impl EvalStrategy for RandStdRng {
    fn pick_runnable(
        &mut self,
        runnable: &mut VecDeque<(Name, StageResponse)>,
    ) -> (Name, StageResponse) {
        let idx = self.0.random_range(0..runnable.len());
        runnable
            .remove(idx)
            .expect("runnable queue is guaranteed to be non-empty")
    }
}
