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

use minicbor::{Decode, Decoder, Encode};
use std::{fmt, ops::Add};

#[derive(
    Clone,
    Debug,
    Copy,
    PartialEq,
    PartialOrd,
    Ord,
    Eq,
    Hash,
    serde::Serialize,
    serde::Deserialize,
    Default,
)]
#[repr(transparent)]
pub struct Slot(u64);

impl fmt::Display for Slot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, PartialEq, Eq, thiserror::Error, serde::Serialize, serde::Deserialize)]
pub enum SlotArithmeticError {
    #[error("slot arithmetic underflow, substracting {1} from {0}")]
    Underflow(u64, u64),
}

impl Slot {
    pub fn new(slot: u64) -> Self {
        Self(slot)
    }

    pub fn elapsed_from(&self, slot: Slot) -> Result<u64, SlotArithmeticError> {
        if self.0 < slot.0 {
            return Err(SlotArithmeticError::Underflow(self.0, slot.0));
        }
        Ok(self.0 - slot.0)
    }

    pub fn offset_by(&self, slots_elapsed: u64) -> Slot {
        Slot(self.0 + slots_elapsed)
    }

    pub fn as_u64(&self) -> u64 {
        self.0
    }
}

impl From<u64> for Slot {
    fn from(slot: u64) -> Slot {
        Slot(slot)
    }
}

impl From<Slot> for u64 {
    fn from(slot: Slot) -> u64 {
        slot.0
    }
}

impl Add<u64> for Slot {
    type Output = Self;

    fn add(self, rhs: u64) -> Self::Output {
        Slot(self.0 + rhs)
    }
}

impl<C> Encode<C> for Slot {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        self.0.encode(e, ctx)
    }
}

impl<'b, C> Decode<'b, C> for Slot {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut C) -> Result<Self, minicbor::decode::Error> {
        d.u64().map(Slot)
    }
}
