// Copyright 2026 PRAGMA
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

use std::fmt;

use amaru_ledger::state::volatile::VolatileSequence;
use divan::Bencher;
use rand::{SeedableRng, rngs::StdRng};

use crate::common::{scale::BenchScale, scenario::Scenario};

/// A bench scenario which insert a loaded fragment to an already filled volatile db along a
/// particular dimension (UTxO, Account, ...). This also includes popping a fragment from the front,
/// to mimick a roll forward operation.
#[derive(Debug, Clone, Copy)]
pub struct RollForward {
    pub scenario: Scenario,
    pub scale: BenchScale,
}

impl fmt::Display for RollForward {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.scenario.fmt(f, &self.scale)
    }
}

impl RollForward {
    pub fn new(scenario: Scenario) -> Self {
        let scale = BenchScale::from_env();
        Self { scenario, scale }
    }

    pub fn run(self, bencher: Bencher<'_, '_>) -> i64 {
        let mut rng = StdRng::seed_from_u64(self.scenario.seed());
        let (db, retained_bytes) = crate::retained_bytes(|| self.scenario.new_volatile_db(&mut rng, &self.scale));
        let fragment = self.scenario.new_fragment(&mut rng, &self.scale);

        bencher.with_inputs(|| (db.clone(), fragment.clone())).bench_local_values(|(mut db, next)| {
            let _previous = db.pop_front();
            db.push_back(next);
        });

        retained_bytes
    }
}
