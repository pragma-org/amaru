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

/// This modules captures protocol-wide value pots such as treasury and reserves accounts.
use crate::{Lovelace, cbor};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct Pots {
    #[serde(default)]
    pub treasury: Lovelace,
    #[serde(default)]
    pub reserves: Lovelace,
    #[serde(default)]
    pub fees: Lovelace,
    #[serde(default)]
    pub donations: Lovelace,
}

impl<C> cbor::Encode<C> for Pots {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(4)?;
        e.encode_with(self.treasury, ctx)?;
        e.encode_with(self.reserves, ctx)?;
        e.encode_with(self.fees, ctx)?;
        e.encode_with(self.donations, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::Decode<'a, C> for Pots {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        let treasury = d.decode_with(ctx)?;
        let reserves = d.decode_with(ctx)?;
        let fees = d.decode_with(ctx)?;
        let donations = d.decode_with(ctx)?;
        Ok(Self { treasury, reserves, fees, donations })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::prop_cbor_roundtrip;

    prop_compose! {
        pub fn any_pots()(
            treasury in any::<Lovelace>(),
            reserves in any::<Lovelace>(),
            fees in any::<Lovelace>(),
            donations in any::<Lovelace>(),
        ) -> Pots {
            Pots {
                treasury,
                reserves,
                fees,
                donations,
            }
        }
    }

    prop_cbor_roundtrip!(prop_cbor_roundtrip_pots, Pots, any_pots());
}
