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

use crate::{
    arena::Arena,
    binder::Eval,
    machine::{
        BuiltinSemantics, CostModel, EvalResult, ExBudget, Machine, PlutusVersion,
        cost_model::builtin_costs::{
            BuiltinCostModel, builtin_costs_v1::BuiltinCostsV1, builtin_costs_v2::BuiltinCostsV2,
            builtin_costs_v3::BuiltinCostsV3,
        },
    },
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
    pub fn eval(&'a self, arena: &'a Arena) -> EvalResult<'a, V> {
        self.eval_version(arena, PlutusVersion::V3)
    }

    /// Evaluate with explicit Plutus version
    pub fn eval_version(&'a self, arena: &'a Arena, plutus_version: PlutusVersion) -> EvalResult<'a, V> {
        self.eval_version_budget(arena, plutus_version, ExBudget::default())
    }

    pub fn eval_version_budget(
        &'a self,
        arena: &'a Arena,
        plutus_version: PlutusVersion,
        initial_budget: ExBudget,
    ) -> EvalResult<'a, V> {
        match plutus_version {
            PlutusVersion::V1 => {
                self.evaluate(arena, CostModel::<BuiltinCostsV1>::default(), plutus_version, initial_budget)
            }
            PlutusVersion::V2 => {
                self.evaluate(arena, CostModel::<BuiltinCostsV2>::default(), plutus_version, initial_budget)
            }
            PlutusVersion::V3 => {
                self.evaluate(arena, CostModel::<BuiltinCostsV3>::default(), plutus_version, initial_budget)
            }
        }
    }

    fn evaluate<B: BuiltinCostModel>(
        &'a self,
        arena: &'a Arena,
        cost_model: CostModel<B>,
        plutus_version: PlutusVersion,
        initial_budget: ExBudget,
    ) -> EvalResult<'a, V> {
        let mut machine =
            Machine::new(arena, initial_budget, cost_model, BuiltinSemantics::from(&plutus_version), *self.version);
        let term = machine.run(self.term);
        let info = machine.info();
        EvalResult { term, info }
    }

    pub fn eval_with_params(
        &'a self,
        arena: &'a Arena,
        plutus_version: PlutusVersion,
        protocol_version: (u64, u64),
        cost_model: &[i64],
        initial_budget: ExBudget,
    ) -> EvalResult<'a, V> {
        match plutus_version {
            PlutusVersion::V1 => self.evaluate(
                arena,
                CostModel::<BuiltinCostsV1>::initialize_cost_model(&plutus_version, protocol_version, cost_model),
                plutus_version,
                initial_budget,
            ),
            PlutusVersion::V2 => self.evaluate(
                arena,
                CostModel::<BuiltinCostsV2>::initialize_cost_model(&plutus_version, protocol_version, cost_model),
                plutus_version,
                initial_budget,
            ),
            PlutusVersion::V3 => self.evaluate(
                arena,
                CostModel::<BuiltinCostsV3>::initialize_cost_model(&plutus_version, protocol_version, cost_model),
                plutus_version,
                initial_budget,
            ),
        }
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
