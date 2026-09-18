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

use std::{collections::BTreeMap, ops::Deref};

use crate::{AssetName, Hash, NonEmptyKeyValuePairs, cbor, size::SCRIPT};

/// The Haskell node bounds the size of the values it processes, in order to make them
/// addressable in a memory region with a u16 offset, so the region
/// must fit in `u16::MAX` bytes.
///
/// Each asset costs a `u64` amount: two `u16` offsets and its name (at most 32 bytes).
/// Each distinct policy costs one [`struct@Hash`] of [`SCRIPT`] bytes:
///
/// ```text
/// 8n + 2n + 2n + 32n + 28p <= 65535    i.e.    44n + 28p <= 65535
/// ```
///
/// with `n` the total number of assets and `p` the number of distinct policies.
///
/// We need to reproduce this constraint at decoding time to avoid divergences with the Haskell node
/// which does the same.
const MAX_COMPACT_REPRESENTATION_SIZE: usize = 65535;

/// A `u64` amount, two `u16` offsets and a worst-case 32-byte [`AssetName`].
const BYTES_PER_ASSET: usize = 44;

/// One [`struct@Hash`] of [`SCRIPT`] bytes per distinct policy.
const BYTES_PER_POLICY: usize = 28;

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Multiasset<A>(BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>);

impl<A> From<BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>> for Multiasset<A> {
    fn from(map: BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>) -> Self {
        Self(map)
    }
}

impl<A> Deref for Multiasset<A> {
    type Target = BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl<'d, C: cbor::HasProtocolVersion, A: for<'a> cbor::Decode<'a, C>> cbor::Decode<'d, C> for Multiasset<A> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let assets: BTreeMap<Hash<{ SCRIPT }>, NonEmptyKeyValuePairs<AssetName, A>> = d.decode_with(ctx)?;

        let size = BYTES_PER_ASSET * assets.values().map(|policy| policy.len()).sum::<usize>()
            + BYTES_PER_POLICY * assets.len();
        if size > MAX_COMPACT_REPRESENTATION_SIZE {
            return Err(cbor::decode::Error::message(
                "multi-asset bundle is too big for the ledger's compact representation",
            ));
        }

        Ok(Self(assets))
    }
}

/// Write a map the way cardano-ledger does: definite-length up to 23 entries, indefinite-length
/// (header plus break byte) above.
impl<C, A: cbor::Encode<C>> cbor::Encode<C> for Multiasset<A> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        cbor::encode_variable_length_map(e, self.iter(), ctx)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::{PositiveCoin, protocol_version::PROTOCOL_VERSION_10};

    /// The limit is `44n + 28p <= 65535`, so it depends on the number of policies as well as the
    /// number of assets: the last two cases carry the same 1480 assets and differ only in how many
    /// policies they are spread over.
    ///
    /// The single-policy boundary was checked against the Haskell decoder, which accepts 1488
    /// assets and rejects 1489 with "MultiAsset is too big to compact".
    #[test_case(multiasset(1, 1488) => matches Ok(_)  ; "one policy at the limit")]
    #[test_case(multiasset(1, 1489) => matches Err(_) ; "one policy, one asset too many")]
    #[test_case(multiasset(8, 185)  => matches Ok(_)  ; "1480 assets over 8 policies fit")]
    #[test_case(multiasset(20, 74)  => matches Err(_) ; "the same 1480 assets over 20 policies do not")]
    fn decode_enforces_the_compact_representation_limit(
        bytes: Vec<u8>,
    ) -> Result<Multiasset<PositiveCoin>, cbor::decode::Error> {
        let mut version = PROTOCOL_VERSION_10;
        cbor::from_cbor_no_leftovers_with(&bytes, &mut version)
    }

    // HELPERS

    fn map_header(len: usize) -> Vec<u8> {
        match len {
            0..=23 => vec![0xa0 | len as u8],
            24..=255 => vec![0xb8, len as u8],
            _ => [vec![0xb9], (len as u16).to_be_bytes().to_vec()].concat(),
        }
    }

    fn byte_string(payload: &[u8]) -> Vec<u8> {
        match payload.len() {
            0..=23 => [vec![0x40 | payload.len() as u8], payload.to_vec()].concat(),
            _ => [vec![0x58, payload.len() as u8], payload.to_vec()].concat(),
        }
    }

    /// A bundle of `policies` policies, each holding `assets_per_policy` distinct assets worth 1.
    fn multiasset(policies: usize, assets_per_policy: usize) -> Vec<u8> {
        let mut out = map_header(policies);
        for policy in 0..policies {
            let mut policy_id = [0u8; SCRIPT];
            policy_id[..2].copy_from_slice(&(policy as u16).to_be_bytes());
            out.extend(byte_string(&policy_id));
            out.extend(map_header(assets_per_policy));
            for asset in 0..assets_per_policy {
                out.extend(byte_string(&(asset as u16).to_be_bytes()));
                out.push(0x01);
            }
        }
        out
    }
}
