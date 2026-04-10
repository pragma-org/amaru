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

//! Decoder for cardano-node's mempack binary format.
//!
//! Mempack is a compact custom binary serialisation of `BabbageTxOut` (which is also the Conway
//! output type). It is **not CBOR**: integers use a 7-bit-per-byte variable-length encoding (MSB
//! continuation bit), byte arrays are varuint-length-prefixed, and multi-asset values use a
//! packed region layout with little-endian offsets and quantities.

use std::collections::BTreeMap;

use amaru_kernel::{
    Address, Bytes, Hash, MemoizedDatum, MemoizedNativeScript, MemoizedPlutusData, MemoizedScript,
    MemoizedTransactionOutput, Multiasset, MultiassetKeyValuePairs, Network, PlutusScript, PositiveCoin,
    ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart, StakeCredential, Value,
};

const INDEFINITE_MAP_THRESHOLD: usize = 23;

/// Decode a mempack-encoded `BabbageTxOut`.
///
/// The first byte is a tag that describes which fields are present:
///
/// | Tag | Contents                                         |
/// |-----|--------------------------------------------------|
/// |   0 | compact address + compact value                  |
/// |   1 | compact address + compact value + datum hash     |
/// |   2 | enterprise address (28-byte) + compact coin      |
/// |   3 | enterprise address (28-byte) + coin + datum hash |
/// |   4 | compact address + compact value + inline datum   |
/// |   5 | compact address + compact value + datum + script |
pub fn decode_transaction_output(bytes: &[u8]) -> Result<MemoizedTransactionOutput, String> {
    let mut decoder = Decoder::new(bytes);

    match decoder.tag()? {
        0 => Ok(make_transaction_output(
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            MemoizedDatum::None,
            None,
        )),
        1 => Ok(make_transaction_output(
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            MemoizedDatum::Hash(decoder.hash32()?),
            None,
        )),
        2 => {
            let stake = decode_stake_credential(&mut decoder)?;
            Ok(make_transaction_output(
                decode_address28(&mut decoder, stake)?,
                Value::Coin(decode_compact_coin(&mut decoder)?),
                MemoizedDatum::None,
                None,
            ))
        }
        3 => {
            let stake = decode_stake_credential(&mut decoder)?;
            Ok(make_transaction_output(
                decode_address28(&mut decoder, stake)?,
                Value::Coin(decode_compact_coin(&mut decoder)?),
                MemoizedDatum::Hash(decoder.packed_hash32()?),
                None,
            ))
        }
        4 => Ok(make_transaction_output(
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            MemoizedDatum::Inline(decode_inline_plutus_data(&mut decoder)?),
            None,
        )),
        5 => Ok(make_transaction_output(
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            decode_datum(&mut decoder)?,
            Some(decode_script(&mut decoder)?),
        )),
        tag => Err(format!("unsupported BabbageTxOut mempack tag {tag}")),
    }
}

fn make_transaction_output(
    address: Address,
    value: Value,
    datum: MemoizedDatum,
    script: Option<MemoizedScript>,
) -> MemoizedTransactionOutput {
    MemoizedTransactionOutput { is_legacy: false, address, value, datum, script }
}

// ------------------------------------------------------------------- Decoder

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn tag(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], String> {
        let end = self.offset.checked_add(len).ok_or_else(|| "mempack offset overflow".to_string())?;
        if end > self.bytes.len() {
            return Err(format!("unexpected end of mempack data at {} while reading {} bytes", self.offset, len));
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn take_array<const N: usize>(&mut self) -> Result<[u8; N], String> {
        let mut bytes = [0u8; N];
        bytes.copy_from_slice(self.take(N)?);
        Ok(bytes)
    }

    /// 7-bit variable-length unsigned integer (MSB continuation bit).
    fn varuint(&mut self) -> Result<u64, String> {
        let mut value = 0u64;
        loop {
            let byte = self.tag()?;
            value = (value << 7) | u64::from(byte & 0x7f);
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
    }

    /// `varuint`-prefixed byte slice.
    fn short_bytes(&mut self) -> Result<&'a [u8], String> {
        let len = self.varuint()? as usize;
        self.take(len)
    }

    fn short_bytes_vec(&mut self) -> Result<Vec<u8>, String> {
        Ok(self.short_bytes()?.to_vec())
    }

    fn decode_from_short_bytes<T, E, F>(&mut self, what: &str, decode: F) -> Result<T, String>
    where
        E: std::fmt::Display,
        F: FnOnce(Vec<u8>) -> Result<T, E>,
    {
        decode(self.short_bytes_vec()?).map_err(|err| format!("invalid {what}: {err}"))
    }

    fn decode_from_short_bytes_ref<T, E, F>(&mut self, what: &str, decode: F) -> Result<T, String>
    where
        E: std::fmt::Display,
        F: FnOnce(&[u8]) -> Result<T, E>,
    {
        decode(self.short_bytes()?).map_err(|err| format!("invalid {what}: {err}"))
    }

    fn hash32(&mut self) -> Result<Hash<32>, String> {
        Ok(Hash::new(self.take_array()?))
    }

    fn packed_hash32(&mut self) -> Result<Hash<32>, String> {
        let bytes = self.take_array::<32>()?;
        let mut hash = [0u8; 32];
        for (index, chunk) in bytes.chunks(8).enumerate() {
            hash[index * 8..(index + 1) * 8].copy_from_slice(chunk);
            hash[index * 8..(index + 1) * 8].reverse();
        }
        Ok(Hash::new(hash))
    }
}

fn decode_compact_address(decoder: &mut Decoder<'_>) -> Result<Address, String> {
    decoder.decode_from_short_bytes_ref("compact address", Address::from_bytes)
}

fn decode_stake_credential(decoder: &mut Decoder<'_>) -> Result<StakeCredential, String> {
    let tag = decoder.tag()?;
    // Haskell MemPack stores StakeCredential hash bytes verbatim (no Word64 packing).
    let hash = Hash::new(decoder.take_array()?);
    match tag {
        0 => Ok(StakeCredential::ScriptHash(hash)),
        1 => Ok(StakeCredential::AddrKeyhash(hash)),
        other => Err(format!("unsupported stake credential tag {other}")),
    }
}

fn decode_address28(decoder: &mut Decoder<'_>, stake: StakeCredential) -> Result<Address, String> {
    let extra = decoder.take_array::<32>()?;
    // Haskell MemPack lays out Addr28Extra as 4 LE Word64 values:
    //   Word64 0-2: payment hash bytes 0-23 (each Word64 is byte-reversed)
    //   Word64 3:   low  4 bytes = flags (LE u32, already correct)
    //               high 4 bytes = payment hash bytes 24-27 (LE Word32, byte-reversed)
    let flags = u32::from_le_bytes(extra[24..28].try_into().map_err(|_| "slice length checked".to_string())?);

    let mut payment_hash = [0u8; 28];
    for (i, chunk) in extra[..24].chunks(8).enumerate() {
        payment_hash[i * 8..(i + 1) * 8].copy_from_slice(chunk);
        payment_hash[i * 8..(i + 1) * 8].reverse();
    }
    payment_hash[24..].copy_from_slice(&extra[28..32]);
    payment_hash[24..].reverse();
    let payment_hash = Hash::new(payment_hash);

    let network = if flags & 0b10 != 0 { Network::Mainnet } else { Network::Testnet };
    let payment =
        if flags & 0b1 != 0 { ShelleyPaymentPart::Key(payment_hash) } else { ShelleyPaymentPart::Script(payment_hash) };
    let delegation = match stake {
        StakeCredential::AddrKeyhash(hash) => ShelleyDelegationPart::Key(hash),
        StakeCredential::ScriptHash(hash) => ShelleyDelegationPart::Script(hash),
    };

    Ok(Address::Shelley(ShelleyAddress::new(network, payment, delegation)))
}

fn decode_compact_coin(decoder: &mut Decoder<'_>) -> Result<u64, String> {
    let tag = decoder.tag()?;
    if tag != 0 {
        return Err(format!("unsupported compact coin tag {tag}"));
    }
    decoder.varuint()
}

fn decode_compact_value(decoder: &mut Decoder<'_>) -> Result<Value, String> {
    match decoder.tag()? {
        0 => Ok(Value::Coin(decoder.varuint()?)),
        1 => {
            let coin = decoder.varuint()?;
            let asset_count = decoder.varuint()? as usize;
            Ok(Value::Multiasset(
                coin,
                decoder.decode_from_short_bytes_ref("multiasset representation", |rep| {
                    decode_multiasset_rep(rep, asset_count)
                })?,
            ))
        }
        other => Err(format!("unsupported compact value tag {other}")),
    }
}

fn decode_datum(decoder: &mut Decoder<'_>) -> Result<MemoizedDatum, String> {
    match decoder.tag()? {
        0 => Ok(MemoizedDatum::None),
        1 => Ok(MemoizedDatum::Hash(decoder.hash32()?)),
        2 => Ok(MemoizedDatum::Inline(decode_inline_plutus_data(decoder)?)),
        other => Err(format!("unsupported datum tag {other}")),
    }
}

fn decode_inline_plutus_data(decoder: &mut Decoder<'_>) -> Result<MemoizedPlutusData, String> {
    decoder.decode_from_short_bytes("inline datum", MemoizedPlutusData::try_from)
}

fn decode_script(decoder: &mut Decoder<'_>) -> Result<MemoizedScript, String> {
    match decoder.tag()? {
        0 => {
            let native = decoder
                .decode_from_short_bytes("native script", |bytes| MemoizedNativeScript::try_from(Bytes::from(bytes)))?;
            Ok(MemoizedScript::NativeScript(native))
        }
        1 => {
            let tag = decoder.tag()?;
            let bytes = Bytes::from(decoder.short_bytes_vec()?);
            match tag {
                0 => Ok(MemoizedScript::PlutusV1Script(PlutusScript(bytes))),
                1 => Ok(MemoizedScript::PlutusV2Script(PlutusScript(bytes))),
                2 => Ok(MemoizedScript::PlutusV3Script(PlutusScript(bytes))),
                other => Err(format!("unsupported plutus script tag {other}")),
            }
        }
        other => Err(format!("unsupported script tag {other}")),
    }
}

// ---------------------------------------------------------- Multiasset helper

/// Decode the packed multi-asset representation stored in a compact value.
///
/// Layout (all offsets are into the `rep` blob that follows the asset count):
///
/// ```text
/// [ quantity × 8 bytes (u64 LE) ]
/// [ policy_offset × 2 bytes (u16 LE) ]
/// [ asset_offset  × 2 bytes (u16 LE) ]
/// [ policy ids and asset names packed back-to-back ]
/// ```
fn decode_multiasset_rep(rep: &[u8], asset_count: usize) -> Result<Multiasset<PositiveCoin>, String> {
    let quantity_region_end =
        asset_count.checked_mul(8).ok_or_else(|| "multiasset quantity region overflow".to_string())?;
    let policy_region_end = quantity_region_end
        .checked_add(asset_count * 2)
        .ok_or_else(|| "multiasset policy region overflow".to_string())?;
    let asset_region_end = policy_region_end
        .checked_add(asset_count * 2)
        .ok_or_else(|| "multiasset asset-name region overflow".to_string())?;

    if rep.len() < asset_region_end {
        return Err("multiasset representation is truncated".to_string());
    }

    let mut triples = Vec::with_capacity(asset_count);
    for index in 0..asset_count {
        let quantity = u64::from_le_bytes(
            rep[index * 8..index * 8 + 8].try_into().map_err(|_| "invalid quantity bytes".to_string())?,
        );
        let policy_offset = u16::from_le_bytes(
            rep[quantity_region_end + index * 2..quantity_region_end + index * 2 + 2]
                .try_into()
                .map_err(|_| "invalid policy offset bytes".to_string())?,
        ) as usize;
        let asset_offset = u16::from_le_bytes(
            rep[policy_region_end + index * 2..policy_region_end + index * 2 + 2]
                .try_into()
                .map_err(|_| "invalid asset-name offset bytes".to_string())?,
        ) as usize;

        triples.push((policy_offset, asset_offset, quantity));
    }

    let mut ordered_asset_offsets = Vec::new();
    for (_, asset_offset, _) in &triples {
        if ordered_asset_offsets.last() != Some(asset_offset) {
            ordered_asset_offsets.push(*asset_offset);
        }
    }

    let mut asset_lengths = BTreeMap::new();
    for (index, asset_offset) in ordered_asset_offsets.iter().enumerate() {
        let next_offset = ordered_asset_offsets.get(index + 1).copied().unwrap_or(rep.len());
        if *asset_offset > next_offset || next_offset > rep.len() {
            return Err("invalid asset-name offsets in multiasset representation".to_string());
        }
        asset_lengths.insert(*asset_offset, next_offset - *asset_offset);
    }

    let mut bundles: BTreeMap<Hash<28>, Vec<(Bytes, PositiveCoin)>> = BTreeMap::new();
    for (policy_offset, asset_offset, quantity) in triples {
        let policy_end = policy_offset + 28;
        if policy_end > rep.len() {
            return Err("policy id offset is out of bounds".to_string());
        }

        let asset_len = *asset_lengths.get(&asset_offset).ok_or_else(|| "missing asset-name length".to_string())?;
        let asset_end = asset_offset + asset_len;
        if asset_end > rep.len() {
            return Err("asset-name offset is out of bounds".to_string());
        }

        let quantity: PositiveCoin =
            quantity.try_into().map_err(|_| format!("invalid non-positive asset quantity {quantity}"))?;
        bundles
            .entry(Hash::from(&rep[policy_offset..policy_end]))
            .or_default()
            .push((Bytes::from(rep[asset_offset..asset_end].to_vec()), quantity));
    }

    let mut policies = Vec::with_capacity(bundles.len());
    for (policy_id, mut assets) in bundles {
        // Sort by asset name to match the canonical ordering produced by Ogmios.
        assets.sort_by(|(a, _), (b, _)| a.cmp(b));
        if assets.is_empty() {
            return Err("invalid multiasset bundle: empty asset bundle".to_string());
        }
        let assets = if assets.len() > INDEFINITE_MAP_THRESHOLD {
            MultiassetKeyValuePairs::Indef(assets)
        } else {
            MultiassetKeyValuePairs::Def(assets)
        };
        policies.push((policy_id, assets));
    }

    if policies.is_empty() {
        Err("empty multiasset representation".to_string())
    } else if policies.len() > INDEFINITE_MAP_THRESHOLD {
        Ok(MultiassetKeyValuePairs::Indef(policies))
    } else {
        Ok(MultiassetKeyValuePairs::Def(policies))
    }
}

// ============================================================ Tests
//
// Each test constructs raw mempack bytes by hand using the same wire format
// documented in cardano-ledger's `MemPack (BabbageTxOut era)` instance:
//
//   Tag 0 → TxOutCompact'               compact_addr + compact_value
//   Tag 1 → TxOutCompactDH'             compact_addr + compact_value + 32-byte data-hash
//   Tag 2 → TxOut_AddrHash28_AdaOnly    stake_cred   + addr28       + compact_coin
//   Tag 3 → TxOut_AddrHash28_AdaOnly_DataHash32
//                                       stake_cred   + addr28       + compact_coin + 32-byte data-hash
//   Tag 4 → TxOutCompactDatum           compact_addr + compact_value + inline-datum
//   Tag 5 → TxOutCompactRefScript       compact_addr + compact_value + datum + script
//
// Encoding primitives:
//   varuint     — big-endian 7-bit groups, MSB continuation bit (matching `Decoder::varuint`)
//   short_bytes — varuint(len) || bytes              (matching `Decoder::short_bytes`)
//   compact_addr — short_bytes(raw_address_bytes)
//   compact_value — tag(0 or 1) || varuint(coin) [|| varuint(n_assets) || short_bytes(rep)]
//   compact_coin  — tag(0) || varuint(coin)
//   stake_cred    — tag(0=script/1=key) || 28 bytes
//   addr28        — 32 bytes: hash[0..24] || u32-LE-flags || hash[24..28]
//                   flags: bit0=1→key-payment, bit1=1→mainnet
//
// These match the Haskell `encodeAddress28` and compact-coin MemPack instances in
// cardano-ledger's Alonzo/Babbage TxOut modules.

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        Address, Hash, MemoizedDatum, Value,
        cbor::{self, Encode},
    };

    use super::decode_transaction_output;

    // ─────────────────────────────── wire-format helpers ────────────────────

    /// Big-endian 7-bit variable-length encoding (MSB continuation bit).
    ///
    /// Mirrors `Decoder::varuint` so that these tests are self-consistent: if
    /// the encoder and decoder disagree, every test using `encode_varuint` would
    /// fail, which itself is a useful property to catch.
    fn encode_varuint(value: u64, out: &mut Vec<u8>) {
        // How many 7-bit groups do we need?
        let groups = if value == 0 { 1 } else { (63 - value.leading_zeros() as usize) / 7 + 1 };
        for i in (0..groups).rev() {
            let group = ((value >> (i * 7)) & 0x7f) as u8;
            out.push(if i > 0 { group | 0x80 } else { group });
        }
    }

    /// `varuint(len) || bytes` — the `short_bytes` wire format.
    fn short_bytes_wire(bytes: &[u8], out: &mut Vec<u8>) {
        encode_varuint(bytes.len() as u64, out);
        out.extend_from_slice(bytes);
    }

    /// `compact_value` for a coin-only output: tag byte `0x00` + varuint(coin).
    fn coin_value_wire(coin: u64, out: &mut Vec<u8>) {
        out.push(0x00);
        encode_varuint(coin, out);
    }

    fn pack_hash32_words_le(bytes: [u8; 32]) -> [u8; 32] {
        let mut packed = bytes;
        for chunk in packed.chunks_mut(8) {
            chunk.reverse();
        }
        packed
    }

    /// A 57-byte mainnet base address (key payment + key stake, all-zero hashes).
    ///
    /// Header `0x01`: Shelley type-0 (key×key) + mainnet network nibble.
    fn base_addr_bytes() -> Vec<u8> {
        let mut v = vec![0x01u8]; // type=0, network=mainnet
        v.extend_from_slice(&[0u8; 56]); // 28-byte payment hash || 28-byte stake hash
        v
    }

    // ──────────────────────── decode_transaction_output ─────────────────────

    // ── tag 0: compact address + compact value (coin only) ──

    #[test]
    fn txout_tag0_coin_only() {
        let mut bytes = vec![0x00u8]; // TxOutCompact' tag
        short_bytes_wire(&base_addr_bytes(), &mut bytes);
        coin_value_wire(1_000_000, &mut bytes);

        let out = decode_transaction_output(&bytes).unwrap();
        assert!(!out.is_legacy);
        assert_eq!(out.value, Value::Coin(1_000_000));
        assert_eq!(out.datum, MemoizedDatum::None);
        assert!(out.script.is_none());
        assert!(matches!(out.address, Address::Shelley(_)));
    }

    #[test]
    fn txout_tag0_zero_coin() {
        let mut bytes = vec![0x00u8];
        short_bytes_wire(&base_addr_bytes(), &mut bytes);
        coin_value_wire(0, &mut bytes);

        let out = decode_transaction_output(&bytes).unwrap();
        assert_eq!(out.value, Value::Coin(0));
    }

    // ── tag 0: compact address + compact value (multiasset) ──
    //
    // One policy (all-zero 28-byte id), two assets, to assert the small-bundle encoding:
    // both the outer policy map and the inner asset bundle stay definite.
    //
    // Multi-asset rep layout for 2 assets:
    //   rep[0..16]  quantity region  : [1, 2] as u64 LE
    //   rep[16..20] policy offsets   : [24, 24] as u16 LE
    //   rep[20..24] asset offsets    : [52, 53] as u16 LE
    //   rep[24..52] policy id        : 28 zero bytes
    //   rep[52]     asset name #1    : 0x61
    //   rep[53]     asset name #2    : 0x62

    #[test]
    fn txout_tag0_multiasset_small_bundle_stays_definite() {
        let asset_count = 2usize;
        let quantity_end = asset_count * 8; // 16
        let policy_end = quantity_end + asset_count * 2; // 20
        let index_end = policy_end + asset_count * 2; // 24  (≡ asset_region_end)
        let policy_offset = index_end as u16; // 12
        let asset_offset_1 = (index_end + 28) as u16; // 52
        let asset_offset_2 = asset_offset_1 + 1; // 53

        let mut rep = Vec::new();
        rep.extend_from_slice(&1u64.to_le_bytes()); // quantity = 1
        rep.extend_from_slice(&2u64.to_le_bytes()); // quantity = 2
        rep.extend_from_slice(&policy_offset.to_le_bytes());
        rep.extend_from_slice(&policy_offset.to_le_bytes());
        rep.extend_from_slice(&asset_offset_1.to_le_bytes());
        rep.extend_from_slice(&asset_offset_2.to_le_bytes());
        rep.extend_from_slice(&[0u8; 28]); // policy id
        rep.push(b'a');
        rep.push(b'b');
        assert_eq!(rep.len(), 54);

        let mut bytes = vec![0x00u8]; // TxOutCompact' tag
        short_bytes_wire(&base_addr_bytes(), &mut bytes);
        bytes.push(0x01); // multiasset compact_value tag
        encode_varuint(2_000_000, &mut bytes); // coin
        encode_varuint(asset_count as u64, &mut bytes);
        short_bytes_wire(&rep, &mut bytes);

        let out = decode_transaction_output(&bytes).unwrap();
        assert!(!out.is_legacy);
        assert!(out.script.is_none());
        match out.value {
            Value::Multiasset(coin, _) => assert_eq!(coin, 2_000_000),
            _ => panic!("expected Value::Multiasset"),
        }

        let mut encoder = cbor::Encoder::new(Vec::new());
        let mut ctx = ();
        out.encode(&mut encoder, &mut ctx).unwrap();
        let encoded = encoder.writer().clone();

        let mut expected_value = vec![0x82, 0x1a, 0x00, 0x1e, 0x84, 0x80, 0xa1, 0x58, 0x1c];
        expected_value.extend_from_slice(&[0u8; 28]);
        expected_value.extend_from_slice(&[0xa2, 0x41, b'a', 0x01, 0x41, b'b', 0x02]);

        assert!(
            encoded.windows(expected_value.len()).any(|window| window == expected_value),
            "encoded output did not contain the expected definite multiasset shape: {}",
            hex::encode(&encoded)
        );
    }

    #[test]
    fn txout_tag0_multiasset_large_bundle_uses_indefinite_inner_map() {
        let asset_count = 24usize;
        let quantity_end = asset_count * 8;
        let policy_end = quantity_end + asset_count * 2;
        let index_end = policy_end + asset_count * 2;
        let policy_offset = index_end as u16;
        let mut next_asset_offset = index_end as u16 + 28;

        let mut rep = Vec::new();
        for quantity in 1..=asset_count as u64 {
            rep.extend_from_slice(&quantity.to_le_bytes());
        }
        for _ in 0..asset_count {
            rep.extend_from_slice(&policy_offset.to_le_bytes());
        }
        for _ in 0..asset_count {
            rep.extend_from_slice(&next_asset_offset.to_le_bytes());
            next_asset_offset += 1;
        }
        rep.extend_from_slice(&[0u8; 28]);
        for asset_name in b'a'..=b'x' {
            rep.push(asset_name);
        }

        let mut bytes = vec![0x00u8];
        short_bytes_wire(&base_addr_bytes(), &mut bytes);
        bytes.push(0x01);
        encode_varuint(2_000_000, &mut bytes);
        encode_varuint(asset_count as u64, &mut bytes);
        short_bytes_wire(&rep, &mut bytes);

        let out = decode_transaction_output(&bytes).unwrap();
        let mut encoder = cbor::Encoder::new(Vec::new());
        let mut ctx = ();
        out.encode(&mut encoder, &mut ctx).unwrap();
        let encoded = encoder.writer().clone();

        let mut expected_prefix = vec![0x82, 0x1a, 0x00, 0x1e, 0x84, 0x80, 0xa1, 0x58, 0x1c];
        expected_prefix.extend_from_slice(&[0u8; 28]);
        expected_prefix.push(0xbf);

        assert!(
            encoded.windows(expected_prefix.len()).any(|window| window == expected_prefix),
            "encoded output did not contain the expected indefinite inner map prefix: {}",
            hex::encode(&encoded)
        );
    }

    #[test]
    fn txout_tag0_multiasset_large_policy_set_uses_indefinite_outer_map() {
        let asset_count = 24usize;
        let quantity_end = asset_count * 8;
        let policy_end = quantity_end + asset_count * 2;
        let index_end = policy_end + asset_count * 2;
        let mut next_policy_offset = index_end as u16;
        let asset_offset = index_end as u16 + (28 * asset_count) as u16;

        let mut rep = Vec::new();
        for quantity in 1..=asset_count as u64 {
            rep.extend_from_slice(&quantity.to_le_bytes());
        }
        for _ in 0..asset_count {
            rep.extend_from_slice(&next_policy_offset.to_le_bytes());
            next_policy_offset += 28;
        }
        for _ in 0..asset_count {
            rep.extend_from_slice(&asset_offset.to_le_bytes());
        }
        for policy_byte in 0u8..asset_count as u8 {
            rep.extend_from_slice(&[policy_byte; 28]);
        }
        rep.push(b'a');

        let mut bytes = vec![0x00u8];
        short_bytes_wire(&base_addr_bytes(), &mut bytes);
        bytes.push(0x01);
        encode_varuint(2_000_000, &mut bytes);
        encode_varuint(asset_count as u64, &mut bytes);
        short_bytes_wire(&rep, &mut bytes);

        let out = decode_transaction_output(&bytes).unwrap();
        let mut encoder = cbor::Encoder::new(Vec::new());
        let mut ctx = ();
        out.encode(&mut encoder, &mut ctx).unwrap();
        let encoded = encoder.writer().clone();

        let expected_prefix = [0x82, 0x1a, 0x00, 0x1e, 0x84, 0x80, 0xbf];

        assert!(
            encoded.windows(expected_prefix.len()).any(|window| window == expected_prefix),
            "encoded output did not contain the expected indefinite outer map prefix: {}",
            hex::encode(&encoded)
        );
    }

    // ── tag 1: compact address + compact value + 32-byte datum hash ──

    #[test]
    fn txout_tag1_datum_hash() {
        let datum_hash_bytes = [
            0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let datum_hash = Hash::from(&datum_hash_bytes[..]);

        let mut bytes = vec![0x01u8]; // TxOutCompactDH' tag
        short_bytes_wire(&base_addr_bytes(), &mut bytes);
        coin_value_wire(500_000, &mut bytes);
        bytes.extend_from_slice(&datum_hash[..]); // 32-byte data hash (no length prefix)

        let out = decode_transaction_output(&bytes).unwrap();
        assert!(!out.is_legacy);
        assert_eq!(out.value, Value::Coin(500_000));
        assert_eq!(out.datum, MemoizedDatum::Hash(datum_hash));
        assert!(out.script.is_none());
    }

    // ── tag 2: stake cred + addr28 + compact coin ──
    //
    // addr28 layout (32 bytes):
    //   [0..24]  first 24 bytes of payment hash
    //   [24..28] u32 LE flags: bit0=1→key-payment, bit1=1→mainnet
    //   [28..32] last 4 bytes of payment hash

    #[test]
    fn txout_tag2_mainnet_key_payment() {
        let mut addr28 = [0u8; 32];
        addr28[24] = 0x03; // bit0=1 (key payment) | bit1=1 (mainnet)

        let mut bytes = vec![0x02u8]; // TxOut_AddrHash28_AdaOnly tag
        bytes.push(0x01); // stake credential: key hash
        bytes.extend_from_slice(&[0u8; 28]); // stake key hash
        bytes.extend_from_slice(&addr28);
        bytes.push(0x00); // compact-coin tag
        encode_varuint(1_000_000, &mut bytes);

        let out = decode_transaction_output(&bytes).unwrap();
        assert!(!out.is_legacy);
        assert_eq!(out.value, Value::Coin(1_000_000));
        assert_eq!(out.datum, MemoizedDatum::None);
        assert!(out.script.is_none());
        assert!(matches!(out.address, Address::Shelley(_)));
    }

    #[test]
    fn txout_tag2_testnet_script_payment() {
        // flags = 0x00: bit0=0 (script payment), bit1=0 (testnet)
        let addr28 = [0u8; 32];

        let mut bytes = vec![0x02u8];
        bytes.push(0x00); // stake credential: script hash
        bytes.extend_from_slice(&[0u8; 28]);
        bytes.extend_from_slice(&addr28);
        bytes.push(0x00); // compact-coin tag
        encode_varuint(100, &mut bytes);

        let out = decode_transaction_output(&bytes).unwrap();
        assert_eq!(out.value, Value::Coin(100));
        assert!(matches!(out.address, Address::Shelley(_)));
    }

    // ── tag 3: stake cred + addr28 + compact coin + datum hash ──

    #[test]
    fn txout_tag3_addr28_datum_hash() {
        let mut addr28 = [0u8; 32];
        addr28[24] = 0x03;
        let datum_hash_bytes = [
            0x00u8, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0x10,
            0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e, 0x1f,
        ];
        let datum_hash = Hash::from(&datum_hash_bytes[..]);

        let mut bytes = vec![0x03u8]; // TxOut_AddrHash28_AdaOnly_DataHash32 tag
        bytes.push(0x01); // stake credential: key hash
        bytes.extend_from_slice(&[0u8; 28]);
        bytes.extend_from_slice(&addr28);
        bytes.push(0x00); // compact-coin tag
        encode_varuint(777_777, &mut bytes);
        bytes.extend_from_slice(&pack_hash32_words_le(datum_hash_bytes));

        let out = decode_transaction_output(&bytes).unwrap();
        assert_eq!(out.value, Value::Coin(777_777));
        assert_eq!(out.datum, MemoizedDatum::Hash(datum_hash));
        assert!(matches!(out.address, Address::Shelley(_)));
    }

    // ── varuint boundary values ──

    #[test]
    fn txout_varuint_single_byte_boundary() {
        // varuint(127) fits in one byte (0x7F); varuint(128) needs two bytes (0x81 0x00).
        for (coin, expected_wire) in [
            (0u64, [0x00u8, 0x00].as_slice()), // compact_value: tag=0, varuint(0)=0x00
            (127, &[0x00, 0x7f]),              // varuint(127)=0x7F
            (128, &[0x00, 0x81, 0x00]),        // varuint(128)=0x81 0x00
        ] {
            let mut bytes = vec![0x00u8]; // tag 0
            short_bytes_wire(&base_addr_bytes(), &mut bytes);
            bytes.extend_from_slice(expected_wire); // raw compact_value bytes

            let out = decode_transaction_output(&bytes).unwrap();
            assert_eq!(out.value, Value::Coin(coin), "coin={coin}");
        }
    }

    // ── error cases ──

    #[test]
    fn txout_unknown_tag_errors() {
        assert!(decode_transaction_output(&[0x06]).is_err());
        assert!(decode_transaction_output(&[0xFF]).is_err());
    }

    #[test]
    fn txout_empty_buffer_errors() {
        assert!(decode_transaction_output(&[]).is_err());
    }

    #[test]
    fn txout_truncated_after_tag_errors() {
        // Tag 0 but no address bytes.
        assert!(decode_transaction_output(&[0x00]).is_err());
    }

    #[test]
    fn txout_truncated_address_errors() {
        // Tag 0, address length says 57 but we only supply 10 bytes.
        let mut bytes = vec![0x00u8, 0x39]; // tag + varuint(57)
        bytes.extend_from_slice(&[0u8; 10]);
        assert!(decode_transaction_output(&bytes).is_err());
    }

    #[test]
    fn txout_unknown_compact_value_tag_errors() {
        let mut bytes = vec![0x00u8]; // tag 0
        short_bytes_wire(&base_addr_bytes(), &mut bytes);
        bytes.push(0x02); // tag 2 is not a valid compact_value tag
        bytes.extend_from_slice(&[0u8; 8]);
        assert!(decode_transaction_output(&bytes).is_err());
    }

    #[test]
    fn txout_unknown_compact_coin_tag_errors() {
        // Tag 2 path; compact_coin_with_tag must have tag byte == 0.
        let mut bytes = vec![0x02u8];
        bytes.push(0x01); // stake cred: key hash
        bytes.extend_from_slice(&[0u8; 28]);
        bytes.extend_from_slice(&[0u8; 32]); // addr28
        bytes.push(0x01); // compact-coin tag != 0 → error
        bytes.extend_from_slice(&[0u8; 4]);
        assert!(decode_transaction_output(&bytes).is_err());
    }

    #[test]
    fn txout_unknown_stake_cred_tag_errors() {
        let mut bytes = vec![0x02u8];
        bytes.push(0x05); // unsupported stake credential tag
        bytes.extend_from_slice(&[0u8; 28]);
        bytes.extend_from_slice(&[0u8; 32]);
        bytes.push(0x00);
        encode_varuint(0, &mut bytes);
        assert!(decode_transaction_output(&bytes).is_err());
    }
}
