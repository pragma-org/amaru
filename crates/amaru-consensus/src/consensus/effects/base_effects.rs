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

use pure_stage::{BoxFuture, Effects, Instant, SendData, StageRef};
use std::time::Duration;

/// Base operations available to a stage: send message, get current time, wait, terminate.
/// This trait can have mock implementations for unit testing a stage.
pub trait BaseOps: Clone + Send {
    fn send<Msg: SendData + 'static>(
        &self,
        target: &StageRef<Msg>,
        msg: Msg,
    ) -> BoxFuture<'static, ()>;
    fn clock(&self) -> BoxFuture<'static, Instant>;
    fn wait(&self, duration: Duration) -> BoxFuture<'static, Instant>;
    fn terminate(&self) -> BoxFuture<'static, ()>;
}

/// Implementation of BaseOps using pure_stage::Effects.
#[derive(Clone)]
pub struct Base<'a, T>(&'a Effects<T>);

impl<'a, T> Base<'a, T> {
    pub fn new(eff: &'a Effects<T>) -> Base<'a, T> {
        Base(eff)
    }
}

impl<T: Clone + SendData + Sync> BaseOps for Base<'_, T> {
    fn send<Msg: SendData + 'static>(
        &self,
        target: &StageRef<Msg>,
        msg: Msg,
    ) -> BoxFuture<'static, ()> {
        self.0.send(target, msg)
    }

    fn clock(&self) -> BoxFuture<'static, Instant> {
        self.0.clock()
    }

    fn wait(&self, duration: Duration) -> BoxFuture<'static, Instant> {
        self.0.wait(duration)
    }

    fn terminate(&self) -> BoxFuture<'static, ()> {
        self.0.terminate()
    }
}
