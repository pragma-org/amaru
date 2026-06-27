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

pub mod arena;
pub mod binder;
pub mod bls;
pub mod builtin;
pub mod constant;
pub mod data;
pub mod flat;
pub mod ledger_value;
pub mod machine;
pub mod program;
pub mod syn;
pub mod term;
pub mod typ;

pub use bumpalo;

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        PROTOCOL_VERSION_10, PlutusVersion,
        protocol_version::{PROTOCOL_VERSION_9, PROTOCOL_VERSION_11},
    };
    use pretty_assertions::assert_eq;

    use super::{arena::Arena, program::Program, term::Term};
    use crate::{
        binder::DeBruijn,
        machine::{CostModel, ExBudget},
        program::Version,
    };

    #[test]
    fn add_integer() {
        let arena = Arena::new();

        let term = Term::add_integer(&arena)
            .apply(&arena, Term::integer_from(&arena, 1))
            .apply(&arena, Term::integer_from(&arena, 3));

        let version = Version::plutus_v3(&arena);

        let program = Program::<DeBruijn>::new(&arena, version, term);

        let result = program.eval_default(&arena);

        assert_eq!(result.term.unwrap(), Term::integer_from(&arena, 4));
    }

    #[test]
    fn fibonacci() {
        let arena = &Arena::new();

        let double_force = Term::var(arena, DeBruijn::new(arena, 1))
            .apply(arena, Term::var(arena, DeBruijn::new(arena, 1)))
            .lambda(arena, DeBruijn::zero(arena))
            .delay(arena)
            .force(arena)
            .apply(
                arena,
                Term::var(arena, DeBruijn::new(arena, 3))
                    .apply(
                        arena,
                        Term::var(arena, DeBruijn::new(arena, 1))
                            .apply(arena, Term::var(arena, DeBruijn::new(arena, 1)))
                            .lambda(arena, DeBruijn::zero(arena))
                            .delay(arena)
                            .force(arena)
                            .apply(arena, Term::var(arena, DeBruijn::new(arena, 2))),
                    )
                    .apply(arena, Term::var(arena, DeBruijn::new(arena, 1)))
                    .lambda(arena, DeBruijn::zero(arena))
                    .lambda(arena, DeBruijn::zero(arena)),
            )
            .lambda(arena, DeBruijn::zero(arena))
            .delay(arena)
            .delay(arena)
            .force(arena)
            .force(arena);

        let if_condition = Term::if_then_else(arena)
            .force(arena)
            .apply(arena, Term::var(arena, DeBruijn::new(arena, 3)))
            .apply(arena, Term::var(arena, DeBruijn::new(arena, 2)))
            .apply(arena, Term::var(arena, DeBruijn::new(arena, 1)))
            .apply(arena, Term::unit(arena))
            .lambda(arena, DeBruijn::zero(arena))
            .lambda(arena, DeBruijn::zero(arena))
            .lambda(arena, DeBruijn::zero(arena))
            .delay(arena)
            .force(arena);

        let add = Term::add_integer(arena)
            .apply(
                arena,
                Term::var(arena, DeBruijn::new(arena, 3)).apply(
                    arena,
                    Term::subtract_integer(arena)
                        .apply(arena, Term::var(arena, DeBruijn::new(arena, 2)))
                        .apply(arena, Term::integer_from(arena, 1)),
                ),
            )
            .apply(
                arena,
                Term::var(arena, DeBruijn::new(arena, 3)).apply(
                    arena,
                    Term::subtract_integer(arena)
                        .apply(arena, Term::var(arena, DeBruijn::new(arena, 2)))
                        .apply(arena, Term::integer_from(arena, 2)),
                ),
            )
            .lambda(arena, DeBruijn::zero(arena));

        let term = double_force
            .apply(
                arena,
                if_condition
                    .apply(
                        arena,
                        Term::less_than_equals_integer(arena)
                            .apply(arena, Term::var(arena, DeBruijn::new(arena, 1)))
                            .apply(arena, Term::integer_from(arena, 1)),
                    )
                    .apply(arena, Term::var(arena, DeBruijn::new(arena, 2)).lambda(arena, DeBruijn::zero(arena)))
                    .apply(arena, add)
                    .lambda(arena, DeBruijn::zero(arena))
                    .lambda(arena, DeBruijn::zero(arena)),
            )
            .apply(arena, Term::var(arena, DeBruijn::new(arena, 1)))
            .lambda(arena, DeBruijn::zero(arena))
            .apply(arena, Term::integer_from(arena, 15));

        let version = Version::plutus_v3(arena);

        let program = Program::new(arena, version, term);

        let result = program.eval_default(arena);

        assert_eq!(result.term.unwrap(), Term::integer_from(arena, 610));
    }
    // --- eval_with_params protocol_version gating tests ---

    #[test]
    fn eval_with_params_base_builtin_same_budget_across_protocol_versions() {
        // add_integer is a base V3 builtin (positions 0-3 in the cost key list).
        // Its costs should be identical regardless of protocol_version since they
        // are always included in the base key section.
        let arena = Arena::new();
        let costs = CostModel::DEFAULT_V3;

        let term = Term::add_integer(&arena)
            .apply(&arena, Term::integer_from(&arena, 1))
            .apply(&arena, Term::integer_from(&arena, 3));
        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term);

        let r9 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_9, &costs).unwrap(),
            ExBudget::default(),
        );
        let r10 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_10, &costs).unwrap(),
            ExBudget::default(),
        );
        let r11 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_11, &costs).unwrap(),
            ExBudget::default(),
        );

        // All three should produce the correct result
        assert_eq!(r9.term.unwrap(), Term::integer_from(&arena, 4));
        assert_eq!(r10.term.unwrap(), Term::integer_from(&arena, 4));
        assert_eq!(r11.term.unwrap(), Term::integer_from(&arena, 4));

        // Base builtin budgets should be identical regardless of protocol version
        assert_eq!(r9.info.consumed_budget, r10.info.consumed_budget);
        assert_eq!(r10.info.consumed_budget, r11.info.consumed_budget);
    }

    #[test]
    fn eval_with_params_plomin_builtin_succeeds_at_protocol_v10() {
        // ripemd_160 is a Plomin builtin. With protocol_version >= 10,
        // PLOMIN_KEYS are included in the cost map so the real costs apply.
        let arena = Arena::new();
        let costs = CostModel::DEFAULT_V3;

        let term = Term::<DeBruijn>::ripemd_160(&arena).apply(&arena, Term::byte_string(&arena, b"test"));
        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term);

        let result = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_10, &costs).unwrap(),
            ExBudget::default(),
        );

        assert!(result.term.is_ok(), "post-Plomin ripemd_160 should succeed with real costs");
    }

    #[test]
    fn eval_with_params_plomin_builtin_exceeds_budget_at_protocol_v9() {
        let arena = Arena::new();
        let costs = CostModel::DEFAULT_V3;

        let term = Term::<DeBruijn>::ripemd_160(&arena).apply(&arena, Term::byte_string(&arena, b"test"));
        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term);

        let result = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_9, &costs).unwrap(),
            ExBudget::default(),
        );

        assert!(result.term.is_err(), "pre-Plomin ripemd_160 should fail");
    }

    #[test]
    fn eval_with_params_plomin_builtin_different_budget_by_protocol_version() {
        let arena = Arena::new();
        let costs = CostModel::DEFAULT_V3;

        let term = Term::<DeBruijn>::ripemd_160(&arena).apply(&arena, Term::byte_string(&arena, b"test"));
        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term);

        let r10 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_10, &costs).unwrap(),
            ExBudget::default(),
        );
        let r11 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_11, &costs).unwrap(),
            ExBudget::default(),
        );

        assert!(r10.term.is_ok());
        assert!(r11.term.is_ok());

        // ripemd_160 cost should be identical at PV 10 and 11 (same PLOMIN keys)
        assert_eq!(r10.info.consumed_budget, r11.info.consumed_budget);
    }

    #[test]
    fn eval_with_params_pv11_builtin_exceeds_budget_pre_pv11() {
        let arena = Arena::new();
        let costs = CostModel::DEFAULT_V3;

        let term = Term::<DeBruijn>::exp_mod_integer(&arena)
            .apply(&arena, Term::integer_from(&arena, 2))
            .apply(&arena, Term::integer_from(&arena, 3))
            .apply(&arena, Term::integer_from(&arena, 5));
        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term);

        let r9 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_9, &costs).unwrap(),
            ExBudget::default(),
        );

        let r10 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_10, &costs).unwrap(),
            ExBudget::default(),
        );

        assert!(r9.term.is_err(), "pre-PV11 exp_mod_integer should fail");
        assert!(r10.term.is_err(), "pre-PV11 exp_mod_integer should fail");
    }

    #[test]
    fn eval_with_params_pv11_builtin_succeeds_at_pv11() {
        let arena = Arena::new();
        let costs = CostModel::DEFAULT_V3;

        let term = Term::<DeBruijn>::exp_mod_integer(&arena)
            .apply(&arena, Term::integer_from(&arena, 2))
            .apply(&arena, Term::integer_from(&arena, 3))
            .apply(&arena, Term::integer_from(&arena, 5));
        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term);

        let r11 = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_11, &costs).unwrap(),
            ExBudget::default(),
        );

        assert_eq!(r11.term.unwrap(), Term::integer_from(&arena, 3));
    }

    #[test]
    fn eval_with_params_plomin_and_pv11_builtins_in_same_program() {
        let arena = Arena::new();
        let costs = CostModel::DEFAULT_V3;

        let ripemd = Term::<DeBruijn>::ripemd_160(&arena).apply(&arena, Term::byte_string(&arena, b""));
        let exp_mod = Term::<DeBruijn>::exp_mod_integer(&arena)
            .apply(&arena, Term::integer_from(&arena, 2))
            .apply(&arena, Term::integer_from(&arena, 3))
            .apply(&arena, Term::integer_from(&arena, 5));

        let term = exp_mod.lambda(&arena, DeBruijn::zero(&arena)).apply(&arena, ripemd);
        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term);

        let result = program.eval(
            &arena,
            CostModel::new(PlutusVersion::V3, PROTOCOL_VERSION_11, &costs).unwrap(),
            ExBudget::default(),
        );

        assert_eq!(result.term.unwrap(), Term::integer_from(&arena, 3));
    }
}
