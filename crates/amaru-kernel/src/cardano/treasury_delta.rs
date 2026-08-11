// Copyright 2026 PRAGMA
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

use std::{
    fmt,
    ops::{Add, Sub},
};

use crate::Lovelace;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TreasuryDelta {
    #[default]
    Zero,
    Credit(Lovelace),
    Debit(Lovelace),
}

impl fmt::Display for TreasuryDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Zero => write!(f, "0"),
            Self::Credit(n) => write!(f, "{n}"),
            Self::Debit(n) => write!(f, "-{n}"),
        }
    }
}

impl TreasuryDelta {
    /// Interpret the TreasuryDelta as credit, or return `None` if that's a Debit.
    pub fn as_credit(&self) -> Option<Lovelace> {
        match self {
            Self::Zero => Some(0),
            Self::Credit(credit) => Some(*credit),
            Self::Debit(_) => None,
        }
    }
}

impl Add<Lovelace> for TreasuryDelta {
    type Output = TreasuryDelta;
    fn add(self, rhs: Lovelace) -> Self::Output {
        Add::add(&self, rhs)
    }
}

impl Add<Lovelace> for &TreasuryDelta {
    type Output = TreasuryDelta;

    fn add(self, rhs: Lovelace) -> Self::Output {
        use TreasuryDelta::*;
        match self {
            Zero => Credit(rhs),
            Credit(lhs) => Credit(lhs + rhs),
            Debit(lhs) => {
                if lhs > &rhs {
                    Debit(lhs - rhs)
                } else if &rhs > lhs {
                    Credit(rhs - lhs)
                } else {
                    Zero
                }
            }
        }
    }
}

impl Sub<Lovelace> for TreasuryDelta {
    type Output = TreasuryDelta;
    fn sub(self, rhs: Lovelace) -> Self::Output {
        Sub::sub(&self, rhs)
    }
}

impl Sub<Lovelace> for &TreasuryDelta {
    type Output = TreasuryDelta;

    fn sub(self, rhs: Lovelace) -> Self::Output {
        use TreasuryDelta::*;
        match self {
            Zero => Debit(rhs),
            Debit(lhs) => Debit(lhs + rhs),
            Credit(lhs) => {
                if lhs > &rhs {
                    Credit(lhs - rhs)
                } else if &rhs > lhs {
                    Debit(rhs - lhs)
                } else {
                    Zero
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::{
        TreasuryDelta,
        TreasuryDelta::{Credit, Debit, Zero},
    };

    #[test_case(Zero + 5 => Credit(5); "zero becomes credit")]
    #[test_case(Credit(7) + 5 => Credit(12); "credit accumulates")]
    #[test_case(Debit(7) + 5 => Debit(2); "debit shrinks")]
    #[test_case(Debit(5) + 5 => Zero; "debit cancels out")]
    #[test_case(Debit(3) + 5 => Credit(2); "debit flips to credit")]
    #[test_case(Zero - 5 => Debit(5); "zero becomes debit")]
    #[test_case(Debit(7) - 5 => Debit(12); "debit accumulates")]
    #[test_case(Credit(7) - 5 => Credit(2); "credit shrinks")]
    #[test_case(Credit(5) - 5 => Zero; "credit cancels out")]
    #[test_case(Credit(3) - 5 => Debit(2); "credit flips to debit")]
    fn add_sub_treasury_delta(result: TreasuryDelta) -> TreasuryDelta {
        result
    }
}
