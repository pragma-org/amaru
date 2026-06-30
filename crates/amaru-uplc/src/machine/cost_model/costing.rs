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

pub trait Cost<const N: usize> {
    fn cost(&self, args: [i64; N]) -> i64;
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
    fn cost(&self, args: [i64; 1]) -> i64 {
        let x = args[0];

        match self {
            OneArgument::Constant(c) => *c,
            OneArgument::LinearInX(m) => m.slope.saturating_mul(x).saturating_add(m.intercept),
            OneArgument::Quadratic(q) => q
                .coeff_0
                .saturating_add(q.coeff_1.saturating_mul(x))
                .saturating_add(q.coeff_2.saturating_mul(x).saturating_mul(x)),
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
    fn cost(&self, args: [i64; 2]) -> i64 {
        let x = args[0];
        let y = args[1];

        match self {
            TwoArguments::Constant(c) => *c,
            TwoArguments::LinearInX(l) => l.slope.saturating_mul(x).saturating_add(l.intercept),
            TwoArguments::LinearInY(l) => l.slope.saturating_mul(y).saturating_add(l.intercept),
            TwoArguments::LinearInXAndY(l) => {
                l.slope1.saturating_mul(x).saturating_add(l.slope2.saturating_mul(y)).saturating_add(l.intercept)
            }
            TwoArguments::AddedSizes(s) => s.slope.saturating_mul(x.saturating_add(y)).saturating_add(s.intercept),
            TwoArguments::SubtractedSizes(s) => {
                s.slope.saturating_mul(s.minimum.max(x.saturating_sub(y))).saturating_add(s.intercept)
            }
            TwoArguments::MultipliedSizes(s) => s.slope.saturating_mul(x.saturating_mul(y)).saturating_add(s.intercept),
            TwoArguments::MinSize(s) => s.slope.saturating_mul(x.min(y)).saturating_add(s.intercept),
            TwoArguments::MaxSize(s) => s.slope.saturating_mul(x.max(y)).saturating_add(s.intercept),
            TwoArguments::LinearOnDiagonal(l) => {
                if x == y {
                    x.saturating_mul(l.slope).saturating_add(l.intercept)
                } else {
                    l.constant
                }
            }
            TwoArguments::QuadraticInY(q) => q
                .coeff_0
                .saturating_add(q.coeff_1.saturating_mul(y))
                .saturating_add(q.coeff_2.saturating_mul(y).saturating_mul(y)),
            TwoArguments::QuadraticInXAndY(q) => q.minimum.max(
                q.coeff_00
                    .saturating_add(q.coeff_10.saturating_mul(x))
                    .saturating_add(q.coeff_01.saturating_mul(y))
                    .saturating_add(q.coeff_20.saturating_mul(x).saturating_mul(x))
                    .saturating_add(q.coeff_11.saturating_mul(x).saturating_mul(y))
                    .saturating_add(q.coeff_02.saturating_mul(y).saturating_mul(y)),
            ),
            TwoArguments::ConstAboveDiagonal(constant, q) => {
                if x < y {
                    *constant
                } else {
                    q.cost(args)
                }
            }
            TwoArguments::AboveAndBelowDiagonal(q) => q.cost([x.max(y), x.min(y)]),
            TwoArguments::WithInteraction(w) => w
                .coeff_00
                .saturating_add(w.coeff_10.saturating_mul(x))
                .saturating_add(w.coeff_01.saturating_mul(y))
                .saturating_add(w.coeff_11.saturating_mul(x).saturating_mul(y)),
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
    fn cost(&self, args: [i64; 3]) -> i64 {
        let x = args[0];
        let y = args[1];
        let z = args[2];

        match self {
            ThreeArguments::Constant(c) => *c,
            ThreeArguments::LinearInX(l) => x.saturating_mul(l.slope).saturating_add(l.intercept),
            ThreeArguments::LinearInY(l) => y.saturating_mul(l.slope).saturating_add(l.intercept),
            ThreeArguments::LinearInZ(l) => z.saturating_mul(l.slope).saturating_add(l.intercept),
            ThreeArguments::QuadraticInZ(q) => q
                .coeff_0
                .saturating_add(q.coeff_1.saturating_mul(z))
                .saturating_add(q.coeff_2.saturating_mul(z).saturating_mul(z)),
            ThreeArguments::LiteralInYorLinearInZ(l) => {
                if y == 0 {
                    l.slope.saturating_mul(z).saturating_add(l.intercept)
                } else {
                    y
                }
            }
            ThreeArguments::LinearInYAndZ(l) => {
                y.saturating_mul(l.slope1).saturating_add(z.saturating_mul(l.slope2)).saturating_add(l.intercept)
            }
            ThreeArguments::LinearInMaxYZ(l) => y.max(z).saturating_mul(l.slope).saturating_add(l.intercept),
            ThreeArguments::ExpModCost(c) => {
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
    fn cost(&self, args: [i64; 4]) -> i64 {
        let u = args[3];

        match self {
            FourArguments::Constant(c) => *c,
            FourArguments::LinearInU(l) => u * l.slope + l.intercept,
        }
    }
}

#[derive(Debug, PartialEq)]
pub enum SixArguments {
    Constant(i64),
}

pub type SixArgumentsCosting = Costing<6, SixArguments>;

impl Cost<6> for SixArguments {
    fn cost(&self, _args: [i64; 6]) -> i64 {
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
