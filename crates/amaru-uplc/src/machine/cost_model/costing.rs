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

use super::cost_argument::{CostArgument, FixedSize};

pub trait Cost<const N: usize> {
    fn cost(&self, args: &[CostArgument<'_>]) -> i64;
}

// Struct using the trait
#[derive(Debug, PartialEq)]
pub struct Costing<const N: usize, T: Cost<N>> {
    pub mem: T,
    pub cpu: T,
}

impl<const N: usize, T> Costing<N, T>
where
    T: Cost<N>,
{
    pub fn new(mem: T, cpu: T) -> Self {
        Self { mem, cpu }
    }
}

#[derive(Debug, PartialEq)]
pub enum OneArgument {
    Constant(i64),
    LinearInX(LinearSize),
    Quadratic(QuadraticFunction),
}

impl Cost<1> for OneArgument {
    fn cost(&self, args: &[CostArgument<'_>]) -> i64 {
        match self {
            OneArgument::Constant(c) => *c,
            OneArgument::LinearInX(m) => {
                let x = args[0].size();
                m.slope.saturating_mul(x).saturating_add(m.intercept)
            }
            OneArgument::Quadratic(q) => {
                let x = args[0].size();
                q.coeff_0
                    .saturating_add(q.coeff_1.saturating_mul(x))
                    .saturating_add(q.coeff_2.saturating_mul(x).saturating_mul(x))
            }
        }
    }
}

pub type OneArgumentCosting = Costing<1, OneArgument>;

#[derive(Debug, PartialEq)]
pub enum TwoArguments {
    Constant(i64),
    LinearInX(LinearSize),
    LinearInY(LinearSize),
    LinearInXAndY(TwoVariableLinearSize),
    AddedSizes(AddedSizes),
    SubtractedSizes(SubtractedSizes),
    MultipliedSizes(MultipliedSizes),
    MinSize(MinSize),
    MaxSize(MaxSize),
    LinearOnDiagonal(ConstantOrLinear),
    ConstAboveDiagonal(i64, Box<TwoArguments>),
    AboveAndBelowDiagonal(Box<TwoArguments>),
    QuadraticInY(QuadraticFunction),
    QuadraticInXAndY(TwoArgumentsQuadraticFunction),
    WithInteraction(WithInteraction),
}

pub type TwoArgumentsCosting = Costing<2, TwoArguments>;

impl Cost<2> for TwoArguments {
    fn cost(&self, args: &[CostArgument<'_>]) -> i64 {
        match self {
            TwoArguments::Constant(c) => *c,
            TwoArguments::LinearInX(l) => {
                let x = args[0].size();
                l.slope.saturating_mul(x).saturating_add(l.intercept)
            }
            TwoArguments::LinearInY(l) => {
                let y = args[1].size();
                l.slope.saturating_mul(y).saturating_add(l.intercept)
            }
            TwoArguments::LinearInXAndY(l) => {
                let x = args[0].size();
                let y = args[1].size();
                l.slope1.saturating_mul(x).saturating_add(l.slope2.saturating_mul(y)).saturating_add(l.intercept)
            }
            TwoArguments::AddedSizes(s) => {
                let x = args[0].size();
                let y = args[1].size();
                s.slope.saturating_mul(x.saturating_add(y)).saturating_add(s.intercept)
            }
            TwoArguments::SubtractedSizes(s) => {
                let x = args[0].size();
                let y = args[1].size();
                s.slope.saturating_mul(s.minimum.max(x.saturating_sub(y))).saturating_add(s.intercept)
            }
            TwoArguments::MultipliedSizes(s) => {
                let x = args[0].size();
                let y = args[1].size();
                s.slope.saturating_mul(x.saturating_mul(y)).saturating_add(s.intercept)
            }
            TwoArguments::MinSize(s) => {
                let x = args[0].size();
                let y = args[1].size();
                s.slope.saturating_mul(x.min(y)).saturating_add(s.intercept)
            }
            TwoArguments::MaxSize(s) => {
                let x = args[0].size();
                let y = args[1].size();
                s.slope.saturating_mul(x.max(y)).saturating_add(s.intercept)
            }
            TwoArguments::LinearOnDiagonal(l) => {
                let x = args[0].size();
                let y = args[1].size();
                if x == y { x.saturating_mul(l.slope).saturating_add(l.intercept) } else { l.constant }
            }
            TwoArguments::QuadraticInY(q) => {
                let y = args[1].size();
                q.coeff_0
                    .saturating_add(q.coeff_1.saturating_mul(y))
                    .saturating_add(q.coeff_2.saturating_mul(y).saturating_mul(y))
            }
            TwoArguments::QuadraticInXAndY(q) => {
                let x = args[0].size();
                let y = args[1].size();
                q.minimum.max(
                    q.coeff_00
                        .saturating_add(q.coeff_10.saturating_mul(x))
                        .saturating_add(q.coeff_01.saturating_mul(y))
                        .saturating_add(q.coeff_20.saturating_mul(x).saturating_mul(x))
                        .saturating_add(q.coeff_11.saturating_mul(x).saturating_mul(y))
                        .saturating_add(q.coeff_02.saturating_mul(y).saturating_mul(y)),
                )
            }
            TwoArguments::ConstAboveDiagonal(constant, q) => {
                let x = args[0].size();
                let y = args[1].size();
                if x < y { *constant } else { q.cost(args) }
            }
            TwoArguments::AboveAndBelowDiagonal(q) => {
                let x = args[0].size();
                let y = args[1].size();
                {
                    let maximum = FixedSize(x.max(y));
                    let minimum = FixedSize(x.min(y));
                    q.cost(&[(&maximum).into(), (&minimum).into()])
                }
            }
            TwoArguments::WithInteraction(w) => {
                let x = args[0].size();
                let y = args[1].size();
                w.coeff_00
                    .saturating_add(w.coeff_10.saturating_mul(x))
                    .saturating_add(w.coeff_01.saturating_mul(y))
                    .saturating_add(w.coeff_11.saturating_mul(x).saturating_mul(y))
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum ThreeArguments {
    Constant(i64),
    LinearInX(LinearSize),
    LinearInY(LinearSize),
    LinearInZ(LinearSize),
    QuadraticInZ(QuadraticFunction),
    LiteralInYorLinearInZ(LinearSize),
    LinearInYAndZ(TwoVariableLinearSize),
    LinearInMaxYZ(LinearSize),
    ExpModCost(ExpModCost),
}

pub type ThreeArgumentsCosting = Costing<3, ThreeArguments>;

impl Cost<3> for ThreeArguments {
    fn cost(&self, args: &[CostArgument<'_>]) -> i64 {
        match self {
            ThreeArguments::Constant(c) => *c,
            ThreeArguments::LinearInX(l) => {
                let x = args[0].size();
                x.saturating_mul(l.slope).saturating_add(l.intercept)
            }
            ThreeArguments::LinearInY(l) => {
                let y = args[1].size();
                y.saturating_mul(l.slope).saturating_add(l.intercept)
            }
            ThreeArguments::LinearInZ(l) => {
                let z = args[2].size();
                z.saturating_mul(l.slope).saturating_add(l.intercept)
            }
            ThreeArguments::QuadraticInZ(q) => {
                let z = args[2].size();
                q.coeff_0
                    .saturating_add(q.coeff_1.saturating_mul(z))
                    .saturating_add(q.coeff_2.saturating_mul(z).saturating_mul(z))
            }
            ThreeArguments::LiteralInYorLinearInZ(l) => {
                let y = args[1].size();
                if y == 0 {
                    let z = args[2].size();
                    l.slope.saturating_mul(z).saturating_add(l.intercept)
                } else {
                    y
                }
            }
            ThreeArguments::LinearInYAndZ(l) => {
                let y = args[1].size();
                let z = args[2].size();
                y.saturating_mul(l.slope1).saturating_add(z.saturating_mul(l.slope2)).saturating_add(l.intercept)
            }
            ThreeArguments::LinearInMaxYZ(l) => {
                let y = args[1].size();
                let z = args[2].size();
                y.max(z).saturating_mul(l.slope).saturating_add(l.intercept)
            }
            ThreeArguments::ExpModCost(c) => {
                let x = args[0].size();
                let y = args[1].size();
                let z = args[2].size();
                let cost = c
                    .coeff_00
                    .saturating_add(c.coeff_11.saturating_mul(y).saturating_mul(z))
                    .saturating_add(c.coeff_12.saturating_mul(y).saturating_mul(z).saturating_mul(z));
                if x <= z { cost } else { cost.saturating_add(cost / 2) }
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum FourArguments {
    Constant(i64),
    LinearInU(LinearSize),
}

pub type FourArgumentsCosting = Costing<4, FourArguments>;

impl Cost<4> for FourArguments {
    fn cost(&self, args: &[CostArgument<'_>]) -> i64 {
        match self {
            FourArguments::Constant(c) => *c,
            FourArguments::LinearInU(l) => {
                let u = args[3].size();
                u * l.slope + l.intercept
            }
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum SixArguments {
    Constant(i64),
}

pub type SixArgumentsCosting = Costing<6, SixArguments>;

impl Cost<6> for SixArguments {
    fn cost(&self, _args: &[CostArgument<'_>]) -> i64 {
        match self {
            SixArguments::Constant(c) => *c,
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct LinearSize {
    pub intercept: i64,
    pub slope: i64,
}

#[derive(Debug, PartialEq)]
pub struct TwoVariableLinearSize {
    pub intercept: i64,
    pub slope1: i64,
    pub slope2: i64,
}

#[derive(Debug, PartialEq)]
pub struct AddedSizes {
    pub intercept: i64,
    pub slope: i64,
}

#[derive(Debug, PartialEq)]
pub struct SubtractedSizes {
    pub intercept: i64,
    pub slope: i64,
    pub minimum: i64,
}

#[derive(Debug, PartialEq)]
pub struct MultipliedSizes {
    pub intercept: i64,
    pub slope: i64,
}

#[derive(Debug, PartialEq)]
pub struct MinSize {
    pub intercept: i64,
    pub slope: i64,
}

#[derive(Debug, PartialEq)]
pub struct MaxSize {
    pub intercept: i64,
    pub slope: i64,
}

#[derive(Debug, PartialEq)]
pub struct ConstantOrLinear {
    pub constant: i64,
    pub intercept: i64,
    pub slope: i64,
}

#[derive(Debug, PartialEq)]
pub struct QuadraticFunction {
    pub coeff_0: i64,
    pub coeff_1: i64,
    pub coeff_2: i64,
}

#[derive(Debug, PartialEq, Clone)]
pub struct TwoArgumentsQuadraticFunction {
    pub minimum: i64,
    pub coeff_00: i64,
    pub coeff_01: i64,
    pub coeff_02: i64,
    pub coeff_10: i64,
    pub coeff_11: i64,
    pub coeff_20: i64,
}

#[derive(Debug, PartialEq)]
pub struct WithInteraction {
    pub coeff_00: i64,
    pub coeff_10: i64,
    pub coeff_01: i64,
    pub coeff_11: i64,
}

#[derive(Debug, PartialEq)]
pub struct ExpModCost {
    pub coeff_00: i64,
    pub coeff_11: i64,
    pub coeff_12: i64,
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use super::{Cost, LinearSize, ThreeArguments};
    use crate::machine::cost_model::cost_argument::{CostArgument, IntoMachineSize};

    struct CountedMachineSize<'a> {
        calls: &'a Cell<u8>,
        value: i64,
    }

    impl IntoMachineSize for CountedMachineSize<'_> {
        fn size(&self) -> i64 {
            self.calls.set(self.calls.get() + 1);
            self.value
        }
    }

    #[test]
    fn constant_cost_does_not_measure_arguments() {
        let calls = Cell::new(0);
        let value = CountedMachineSize { calls: &calls, value: 10 };
        let args = [CostArgument::from(&value), CostArgument::from(&value), CostArgument::from(&value)];

        assert_eq!(ThreeArguments::Constant(42).cost(args.as_slice()), 42);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn cost_models_measure_only_the_arguments_they_use() {
        let x_calls = Cell::new(0);
        let y_calls = Cell::new(0);
        let z_calls = Cell::new(0);
        let x = CountedMachineSize { calls: &x_calls, value: 2 };
        let y = CountedMachineSize { calls: &y_calls, value: 3 };
        let z = CountedMachineSize { calls: &z_calls, value: 4 };
        let args = [CostArgument::from(&x), CostArgument::from(&y), CostArgument::from(&z)];
        assert_eq!(ThreeArguments::LinearInX(LinearSize { intercept: 1, slope: 2 }).cost(args.as_slice()), 5);
        assert_eq!(ThreeArguments::LinearInY(LinearSize { intercept: 1, slope: 2 }).cost(args.as_slice()), 7);
        assert_eq!(ThreeArguments::LinearInY(LinearSize { intercept: 1, slope: 2 }).cost(args.as_slice()), 7);
        assert_eq!(x_calls.get(), 1);
        assert_eq!(y_calls.get(), 1);
        assert_eq!(z_calls.get(), 0);
    }
}
