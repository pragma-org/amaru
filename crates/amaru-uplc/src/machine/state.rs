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

use super::{context::Context, env::Env, value::Value};
use crate::{arena::Arena, binder::Eval, term::Term};

pub enum MachineState<'a, V>
where
    V: Eval<'a>,
{
    Return(&'a Context<'a, V>, &'a Value<'a, V>),
    Compute(&'a Context<'a, V>, &'a Env<'a, V>, &'a Term<'a, V>),
    Done(&'a Value<'a, V>),
}

impl<'a, V> MachineState<'a, V>
where
    V: Eval<'a>,
{
    pub fn compute(
        arena: &'a Arena,
        context: &'a Context<'a, V>,
        env: &'a Env<'a, V>,
        term: &'a Term<'a, V>,
    ) -> &'a mut MachineState<'a, V> {
        arena.alloc(MachineState::Compute(context, env, term))
    }

    pub fn return_(
        arena: &'a Arena,
        context: &'a Context<'a, V>,
        value: &'a Value<'a, V>,
    ) -> &'a mut MachineState<'a, V> {
        arena.alloc(MachineState::Return(context, value))
    }

    pub fn done(arena: &'a Arena, value: &'a Value<'a, V>) -> &'a mut MachineState<'a, V> {
        arena.alloc(MachineState::Done(value))
    }
}
