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

use std::collections::BTreeMap;

use crate::{AssetName, CompactMap, Hash, cbor, protocol_version::PROTOCOL_VERSION_12, size::SCRIPT};

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

/// Policies held in a single flat allocation before the map promotes to a tree.
///
/// Sampled over ~480 mainnet blocks spread across several days: 95% of outputs carry at most 9
/// policies and 99% at most 26, with a long tail reaching 111.
const POLICIES_INLINE: usize = 16;

/// Assets of one policy held in a single flat allocation.
///
/// Over the same sample, 95% of policies carry at most 2 assets and 99% at most 8, with a tail
/// reaching 208.
const ASSETS_INLINE: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct Multiasset<A>(CompactMap<Hash<{ SCRIPT }>, Assets<A>, POLICIES_INLINE>);

/// The assets held by a single policy, in key order and never empty.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "BTreeMap<AssetName, A>")]
pub struct Assets<A>(CompactMap<AssetName, A, ASSETS_INLINE>);

impl<A> From<BTreeMap<Hash<{ SCRIPT }>, Assets<A>>> for Multiasset<A> {
    fn from(policies: BTreeMap<Hash<{ SCRIPT }>, Assets<A>>) -> Self {
        Self(policies.into_iter().collect())
    }
}

impl<A> Multiasset<A> {
    pub fn new() -> Self {
        Self(CompactMap::new())
    }

    pub fn insert(&mut self, policy: Hash<{ SCRIPT }>, assets: Assets<A>) -> Option<Assets<A>> {
        self.0.insert(policy, assets)
    }

    pub fn get(&self, policy: &Hash<{ SCRIPT }>) -> Option<&Assets<A>> {
        self.0.get(policy)
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Iterate over each `(policy, assets)` pair, in policy order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&Hash<{ SCRIPT }>, &Assets<A>)> {
        self.0.iter()
    }

    /// Iterate over the policies, in order.
    pub fn keys(&self) -> impl Iterator<Item = &Hash<{ SCRIPT }>> {
        self.0.keys()
    }

    /// Iterate over each policy's assets, in policy order.
    pub fn values(&self) -> impl ExactSizeIterator<Item = &Assets<A>> {
        self.0.iter().map(|(_, assets)| assets)
    }
}

impl<A> IntoIterator for Multiasset<A> {
    type Item = (Hash<{ SCRIPT }>, Assets<A>);
    type IntoIter = <CompactMap<Hash<{ SCRIPT }>, Assets<A>, POLICIES_INLINE> as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// A policy was given no asset at all is an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a policy must carry at least one asset")]
pub struct EmptyAssets;

impl<A> TryFrom<BTreeMap<AssetName, A>> for Assets<A> {
    type Error = EmptyAssets;

    fn try_from(assets: BTreeMap<AssetName, A>) -> Result<Self, Self::Error> {
        if assets.is_empty() {
            return Err(EmptyAssets);
        }
        Ok(Self(assets.into_iter().collect()))
    }
}

impl<A> Assets<A> {
    /// Always at least one: the type cannot be built empty.
    #[expect(clippy::len_without_is_empty)]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn get(&self, name: &AssetName) -> Option<&A> {
        self.0.get(name)
    }

    /// Iterate over each `(asset name, amount)` pair, in asset name order.
    pub fn iter(&self) -> impl ExactSizeIterator<Item = (&AssetName, &A)> {
        self.0.iter()
    }

    /// Iterate over the asset names, in order.
    pub fn keys(&self) -> impl Iterator<Item = &AssetName> {
        self.0.keys()
    }

    /// Iterate over the amounts, in asset name order.
    pub fn values(&self) -> impl ExactSizeIterator<Item = &A> {
        self.0.iter().map(|(_, amount)| amount)
    }
}

impl<A> IntoIterator for Assets<A> {
    type Item = (AssetName, A);
    type IntoIter = <CompactMap<AssetName, A, ASSETS_INLINE> as IntoIterator>::IntoIter;
    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

/// Decode the assets of a policy:
///  - They are sorted by asset name
///  - No repeated asset name is allowed.
///  - They cannot be empty.
///
impl<'d, C: cbor::HasProtocolVersion, A> cbor::Decode<'d, C> for Assets<A>
where
    A: for<'a> cbor::Decode<'a, C>,
{
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let assets: BTreeMap<AssetName, A> = cbor::btree_map_with_unique_keys(d, ctx)?;
        Self::try_from(assets).map_err(|e| cbor::decode::Error::message(e.to_string()))
    }
}

/// Write a map the way cardano-ledger does: definite-length up to 23 entries, indefinite-length
/// (header plus break byte) above.
impl<C, A: cbor::Encode<C>> cbor::Encode<C> for Assets<A> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        cbor::encode_variable_length_map(e, self.iter(), ctx)
    }
}

impl<'d, C: cbor::HasProtocolVersion, A: for<'a> cbor::Decode<'a, C>> cbor::Decode<'d, C> for Multiasset<A> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let policies = cbor::heterogeneous_map_with(
            d,
            ctx,
            Multiasset::new(),
            |d, ctx| d.decode_with(ctx),
            |d, ctx, policies, policy| {
                let assets = d.decode_with(ctx)?;
                if policies.insert(policy, assets).is_some() {
                    return Err(cbor::decode::Error::message("duplicate policy in multi-asset bundle"));
                }
                Ok(())
            },
        )?;

        // From protocol version 12 the ledger requires the policy map itself to be non-empty, not
        // just the asset map of each policy. The check belongs here rather than in a caller: it
        // applies to every bundle, minted assets included.
        if policies.is_empty() && ctx.protocol_version() >= PROTOCOL_VERSION_12 {
            return Err(cbor::decode::Error::message("multi-asset bundle must carry at least one policy"));
        }

        // TODO: pass the size limit in the context during the decoding of assets, so that we can
        // reject a bundle as soon as it exceeds the limit rather than decoding the whole thing and then checking the size.
        let size = BYTES_PER_ASSET * policies.values().map(|assets| assets.len()).sum::<usize>()
            + BYTES_PER_POLICY * policies.len();
        if size > MAX_COMPACT_REPRESENTATION_SIZE {
            return Err(cbor::decode::Error::message(
                "multi-asset bundle is too big for the ledger's compact representation",
            ));
        }

        Ok(policies)
    }
}

impl<C, A: cbor::Encode<C>> cbor::Encode<C> for Multiasset<A> {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        cbor::encode_variable_length_map(e, self.0.iter(), ctx)
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::{NonZeroInt, PositiveCoin, ProtocolVersion, protocol_version::PROTOCOL_VERSION_10};

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

    /// The Haskell decoder is shared between a value's assets and a transaction's mint, and from
    /// protocol version 12 it requires the policy map to be non-empty in both.
    #[test_case(PROTOCOL_VERSION_10, &[0xa0]       => matches Ok(_)  ; "definite empty bundle before v12")]
    #[test_case(PROTOCOL_VERSION_10, &[0xbf, 0xff] => matches Ok(_)  ; "indefinite empty bundle before v12")]
    #[test_case(PROTOCOL_VERSION_12, &[0xa0]       => matches Err(_) ; "definite empty bundle from v12")]
    #[test_case(PROTOCOL_VERSION_12, &[0xbf, 0xff] => matches Err(_) ; "indefinite empty bundle from v12")]
    fn decode_rejects_an_empty_mint_from_version_12(
        mut version: ProtocolVersion,
        bytes: &[u8],
    ) -> Result<Multiasset<NonZeroInt>, cbor::decode::Error> {
        cbor::from_cbor_no_leftovers_with(bytes, &mut version)
    }

    /// A `Mint` redeemer points at a policy by its rank in the sorted set, so decoding has to put
    /// the policies in key order whatever order they arrived in.
    #[test]
    fn policies_iterate_in_key_order() {
        let bundle = descending_bundle();

        let policies = bundle.keys().copied().collect::<Vec<_>>();

        assert_eq!(
            policies,
            vec![Hash::new([0x11; SCRIPT]), Hash::new([0x22; SCRIPT]), Hash::new([0x33; SCRIPT])],
            "policies come back sorted, not in the order they were written"
        );
        assert!(policies.is_sorted());
    }

    /// The assets of a policy are a `Map AssetName` too, and sorted for the same reason.
    #[test]
    fn assets_iterate_in_key_order() {
        let bundle = descending_bundle();

        let assets =
            bundle.values().next().expect("one policy at least").keys().map(|name| name.to_vec()).collect::<Vec<_>>();

        assert_eq!(
            assets,
            vec![vec![0x0a], vec![0x0b], vec![0x0c]],
            "assets come back sorted, not in the order they were written"
        );
        assert!(assets.is_sorted());
    }

    // HELPERS

    /// Three policies, each holding three assets, all written in descending key order.
    fn descending_bundle() -> Multiasset<PositiveCoin> {
        let mut out = map_header(3);
        for policy in [0x33, 0x22, 0x11] {
            out.extend(byte_string(&[policy; SCRIPT]));
            out.extend(map_header(3));
            for asset in [0x0c, 0x0b, 0x0a] {
                out.extend(byte_string(&[asset]));
                out.push(0x01);
            }
        }

        let mut version = PROTOCOL_VERSION_10;
        cbor::from_cbor_no_leftovers_with(&out, &mut version).expect("a valid bundle")
    }

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
