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

use crate::{RationalNumber, cbor};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConstitutionalCommitteeStatus {
    NoConfidence,
    Trusted { threshold: RationalNumber },
}

impl<C> cbor::encode::Encode<C> for ConstitutionalCommitteeStatus {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::NoConfidence => {
                e.array(1)?;
                e.u8(0)?;
            }
            Self::Trusted { threshold } => {
                e.array(2)?;
                e.u8(1)?;
                e.encode_with(threshold, ctx)?;
            }
        };
        Ok(())
    }
}

impl<'d, C> cbor::decode::Decode<'d, C> for ConstitutionalCommitteeStatus {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| match d.u8()? {
            0 => {
                assert_len(1)?;
                Ok(Self::NoConfidence)
            }
            1 => {
                assert_len(2)?;
                let threshold = d.decode_with(ctx)?;
                Ok(Self::Trusted { threshold })
            }
            t => Err(cbor::decode::Error::message(format!(
                "unexpected ConstitutionalCommittee kind: {t}; expected 0 or 1."
            ))),
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::ConstitutionalCommitteeStatus::{self, *};
    use crate::{any_rational_number, prop_cbor_roundtrip};
    use proptest::prelude::*;

    prop_cbor_roundtrip!(
        ConstitutionalCommitteeStatus,
        any_constitutional_committee_status()
    );

    pub fn any_constitutional_committee_status()
    -> impl Strategy<Value = ConstitutionalCommitteeStatus> {
        prop_oneof![
            Just(NoConfidence),
            any_rational_number().prop_map(|threshold| Trusted { threshold }),
        ]
    }
}
