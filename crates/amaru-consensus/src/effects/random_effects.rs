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

use amaru_pure_stage::{BoxFuture, ExternalEffectAPI, Resources, SendData};
use rand::Rng;

/// External effect that produces a fresh 256-bit random seed.
///
/// In production this uses the thread-local CSPRNG (`rand::rng()`).
/// Node simulations override it with a deterministic stream seeded by the node seed
/// (`override_generate_random_seed`): each call returns the next 32 bytes. Peer-selection
/// stage tests override it with a fixed `[0x42; 32]`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct GenerateRandomSeed;

impl ExternalEffectAPI for GenerateRandomSeed {
    type Response = [u8; 32];
    const SIMULATED_DURATION: amaru_pure_stage::DurationDist =
        crate::effect_timings::effects__random_effects__GenerateRandomSeed;

    fn run(self: Box<Self>, _resources: Resources) -> BoxFuture<'static, Box<dyn SendData>> {
        self.wrap_sync(rand::rng().random::<[u8; 32]>())
    }
}
