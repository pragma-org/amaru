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

use crate::{
    cbor,
    cbor::IanaTag,
    utils::cbor::versioned::{HasProtocolVersion, decode_bytes_v12},
};

/// Encode a type as tagged CBOR bytes, also known as "CBOR-in-CBOR".
#[derive(Debug)]
#[repr(transparent)]
pub struct SerialisedAsCbor<T>(pub T);

impl<C, T: cbor::Encode<C>> cbor::Encode<C> for SerialisedAsCbor<T> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(IanaTag::Cbor)?;
        e.bytes(&cbor::to_cbor_with(&self.0, ctx))?;
        Ok(())
    }
}

impl<'d, C: HasProtocolVersion, T: for<'a> cbor::Decode<'a, C>> cbor::Decode<'d, C> for SerialisedAsCbor<T> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let tag = d.tag()?;

        if tag != IanaTag::Cbor.tag() {
            return Err(cbor::decode::Error::message(format!(
                "unexpected tag for script: expected {}, got {}",
                IanaTag::Cbor.tag(),
                tag
            )));
        }

        let bytes = decode_bytes_v12(d, ctx)?;
        let data = cbor::from_cbor_no_leftovers_with(&bytes, ctx).map_err(|e| {
            cbor::decode::Error::message(format!("failed to decode serialised {}: {e}", std::any::type_name::<T>()))
        })?;

        Ok(Self(data))
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::PROTOCOL_VERSION_10;

    /// The tagged bytes must hold exactly one CBOR value, as the Haskell node decodes them with `decodeFullDecoder`.
    #[test_case(&[0xd8, 0x18, 0x41, 0x01] => true ; "one value")]
    #[test_case(&[0xd8, 0x18, 0x42, 0x01, 0x00] => false ; "trailing value")]
    #[test_case(&[0xd8, 0x18, 0x42, 0x01, 0xff] => false ; "trailing break")]
    #[test_case(&[0xd8, 0x18, 0x42, 0x01, 0x01] => false ; "second value")]
    fn decodes_exactly_one_value(bytes: &[u8]) -> bool {
        let mut version = PROTOCOL_VERSION_10;
        cbor::from_cbor_no_leftovers_with::<_, SerialisedAsCbor<u8>>(bytes, &mut version).is_ok()
    }
}
