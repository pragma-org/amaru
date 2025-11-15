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

//! This module contains the [`SimulationBuilder`] and [`SimulationRunning`] types, which are
//! used to build and run a simulation.
//!
//! The simulation is a fully controllable and deterministic [`StageGraph`](crate::StageGraph) for testing purposes.
//! Execution is controlled entirely via the [`SimulationRunning`] handle returned from
//! [`StageGraph::run`](crate::StageGraph::run).
//!

#![expect(clippy::panic)]

use crate::{
    BoxFuture, SendData,
    effect::{StageEffect, StageResponse},
};
use either::Either;
use parking_lot::Mutex;
use std::{future::poll_fn, sync::Arc, task::Poll};

pub(crate) type EffectBox =
    Arc<Mutex<Option<Either<StageEffect<Box<dyn SendData>>, StageResponse>>>>;

pub(crate) fn airlock_effect<Out>(
    eb: &EffectBox,
    effect: StageEffect<Box<dyn SendData>>,
    mut response: impl FnMut(Option<StageResponse>) -> Option<Out> + Send + 'static,
) -> BoxFuture<'static, Out> {
    let eb = eb.clone();
    let mut effect = Some(effect);
    Box::pin(poll_fn(move |_| {
        let mut eb = eb.lock();
        if let Some(effect) = effect.take() {
            match eb.take() {
                Some(Either::Left(x)) => panic!("effect already set: {:?}", x),
                // it is either Some(Right(Unit)) after Receive or None otherwise
                Some(Either::Right(StageResponse::Unit)) | None => {}
                Some(Either::Right(resp)) => {
                    panic!("effect airlock contains leftover response: {:?}", resp)
                }
            }
            *eb = Some(Either::Left(effect));
            Poll::Pending
        } else {
            let Some(out) = eb.take() else {
                return Poll::Pending;
            };
            let out = match out {
                Either::Left(x) => panic!("expected response, got effect: {:?}", x),
                Either::Right(x) => response(Some(x)),
            };
            out.map(Poll::Ready).unwrap_or(Poll::Pending)
        }
    }))
}
