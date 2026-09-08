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

use bumpalo::collections::{CollectIn, Vec as BumpVec};

use super::{env::Env, value::Value};
use crate::{arena::Arena, binder::Eval, term::Term};

pub fn value_as_term<'a, V>(arena: &'a Arena, value: &'a Value<'a, V>) -> &'a Term<'a, V>
where
    V: Eval<'a>,
{
    match value {
        Value::Con(x) => arena.alloc(Term::Constant(x)),
        Value::Builtin(runtime) => {
            let mut term = Term::builtin(arena, runtime.fun);

            for _ in 0..runtime.forces {
                term = term.force(arena);
            }

            for arg in &runtime.args {
                term = term.apply(arena, value_as_term(arena, arg));
            }

            term
        }
        Value::Delay(body, env) => with_env(arena, 0, env, body.delay(arena)),
        Value::Lambda { parameter, body, env } => with_env(arena, 0, env, body.lambda(arena, parameter)),
        Value::Constr(tag, fields) => {
            let fields: BumpVec<'_, _> =
                fields.iter().map(|value| value_as_term(arena, value)).collect_in(arena.as_bump());

            let fields = arena.alloc(fields);

            Term::constr(arena, *tag, fields)
        }
    }
}

fn with_env<'a, V>(arena: &'a Arena, lam_cnt: usize, env: &'a Env<'a, V>, term: &'a Term<'a, V>) -> &'a Term<'a, V>
where
    V: Eval<'a>,
{
    #[expect(clippy::wildcard_enum_match_arm)]
    match term {
        Term::Var(name) => {
            let index = name.index();

            if lam_cnt >= index {
                Term::var(arena, name)
            } else {
                env.lookup(index - lam_cnt).map_or_else(|| Term::var(arena, *name), |value| value_as_term(arena, value))
            }
        }
        Term::Lambda { parameter, body } => {
            let body = with_env(arena, lam_cnt + 1, env, body);

            body.lambda(arena, *parameter)
        }
        Term::Apply { function, argument } => {
            let function = with_env(arena, lam_cnt, env, function);
            let argument = with_env(arena, lam_cnt, env, argument);

            function.apply(arena, argument)
        }

        Term::Delay(x) => {
            let body = with_env(arena, lam_cnt, env, x);

            body.delay(arena)
        }
        Term::Force(x) => {
            let body = with_env(arena, lam_cnt, env, x);

            body.force(arena)
        }
        rest => rest,
    }
}
