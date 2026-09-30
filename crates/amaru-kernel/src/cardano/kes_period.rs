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

use std::fmt;

use minicbor::{Decode, Decoder, Encode};

/// Absolute KES period on the chain: the slot number divided by the number of slots per KES period.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct KesPeriod(u64);

/// Number of times a KES key has been evolved since the period it was issued for.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
#[repr(transparent)]
pub struct KesEvolution(u32);

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum KesPeriodError {
    #[error("KES key starting at period {start} cannot be used at earlier period {current}")]
    StartsInTheFuture { start: KesPeriod, current: KesPeriod },
    #[error("KES key starting at period {start} has expired at period {current} after {max_evolutions} evolutions")]
    Expired { start: KesPeriod, current: KesPeriod, max_evolutions: u64 },
}

impl KesPeriod {
    /// Number of evolutions a key issued at `start` needs to sign at `self`.
    pub fn evolutions_since(self, start: KesPeriod, max_evolutions: u64) -> Result<KesEvolution, KesPeriodError> {
        if start > self {
            return Err(KesPeriodError::StartsInTheFuture { start, current: self });
        }
        let evolutions = self.0 - start.0;
        if evolutions >= max_evolutions {
            return Err(KesPeriodError::Expired { start, current: self, max_evolutions });
        }
        u32::try_from(evolutions).map(KesEvolution).map_err(|_| KesPeriodError::Expired {
            start,
            current: self,
            max_evolutions,
        })
    }
}

impl fmt::Display for KesPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u64> for KesPeriod {
    fn from(period: u64) -> Self {
        KesPeriod(period)
    }
}

impl From<KesPeriod> for u64 {
    fn from(period: KesPeriod) -> u64 {
        period.0
    }
}

impl<C> Encode<C> for KesPeriod {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for KesPeriod {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.u64().map(KesPeriod)
    }
}

impl fmt::Display for KesEvolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<u32> for KesEvolution {
    fn from(evolution: u32) -> Self {
        KesEvolution(evolution)
    }
}

impl From<KesEvolution> for u32 {
    fn from(evolution: KesEvolution) -> u32 {
        evolution.0
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;

    #[test_case(10, 10, 62 => Ok(KesEvolution(0)); "same period")]
    #[test_case(10, 71, 62 => Ok(KesEvolution(61)); "last valid evolution")]
    #[test_case(10, 72, 62 => Err(KesPeriodError::Expired { start: KesPeriod(10), current: KesPeriod(72), max_evolutions: 62 }); "one past the last evolution")]
    #[test_case(10, 9, 62 => Err(KesPeriodError::StartsInTheFuture { start: KesPeriod(10), current: KesPeriod(9) }); "before the start")]
    #[test_case(0, u64::from(u32::MAX) + 1, u64::MAX => Err(KesPeriodError::Expired { start: KesPeriod(0), current: KesPeriod(u64::from(u32::MAX) + 1), max_evolutions: u64::MAX }); "beyond u32")]
    fn evolutions_since(start: u64, current: u64, max_evolutions: u64) -> Result<KesEvolution, KesPeriodError> {
        KesPeriod(current).evolutions_since(KesPeriod(start), max_evolutions)
    }
}
