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

use amaru_kernel::PlutusVersion;

use crate::{
    arena::Arena,
    binder::Eval,
    machine::{CostModel, EvalResult, ExBudget, Machine},
    term::Term,
};

#[derive(Debug)]
pub struct Program<'a, V> {
    pub version: &'a Version<'a>,
    pub term: &'a Term<'a, V>,
}

impl<'a, V> Program<'a, V> {
    pub fn new(arena: &'a Arena, version: &'a Version<'a>, term: &'a Term<'a, V>) -> &'a Self {
        let program = Program { version, term };

        arena.alloc(program)
    }

    pub fn apply(&'a self, arena: &'a Arena, term: &'a Term<'a, V>) -> &'a Self {
        let term = self.term.apply(arena, term);

        Self::new(arena, self.version, term)
    }
}

impl<'a, V> Program<'a, V>
where
    V: Eval<'a>,
{
    /// Evaluate a program for the given `CostModel` and budget.
    pub fn eval(&'a self, arena: &'a Arena, cost_model: CostModel, budget: ExBudget) -> EvalResult<'a, V> {
        let mut machine = Machine::new(arena, budget, cost_model, *self.version);
        let term = machine.run(self.term);
        let info = machine.info();
        EvalResult { term, info }
    }

    /// Evaluate with default (most recent) parameters.
    pub fn eval_default(&'a self, arena: &'a Arena) -> EvalResult<'a, V> {
        self.eval_version(arena, PlutusVersion::default())
    }

    /// Evaluate with explicit Plutus version but default budget. This uses the latest/default semantics.
    pub fn eval_version(&'a self, arena: &'a Arena, plutus_version: PlutusVersion) -> EvalResult<'a, V> {
        self.eval_version_budget(arena, plutus_version, ExBudget::default())
    }

    /// Evaluate with explicit Plutus version and budget, but default cost models for the given
    /// version. This uses the latest/default semantics.
    pub fn eval_version_budget(
        &'a self,
        arena: &'a Arena,
        plutus_version: PlutusVersion,
        budget: ExBudget,
    ) -> EvalResult<'a, V> {
        self.eval(
            arena,
            match plutus_version {
                PlutusVersion::V1 => CostModel::v1(),
                PlutusVersion::V2 => CostModel::v2(),
                PlutusVersion::V3 => CostModel::v3(),
            },
            budget,
        )
    }
}

#[derive(Debug, Copy, Clone)]
pub struct Version<'a>(&'a (usize, usize, usize));

impl<'a> Version<'a> {
    pub fn new(arena: &'a Arena, major: usize, minor: usize, patch: usize) -> &'a mut Self {
        let version = arena.alloc((major, minor, patch));

        arena.alloc(Version(version))
    }

    pub fn plutus_v1(arena: &'a Arena) -> &'a mut Self {
        Self::new(arena, 1, 0, 0)
    }

    pub fn plutus_v2(arena: &'a Arena) -> &'a mut Self {
        Self::new(arena, 1, 0, 0)
    }

    pub fn plutus_v3(arena: &'a Arena) -> &'a mut Self {
        Self::new(arena, 1, 1, 0)
    }
    pub fn is_constr_case_available(&'a self) -> bool {
        self.0.0 >= 1 && self.0.1 >= 1
    }

    pub fn major(&'a self) -> usize {
        self.0.0
    }

    pub fn minor(&'a self) -> usize {
        self.0.1
    }

    pub fn patch(&'a self) -> usize {
        self.0.2
    }
}
