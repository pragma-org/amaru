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

use crate::cbor;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Vote {
    No,
    Yes,
    Abstain,
}

impl<'b, C> cbor::Decode<'b, C> for Vote {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        match d.u8()? {
            0 => Ok(Self::No),
            1 => Ok(Self::Yes),
            2 => Ok(Self::Abstain),
            _ => Err(cbor::decode::Error::message("invalid variant id for Vote kind")),
        }
    }
}

impl<C> cbor::Encode<C> for Vote {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match &self {
            Self::No => e.u8(0)?,
            Self::Yes => e.u8(1)?,
            Self::Abstain => e.u8(2)?,
        };
        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::Vote;

    pub static VOTE_YES: Vote = Vote::Yes;
    pub static VOTE_NO: Vote = Vote::No;
    pub static VOTE_ABSTAIN: Vote = Vote::Abstain;

    pub fn any_vote() -> impl Strategy<Value = Vote> {
        prop_oneof![Just(Vote::Yes), Just(Vote::No), Just(Vote::Abstain)]
    }

    pub fn any_vote_ref() -> impl Strategy<Value = &'static Vote> {
        prop_oneof![Just(&VOTE_YES), Just(&VOTE_NO), Just(&VOTE_ABSTAIN)]
    }
}
