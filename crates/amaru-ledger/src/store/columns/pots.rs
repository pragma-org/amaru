// Copyright 2024 PRAGMA
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
use amaru_kernel::{cbor, Lovelace};

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub treasury: Lovelace,
    pub reserves: Lovelace,
    pub fees: Lovelace,
}

impl Row {
    pub fn new(treasury: Lovelace, reserves: Lovelace, fees: Lovelace) -> Self {
        Self {
            treasury,
            reserves,
            fees,
        }
    }
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        e.encode_with(self.treasury, ctx)?;
        e.encode_with(self.reserves, ctx)?;
        e.encode_with(self.fees, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        let treasury = d.decode_with(ctx)?;
        let reserves = d.decode_with(ctx)?;
        let fees = d.decode_with(ctx)?;
        Ok(Row::new(treasury, reserves, fees))
    }
}

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::*;
    use amaru_kernel::prop_cbor_roundtrip;
    use proptest::prelude::*;

    prop_compose! {
        pub fn any_row()(
            treasury in any::<Lovelace>(),
            reserves in any::<Lovelace>(),
            fees in any::<Lovelace>(),
        ) -> Row {
            Row {
                treasury,
                reserves,
                fees,
            }
        }
    }

    prop_cbor_roundtrip!(prop_cbor_roundtrip_row, Row, any_row());
}
