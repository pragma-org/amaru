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

/// A bench scenario which performs a rollback across k/2 blocks, which is arguably one of the
/// worse case scenario (many blocks to prune + aggregate to recompute half way).
#[derive(Debug, Clone, Copy)]
pub struct SwitchToFork {
    pub scenario: Scenario,
    pub scale: BenchScale,
}

impl fmt::Display for SwitchToFork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.scenario.fmt(f, &self.scale)
    }
}

impl SwitchToFork {
    pub const ENV_VAR_FORK_POINT: &str = "AMARU_BENCH_FORK_POINT";

    pub fn new(scenario: Scenario) -> Self {
        let scale = BenchScale::from_env();
        Self { scenario, scale }
    }

    pub fn fork_point(&self) -> usize {
        std::env::var(Self::ENV_VAR_FORK_POINT).ok().and_then(|s| s.parse().ok()).unwrap_or(self.scale.block_size / 2)
    }

    #[expect(clippy::expect_used, reason = "non-production code")]
    pub fn run(self, bencher: Bencher<'_, '_>) {
        let mut rng = StdRng::seed_from_u64(self.scenario.seed());
        let db = self.scenario.new_volatile_db(&mut rng, &self.scale);
        let fork_point = self.fork_point();
        let point = db.iter().nth(fork_point).expect("volatile MUST have fragment at fork point").point();
        bencher
            .with_inputs(|| {
                let fragments = db.iter().skip(fork_point + 1).cloned().collect::<Vec<_>>();
                (db.clone(), point, fragments)
            })
            .bench_local_values(|(mut db, point, fragments)| {
                assert!(db.rollback_to(&point).is_ok());
                fragments.into_iter().for_each(|fragment| db.push_back(fragment));
            })
    }
}
