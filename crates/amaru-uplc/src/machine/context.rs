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

use super::{env::Env, value::Value};
use crate::{arena::Arena, binder::Eval, term::Term};

pub enum Context<'a, V>
where
    V: Eval<'a>,
{
    FrameAwaitArg(&'a Value<'a, V>, &'a Context<'a, V>),
    FrameAwaitFunTerm(&'a Env<'a, V>, &'a Term<'a, V>, &'a Context<'a, V>),
    FrameAwaitFunValue(&'a Value<'a, V>, &'a Context<'a, V>),
    FrameForce(&'a Context<'a, V>),
    FrameConstr(&'a Env<'a, V>, usize, &'a [&'a Term<'a, V>], &'a [&'a Value<'a, V>], &'a Context<'a, V>),
    FrameCases(&'a Env<'a, V>, &'a [&'a Term<'a, V>], &'a Context<'a, V>),
    NoFrame,
}

impl<'a, V> Context<'a, V>
where
    V: Eval<'a>,
{
    pub fn no_frame(arena: &'a Arena) -> &'a Context<'a, V> {
        arena.alloc(Context::NoFrame)
    }

    pub fn frame_await_arg(
        arena: &'a Arena,
        function: &'a Value<'a, V>,
        context: &'a Context<'a, V>,
    ) -> &'a Context<'a, V> {
        arena.alloc(Context::FrameAwaitArg(function, context))
    }

    pub fn frame_await_fun_term(
        arena: &'a Arena,
        arg_env: &'a Env<'a, V>,
        argument: &'a Term<'a, V>,
        context: &'a Context<'a, V>,
    ) -> &'a Context<'a, V> {
        arena.alloc(Context::FrameAwaitFunTerm(arg_env, argument, context))
    }

    pub fn frame_await_fun_value(
        arena: &'a Arena,
        argument: &'a Value<'a, V>,
        context: &'a Context<'a, V>,
    ) -> &'a Context<'a, V> {
        arena.alloc(Context::FrameAwaitFunValue(argument, context))
    }

    pub fn frame_force(arena: &'a Arena, context: &'a Context<'a, V>) -> &'a Context<'a, V> {
        arena.alloc(Context::FrameForce(context))
    }

    pub fn frame_constr_empty(
        arena: &'a Arena,
        env: &'a Env<'a, V>,
        index: usize,
        terms: &'a [&'a Term<'a, V>],
        context: &'a Context<'a, V>,
    ) -> &'a Context<'a, V> {
        let empty: &'a [&'a Value<'a, V>] = &[];

        arena.alloc(Context::FrameConstr(env, index, terms, empty, context))
    }

    pub fn frame_constr(
        arena: &'a Arena,
        env: &'a Env<'a, V>,
        index: usize,
        terms: &'a [&'a Term<'a, V>],
        values: &'a [&'a Value<'a, V>],
        context: &'a Context<'a, V>,
    ) -> &'a Context<'a, V> {
        arena.alloc(Context::FrameConstr(env, index, terms, values, context))
    }

    pub fn frame_cases(
        arena: &'a Arena,
        env: &'a Env<'a, V>,
        terms: &'a [&'a Term<'a, V>],
        context: &'a Context<'a, V>,
    ) -> &'a Context<'a, V> {
        arena.alloc(Context::FrameCases(env, terms, context))
    }
}
