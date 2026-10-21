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
use crate::{binder::Eval, term::Term};

pub(super) enum Frame<'a, V>
where
    V: Eval<'a>,
{
    AwaitArg(&'a Value<'a, V>),
    AwaitFunTerm(&'a Env<'a, V>, &'a Term<'a, V>),
    AwaitFunValue(&'a Value<'a, V>),
    Force,
    Constr(&'a Env<'a, V>, usize, &'a [&'a Term<'a, V>], &'a [&'a Value<'a, V>]),
    Cases(&'a Env<'a, V>, &'a [&'a Term<'a, V>]),
}
