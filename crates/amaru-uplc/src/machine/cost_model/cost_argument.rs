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

use std::cell::OnceCell;

use dashu_base::BitTest;

use crate::{
    binder::Eval,
    constant::{Constant, Integer, IntegerExt, Natural},
    data::PlutusData,
    ledger_value::LedgerValue,
    machine::value::Value,
};

// -------------------------------------------------------------------------------------------------
// CostArgument
// -------------------------------------------------------------------------------------------------

/// A value whose execution-memory representation is computed only if its cost model needs it.
///
/// A builtin's memory and CPU cost formulas share an argument array, so a measured value is
/// cached for the duration of that builtin invocation. This avoids sizing irrelevant arguments
/// and prevents a shared argument from being measured twice.
pub struct CostArgument<'a> {
    arg: &'a (dyn IntoMachineSize + 'a),
    size: OnceCell<i64>,
}

impl<'a> CostArgument<'a> {
    #[inline]
    fn new<T: IntoMachineSize>(arg: &'a T) -> Self {
        Self { arg, size: OnceCell::new() }
    }

    /// Return the cached execution-memory representation consumed by a costing formula.
    pub(crate) fn size(&self) -> i64 {
        *self.size.get_or_init(|| self.arg.size())
    }
}

impl<'a, T: IntoMachineSize> From<&'a T> for CostArgument<'a> {
    fn from(arg: &'a T) -> Self {
        Self::new(arg)
    }
}

// -------------------------------------------------------------------------------------------------
// Type Modifiers
// -------------------------------------------------------------------------------------------------

/// Indicates that data is measured by its number of nodes rather than by its usual data size.
pub struct DataNodeCount<'a>(pub &'a PlutusData<'a>);

/// Indicates that a list or array is measured by its length rather than by the size of its elements.
pub struct ListLength<'a, T>(pub &'a [T]);

/// Indicates a fixed size derived outside the runtime value representation.
pub struct FixedSize(pub i64);

// -------------------------------------------------------------------------------------------------
// IntoMachineSize
// -------------------------------------------------------------------------------------------------

/// Provides the execution-memory representation for a value accepted by a builtin cost model.
pub(crate) trait IntoMachineSize {
    fn size(&self) -> i64;
}

impl<T: IntoMachineSize + ?Sized> IntoMachineSize for &T {
    fn size(&self) -> i64 {
        (*self).size()
    }
}

impl<L: IntoMachineSize, R: IntoMachineSize> IntoMachineSize for (L, R) {
    fn size(&self) -> i64 {
        self.0.size() + self.1.size()
    }
}

impl IntoMachineSize for () {
    fn size(&self) -> i64 {
        1
    }
}

impl IntoMachineSize for bool {
    fn size(&self) -> i64 {
        1
    }
}

impl IntoMachineSize for blst::blst_p1 {
    fn size(&self) -> i64 {
        size_of::<Self>() as i64 / 8
    }
}

impl IntoMachineSize for blst::blst_p2 {
    fn size(&self) -> i64 {
        size_of::<Self>() as i64 / 8
    }
}

impl IntoMachineSize for blst::blst_fp12 {
    fn size(&self) -> i64 {
        size_of::<Self>() as i64 / 8
    }
}

impl IntoMachineSize for [u8] {
    fn size(&self) -> i64 {
        if self.is_empty() { 1 } else { ((self.len() as i64 - 1) / 8) + 1 }
    }
}

impl IntoMachineSize for str {
    fn size(&self) -> i64 {
        self.len() as i64 / 4
    }
}

impl IntoMachineSize for Natural {
    fn size(&self) -> i64 {
        if self == &Natural::ZERO { 1 } else { 1 + ((self.bit_len() - 1) as i64) / 64 }
    }
}

impl IntoMachineSize for Integer {
    fn size(&self) -> i64 {
        if self.is_zero() { 1 } else { 1 + ((self.bits() - 1) as i64) / 64 }
    }
}

impl IntoMachineSize for Constant<'_> {
    fn size(&self) -> i64 {
        match self {
            Constant::Integer(integer) => integer.size(),
            Constant::ByteString(bytes) => bytes.size(),
            Constant::String(string) => string.size(),
            Constant::Unit | Constant::Boolean(_) => 1,
            Constant::ProtoList(_, items) | Constant::ProtoArray(_, items) => ListLength(items).size(),
            Constant::ProtoPair(_, _, left, right) => left.size() + right.size(),
            Constant::Data(data) => data.size(),
            Constant::Bls12_381G1Element(g1) => g1.size(),
            Constant::Bls12_381G2Element(g2) => g2.size(),
            Constant::Bls12_381MlResult(ml_result) => ml_result.size(),
            // FIXME: Increase coverage
            //
            // This branch seems not hit by any of the test / bench; though the overall instance is
            // in several builtins. Some extra test cases is probably necessary.
            Constant::Value(value) => value.size(),
        }
    }
}

impl IntoMachineSize for LedgerValue<'_> {
    fn size(&self) -> i64 {
        let outer_size = self.entries.len();
        let max_inner = self.entries.iter().map(|entry| entry.tokens.len()).max().unwrap_or_default();
        let log_outer = if outer_size > 0 { (outer_size as f64).log2() as i64 + 1 } else { 0 };
        let log_inner = if max_inner > 0 { (max_inner as f64).log2() as i64 + 1 } else { 0 };
        log_outer + log_inner
    }
}

/// FIXME: Lazy sizing of PlutusData
///
/// This should not size all of deeply nested data upfront: its execution budget may already be
/// exhausted and walking it can itself be disproportionately expensive.
impl IntoMachineSize for PlutusData<'_> {
    fn size(&self) -> i64 {
        4 + match self {
            PlutusData::Constr { fields, .. } | PlutusData::List(fields) => {
                fields.iter().map(|field| field.size()).sum()
            }
            PlutusData::Map(items) => items.iter().map(|(key, value)| key.size() + value.size()).sum(),
            PlutusData::Integer(integer) => integer.size(),
            PlutusData::ByteString(bytes) => bytes.size(),
        }
    }
}

impl<'a, V> IntoMachineSize for Value<'a, V>
where
    V: Eval<'a>,
{
    fn size(&self) -> i64 {
        match self {
            Value::Con(constant) => constant.size(),
            Value::Lambda { .. } | Value::Builtin(_) | Value::Delay(_, _) | Value::Constr(_, _) => 1,
        }
    }
}

impl IntoMachineSize for FixedSize {
    fn size(&self) -> i64 {
        self.0
    }
}

impl<T> IntoMachineSize for ListLength<'_, T> {
    fn size(&self) -> i64 {
        self.0.len() as i64
    }
}

impl IntoMachineSize for DataNodeCount<'_> {
    fn size(&self) -> i64 {
        let mut total = 0;
        let mut stack = vec![self.0];

        while let Some(current) = stack.pop() {
            total += 1;
            match current {
                PlutusData::Constr { fields, .. } | PlutusData::List(fields) => stack.extend(fields.iter()),
                PlutusData::Map(pairs) => {
                    for (key, value) in pairs.iter() {
                        stack.push(key);
                        stack.push(value);
                    }
                }
                PlutusData::Integer(_) | PlutusData::ByteString(_) => {}
            }
        }

        total
    }
}

/// Compute the base-2 logarithm expected by the ledger's integer-to-bytes limit checks.
pub(crate) fn integer_log2(integer: &Integer) -> i64 {
    if integer.is_zero() { 0 } else { (integer.bits() - 1) as i64 }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, str::FromStr};

    use test_case::test_case;

    use super::{CostArgument, IntoMachineSize, integer_log2};
    use crate::{
        arena::Arena,
        constant::{Constant, Integer},
        data::PlutusData,
        typ::Type,
    };

    struct CountedMachineSize<'a> {
        calls: &'a Cell<u8>,
        size: i64,
    }

    impl IntoMachineSize for CountedMachineSize<'_> {
        fn size(&self) -> i64 {
            self.calls.set(self.calls.get() + 1);
            self.size
        }
    }

    #[test]
    fn cost_argument_caches_the_machine_size() {
        let calls = Cell::new(0);
        let value = CountedMachineSize { calls: &calls, size: 42 };
        let argument = CostArgument::from(&value);

        assert_eq!(argument.size(), 42);
        assert_eq!(argument.size(), 42);
        assert_eq!(calls.get(), 1);
    }

    #[test_case(false; "list")]
    #[test_case(true; "array")]
    fn polymorphic_container_is_sized_by_length(is_array: bool) {
        let arena = &Arena::new();
        let big = arena.alloc_integer(Integer::from_str("340282366920938463463374607431768211456").unwrap());
        let one_two = &*arena.alloc([PlutusData::integer_from(arena, 1), PlutusData::integer_from(arena, 2)]);
        let items = &*arena.alloc([
            Constant::data(arena, PlutusData::integer(arena, big)),
            Constant::data(arena, PlutusData::byte_string(arena, &[0; 28])),
            Constant::data(arena, PlutusData::constr(arena, 0, one_two)),
        ]);

        let container = if is_array {
            Constant::proto_array(arena, Type::data(arena), items)
        } else {
            Constant::proto_list(arena, Type::data(arena), items)
        };

        assert_eq!(container.size(), 3);
    }

    #[test]
    fn integer_log2_oracle() {
        // Values come from the Haskell implementation.
        assert_eq!(integer_log2(&Integer::ZERO), 0);
        assert_eq!(integer_log2(&Integer::from(1)), 0);
        assert_eq!(integer_log2(&Integer::from(42)), 5);

        assert_eq!(integer_log2(&Integer::from_str("18446744073709551615").unwrap()), 63);
        assert_eq!(integer_log2(&Integer::from_str("999999999999999999999999999999").unwrap()), 99);
        assert_eq!(integer_log2(&Integer::from_str("170141183460469231731687303715884105726").unwrap()), 126);
        assert_eq!(integer_log2(&Integer::from_str("170141183460469231731687303715884105727").unwrap()), 126);
        assert_eq!(integer_log2(&Integer::from_str("170141183460469231731687303715884105728").unwrap()), 127);
        assert_eq!(integer_log2(&Integer::from_str("340282366920938463463374607431768211458").unwrap()), 128);
        assert_eq!(integer_log2(&Integer::from_str("999999999999999999999999999999999999999999").unwrap()), 139);
        assert_eq!(
            integer_log2(
                &Integer::from_str(
                    "999999999999999999999999999999999999999999999999999999999999999999999999999999999999"
                )
                .unwrap()
            ),
            279
        );
    }
}
