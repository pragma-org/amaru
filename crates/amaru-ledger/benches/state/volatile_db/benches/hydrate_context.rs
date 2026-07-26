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

use amaru_ledger::{
    context::{DefaultPreparationContext, UnresolvedInputPolicy},
    state::volatile::VolatileSequence,
};
use divan::Bencher;
use rand::{Rng, SeedableRng, rngs::StdRng};

use crate::common::{fixture, scale::BenchScale, scenario::Scenario};

/// A bench scenario for measuring creation of a validation context from a large preparation context.
#[derive(Debug, Clone, Copy)]
pub struct HydrateContext {
    pub scenario: Scenario,
    pub scale: BenchScale,
}

impl fmt::Display for HydrateContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.scenario.fmt(f, &self.scale)
    }
}

impl HydrateContext {
    pub fn new(scenario: Scenario) -> Self {
        let scale = BenchScale::from_env();
        Self { scenario, scale }
    }

    #[expect(clippy::expect_used, reason = "non-production code")]
    pub fn run(self, bencher: Bencher<'_, '_>) {
        let mut rng = StdRng::seed_from_u64(self.scenario.seed());
        let db = self.scenario.new_volatile_db(&mut rng, &self.scale);
        let roots = fixture::proposals_roots(&mut rng);
        bencher
            .with_inputs(|| {
                let ix = rng.random_range(0..self.scale.volatile_size);
                let mut ctx = DefaultPreparationContext::new();
                let fragment = &db.iter().nth(ix).expect("db must have fragment").fragment;
                self.scenario.prepare_fragment(&mut ctx, fragment);
                (db.clone(), ctx, roots.clone())
            })
            .bench_local_values(|(db, ctx, roots)| {
                assert!(
                    ctx.into_validation_context(UnresolvedInputPolicy::Defer, roots, &db, &self.scenario.mock_store())
                        .is_ok()
                );
            })
    }
}
