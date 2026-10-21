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

use std::fmt;

use super::{MachineError, discharge, info::MachineInfo, value::Value};
use crate::{arena::Arena, binder::Eval, constant::Constant, term::Term};

pub struct EvalResult<'a, V>
where
    V: Eval<'a>,
{
    value: Result<&'a Value<'a, V>, MachineError<'a, V>>,
    pub info: MachineInfo,
    arena: &'a Arena,
}

impl<'a, V> fmt::Debug for EvalResult<'a, V>
where
    V: Eval<'a>,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("EvalResult").field("is_success", &self.is_success()).field("info", &self.info).finish()
    }
}

impl<'a, V> EvalResult<'a, V>
where
    V: Eval<'a>,
{
    pub(crate) fn new(
        value: Result<&'a Value<'a, V>, MachineError<'a, V>>,
        info: MachineInfo,
        arena: &'a Arena,
    ) -> Self {
        Self { value, info, arena }
    }

    /// Whether evaluation reached a value without raising a machine error.
    pub fn is_success(&self) -> bool {
        self.value.is_ok()
    }

    /// Whether the result is the unit constant, without reifying the value as a term.
    pub fn is_unit(&self) -> bool {
        matches!(self.value, Ok(Value::Con(Constant::Unit)))
    }

    /// Borrow the machine error, if evaluation failed.
    pub fn error(&self) -> Option<&MachineError<'a, V>> {
        self.value.as_ref().err()
    }

    /// Consume the result and reify the evaluated value as a term.
    pub fn into_term(self) -> Result<&'a Term<'a, V>, MachineError<'a, V>> {
        let value = self.value?;

        Ok(discharge::value_as_term(self.arena, value))
    }
}
