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
        HasMajorVersion, PROTOCOL_VERSION_10, PlutusVersion,
        protocol_version::{PROTOCOL_VERSION_9, PROTOCOL_VERSION_11},
    };
    use bumpalo::collections::Vec as BumpVec;
    use pretty_assertions::assert_eq;
    use test_case::test_case;

    use super::{arena::Arena, constant::Constant, ledger_value::LedgerValue, program::Program, term::Term, typ::Type};
    use crate::{
        binder::DeBruijn,
        flat,
        machine::{CostModel, ExBudget},
        program::Version,
    };

    fn alloc_constants<'a>(
        arena: &'a Arena,
        values: impl IntoIterator<Item = &'a Constant<'a>>,
    ) -> &'a [&'a Constant<'a>] {
        arena.alloc(BumpVec::from_iter_in(values, arena.as_bump()))
    }

    fn integer_list<'a>(arena: &'a Arena, values: &[i128]) -> &'a Term<'a, DeBruijn> {
        let list = Constant::proto_list(
            arena,
            Type::integer(arena),
            alloc_constants(arena, values.iter().copied().map(|value| Constant::integer_from(arena, value))),
        );

        Term::constant(arena, list)
    }

    fn integer_array<'a>(arena: &'a Arena, values: &[i128]) -> &'a Term<'a, DeBruijn> {
        let array = Constant::proto_array(
            arena,
            Type::integer(arena),
            alloc_constants(arena, values.iter().copied().map(|value| Constant::integer_from(arena, value))),
        );

        Term::constant(arena, array)
    }

    fn singleton_value<'a>(arena: &'a Arena) -> &'a LedgerValue<'a> {
        LedgerValue::insert_coin(
            arena,
            arena.alloc([]),
            arena.alloc([0x01]),
            arena.alloc_integer(1.into()),
            LedgerValue::empty(arena),
        )
    }

    fn exp_mod_integer_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::<DeBruijn>::exp_mod_integer(arena)
            .apply(arena, Term::integer_from(arena, 2))
            .apply(arena, Term::integer_from(arena, 3))
            .apply(arena, Term::integer_from(arena, 5))
    }

    fn singleton_value_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        let value = Constant::ledger_value(arena, singleton_value(arena));
        Term::constant(arena, value)
    }

    fn empty_value_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        let value = Constant::ledger_value(arena, LedgerValue::empty(arena));
        Term::constant(arena, value)
    }

    fn singleton_value_data_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        let data = LedgerValue::value_data(arena, singleton_value(arena)).unwrap();
        Term::data(arena, data)
    }

    fn drop_list_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::drop_list(arena)
            .force(arena)
            .apply(arena, Term::integer_from(arena, 1))
            .apply(arena, integer_list(arena, &[1, 2]))
    }

    fn length_of_array_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::length_of_array(arena).force(arena).apply(arena, integer_array(arena, &[1, 2]))
    }

    fn list_to_array_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::list_to_array(arena).force(arena).apply(arena, integer_list(arena, &[1, 2]))
    }

    fn index_array_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::index_array(arena)
            .force(arena)
            .apply(arena, integer_array(arena, &[1, 2]))
            .apply(arena, Term::integer_from(arena, 1))
    }

    fn bls12_381_g1_multi_scalar_mul_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::bls12_381_g1_multi_scalar_mul(arena)
            .apply(arena, Term::var(arena, DeBruijn::zero(arena)))
            .lambda(arena, DeBruijn::zero(arena))
    }

    fn bls12_381_g2_multi_scalar_mul_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::bls12_381_g2_multi_scalar_mul(arena)
            .apply(arena, Term::var(arena, DeBruijn::zero(arena)))
            .lambda(arena, DeBruijn::zero(arena))
    }

    fn insert_coin_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::insert_coin(arena)
            .apply(arena, Term::var(arena, DeBruijn::zero(arena)))
            .lambda(arena, DeBruijn::zero(arena))
    }

    fn lookup_coin_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::lookup_coin(arena)
            .apply(arena, Term::byte_string(arena, arena.alloc([])))
            .apply(arena, Term::byte_string(arena, arena.alloc([0x01])))
            .apply(arena, singleton_value_fixture(arena))
    }

    fn union_value_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::union_value(arena).apply(arena, singleton_value_fixture(arena)).apply(arena, empty_value_fixture(arena))
    }

    fn value_contains_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::value_contains(arena)
            .apply(arena, singleton_value_fixture(arena))
            .apply(arena, singleton_value_fixture(arena))
    }

    fn value_data_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::value_data(arena).apply(arena, singleton_value_fixture(arena))
    }

    fn un_value_data_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::un_value_data(arena).apply(arena, singleton_value_data_fixture(arena))
    }

    fn scale_value_fixture<'a>(arena: &'a Arena) -> &'a Term<'a, DeBruijn> {
        Term::scale_value(arena).apply(arena, Term::integer_from(arena, 2)).apply(arena, singleton_value_fixture(arena))
    }

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

    #[test_case(exp_mod_integer_fixture; "exp_mod_integer")]
    #[test_case(drop_list_fixture; "drop_list")]
    #[test_case(length_of_array_fixture; "length_of_array")]
    #[test_case(list_to_array_fixture; "list_to_array")]
    #[test_case(index_array_fixture; "index_array")]
    #[test_case(bls12_381_g1_multi_scalar_mul_fixture; "bls12_381_g1_multi_scalar_mul")]
    #[test_case(bls12_381_g2_multi_scalar_mul_fixture; "bls12_381_g2_multi_scalar_mul")]
    #[test_case(insert_coin_fixture; "insert_coin")]
    #[test_case(lookup_coin_fixture; "lookup_coin")]
    #[test_case(union_value_fixture; "union_value")]
    #[test_case(value_contains_fixture; "value_contains")]
    #[test_case(value_data_fixture; "value_data")]
    #[test_case(un_value_data_fixture; "un_value_data")]
    #[test_case(scale_value_fixture; "scale_value")]
    fn cannot_parse_v10_programs_using_pre_v11_builtins(
        term: impl for<'a> FnOnce(&'a Arena) -> &'a Term<'a, DeBruijn>,
    ) {
        let arena = Arena::new();

        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term(&arena));

        assert!(
            flat::decode::<DeBruijn>(
                &arena,
                &flat::encode::<DeBruijn>(program).unwrap(),
                PlutusVersion::V3,
                PROTOCOL_VERSION_10.major(),
            )
            .is_err(),
            "builtin introduced in v11 should not be decoded successfully in v10"
        );
    }

    #[test_case(exp_mod_integer_fixture; "exp_mod_integer")]
    #[test_case(drop_list_fixture; "drop_list")]
    #[test_case(length_of_array_fixture; "length_of_array")]
    #[test_case(list_to_array_fixture; "list_to_array")]
    #[test_case(index_array_fixture; "index_array")]
    #[test_case(bls12_381_g1_multi_scalar_mul_fixture; "bls12_381_g1_multi_scalar_mul")]
    #[test_case(bls12_381_g2_multi_scalar_mul_fixture; "bls12_381_g2_multi_scalar_mul")]
    #[test_case(insert_coin_fixture; "insert_coin")]
    #[test_case(lookup_coin_fixture; "lookup_coin")]
    #[test_case(union_value_fixture; "union_value")]
    #[test_case(value_contains_fixture; "value_contains")]
    #[test_case(value_data_fixture; "value_data")]
    #[test_case(un_value_data_fixture; "un_value_data")]
    #[test_case(scale_value_fixture; "scale_value")]
    fn can_parse_and_evaluate_v11_programs_with_v11_builtins(
        term: impl for<'a> FnOnce(&'a Arena) -> &'a Term<'a, DeBruijn>,
    ) {
        let arena = Arena::new();

        let version = Version::plutus_v3(&arena);
        let program = Program::<DeBruijn>::new(&arena, version, term(&arena));

        assert!(
            flat::decode::<DeBruijn>(
                &arena,
                &flat::encode::<DeBruijn>(program).unwrap(),
                PlutusVersion::V3,
                PROTOCOL_VERSION_11.major(),
            )
            .is_ok(),
            "builtin introduced in v11 should be decoded successfully in v11"
        );

        assert!(dbg!(program.eval_default(&arena).term.is_ok()))
    }
}
