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

#![cfg_attr(feature = "nightly", feature(try_trait_v2))]

mod effect;
mod logging;
mod output;
mod receiver;
mod resources;
mod sender;
pub mod serde;
pub mod simulation;
pub mod stage_ref;
mod stagegraph;
mod time;
pub mod tokio;
pub mod trace_buffer;
mod types;

pub use effect::{
    Effect, Effects, ExternalEffect, ExternalEffectAPI, StageResponse, UnknownExternalEffect,
};
pub use output::OutputEffect;
pub use receiver::Receiver;
pub use resources::Resources;
pub use sender::Sender;
pub use stage_ref::{StageBuildRef, StageRef};
pub use stagegraph::{CallId, CallRef, StageGraph, StageGraphRunning};
pub use time::{Clock, EPOCH, Instant};
pub use types::{BoxFuture, Name, OrReturn, SendData, TryInStage, WrapS};

pub use typetag;
