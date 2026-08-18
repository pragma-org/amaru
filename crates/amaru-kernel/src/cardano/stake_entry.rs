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

use crate::{Address, Lovelace, StakeCredential, Value, cbor, cbor::HasProtocolVersion};

/// A stake distribution entry corresponding to a single key/value mapping between a stake
/// credential and an amount. This is decoded from a UTxO but in a way that circumvent allocations
/// to avoid unnecessary churn when scanning a large number of UTxOs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StakeEntry {
    pub credential: Option<StakeCredential>,
    pub lovelace: Lovelace,
}

impl<'d, C: HasProtocolVersion> cbor::Decode<'d, C> for StakeEntry {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let data_type = d.datatype()?;

        Ok(if matches!(data_type, cbor::Type::MapIndef | cbor::Type::Map) {
            decode_modern(d, ctx)?
        } else if matches!(data_type, cbor::Type::ArrayIndef | cbor::Type::Array) {
            decode_legacy(d, ctx)?
        } else {
            return Err(cbor::decode::Error::type_mismatch(data_type));
        })
    }
}

fn decode_modern<C: HasProtocolVersion>(
    d: &mut cbor::Decoder<'_>,
    ctx: &mut C,
) -> Result<StakeEntry, cbor::decode::Error> {
    let (credential, lovelace) = cbor::heterogeneous_map(
        d,
        (None, None),
        |d| d.u8(),
        |d, state, field| {
            match field {
                0 => state.0 = Some(StakeCredential::from_raw_address(&cbor::decode_bytes_with(d, ctx)?)),
                1 => state.1 = Some(Value::decode_lovelace(d, ctx)?),
                2 => d.skip()?,
                3 => d.skip()?,
                _ => return cbor::unexpected_field::<StakeEntry, _>(field),
            }
            Ok(())
        },
    )?;

    Ok(StakeEntry {
        credential: credential.ok_or_else(|| cbor::missing_field::<StakeEntry, Address>(0))?,
        lovelace: lovelace.ok_or_else(|| cbor::missing_field::<StakeEntry, Lovelace>(1))?,
    })
}

fn decode_legacy<C: HasProtocolVersion>(
    d: &mut cbor::Decoder<'_>,
    ctx: &mut C,
) -> Result<StakeEntry, cbor::decode::Error> {
    let len = d.array()?;

    let credential = StakeCredential::from_raw_address(&cbor::decode_bytes_with(d, ctx)?);
    let lovelace = Value::decode_lovelace(d, ctx)?;

    if let Some(len) = len {
        if len > 2 {
            d.skip()?;
        }
    } else {
        if !cbor::decode_break(d, len)? {
            d.skip()?;
            if !cbor::decode_break(d, len)? {
                return Err(cbor::decode::Error::message("expected break after legacy transaction output datum"));
            }
        }
    }

    Ok(StakeEntry { credential, lovelace })
}

#[cfg(test)]
mod tests {
    use proptest::{prelude::*, prop_oneof};

    use crate::{
        Address, StakeCredential, StakeEntry, any_legacy_output, any_modern_output, from_cbor, to_cbor,
        traits::has_lovelace::HasLovelace,
    };

    proptest! {
    #[test]
        fn decode_memoized_output(output in prop_oneof![any_modern_output(), any_legacy_output()]) {
            let StakeEntry { credential, lovelace } = from_cbor(&to_cbor(&output)).unwrap();

            assert_eq!(lovelace, output.lovelace());

            if let Address::Shelley(addr) = &output.address {
                assert_eq!(credential, StakeCredential::try_from(*addr.delegation()).ok());
            } else {
                assert_eq!(credential, None)
            }
        }
    }
}
