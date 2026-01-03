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

#![expect(
    clippy::wildcard_enum_match_arm,
    clippy::unwrap_used,
    clippy::panic,
    clippy::expect_used
)]

mod blocked;
mod inputs;
mod random;
mod replay;
pub mod running;
pub mod simulation_builder;
mod state;

pub use blocked::{Blocked, SendBlock};
pub use random::{EvalStrategy, Fifo, RandStdRng};
pub use running::SimulationRunning;
pub use simulation_builder::SimulationBuilder;
pub use state::Transition;
