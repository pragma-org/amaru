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

#![feature(generic_const_exprs, const_type_name)]
#![allow(incomplete_features)]
#![deny(clippy::future_not_send)]

pub mod drop_guard;
mod duration_dist;
mod effect;
mod effect_box;
mod logging;
mod output;
mod receiver;
mod resources;
mod sender;
pub mod serde;
pub mod stage_ref;
mod stagegraph;
mod time;
mod timeouts;
pub mod tokio;
pub mod trace_buffer;
pub mod trace_match;
mod types;

pub mod simulation;
pub mod typestate;

pub use duration_dist::DurationDist;
pub use effect::{
    Effect, Effects, ExternalEffect, ExternalEffectAPI, ScheduleIds, StageResponse, UnknownExternalEffect,
};
pub use output::OutputEffect;
pub use receiver::Receiver;
pub use resources::{Resources, WeakResources};
pub use sender::{CallError, SendError, Sender};
pub use serde::{
    DeserializerGuard, DeserializerGuards, serialize_external_effect::register_effect_deserializer,
    serialize_send_data::register_data_deserializer,
};
pub use stage_ref::{StageBuildRef, StageRef};
pub use stagegraph::{ScheduleId, StageGraph, StageGraphRunning, stage_name};
pub use time::{Clock, EPOCH, Instant};
pub use trace_buffer::TerminationReason;
pub use trace_match::{
    Detached, MatchSrc, TraceMatch, assert_effect_match, assert_trace_contains, assert_trace_does_not_contain,
    assert_trace_match, assert_trace_match_filter, tm_add_stage, tm_call, tm_clock, tm_clock_between, tm_effect,
    tm_external_effect, tm_external_effect_any, tm_external_effect_any_match, tm_external_effect_match, tm_input,
    tm_resume, tm_resume_external, tm_resume_external_match, tm_resume_unit, tm_send, tm_state, tm_terminate,
    tm_terminated, tm_wire_stage,
};
pub use types::{
    BLACKHOLE_NAME, BoxFuture, Name, OrTerminateWith, PRIORITY_MAILBOX_SIZE, SendData, TryInStage, Void, err, warn,
};
pub use typetag;
