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

use amaru_kernel::{
    Address, AddressPointer, AssetName, Assets, Bytes, Credential, Hash, MemoizedDatum, MemoizedPlutusData,
    MemoizedScript, MemoizedTransactionOutput, Multiasset, Network, PlutusScript, PositiveCoin, ShelleyAddress,
    StakeReference, Value, from_cbor,
};
use anyhow::{Context, anyhow};

const MAX_VARUINT64_BYTES: usize = 10;

pub fn decode_transaction_output(bytes: &[u8]) -> anyhow::Result<MemoizedTransactionOutput> {
    let mut decoder = Decoder::new(bytes);

    let tag = decoder.tag()?;
    let (address, value) = match tag {
        0 | 1 | 4 | 5 => (decode_compact_address(&mut decoder)?, decode_compact_value(&mut decoder)?),
        2 | 3 => {
            let stake = decode_stake_credential(&mut decoder)?;
            (decode_address28(&mut decoder, stake)?, Value::Coin(decode_compact_coin(&mut decoder)?))
        }
        tag => return Err(anyhow!("unsupported BabbageTxOut mempack tag {tag}")),
    };
    let datum = match tag {
        1 => MemoizedDatum::from(decoder.hash32()?),
        3 => MemoizedDatum::from(decoder.packed_hash32()?),
        4 => MemoizedDatum::from(decode_inline_plutus_data(&mut decoder)?),
        5 => decode_datum(&mut decoder)?,
        _ => MemoizedDatum::None,
    };
    let script = (tag == 5).then(|| decode_script(&mut decoder)).transpose()?;
    Ok(MemoizedTransactionOutput::new(address, value, datum, script))
}

struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn tag(&mut self) -> anyhow::Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn take(&mut self, len: usize) -> anyhow::Result<&'a [u8]> {
        let end = self.offset.checked_add(len).ok_or_else(|| anyhow!("mempack offset overflow"))?;
        if end > self.bytes.len() {
            return Err(anyhow!("unexpected end of mempack data at {} while reading {} bytes", self.offset, len));
        }
        let slice = &self.bytes[self.offset..end];
        self.offset = end;
        Ok(slice)
    }

    fn take_array<const N: usize>(&mut self) -> anyhow::Result<[u8; N]> {
        let mut bytes = [0u8; N];
        bytes.copy_from_slice(self.take(N)?);
        Ok(bytes)
    }

    fn varuint(&mut self, what: &str) -> anyhow::Result<u64> {
        let mut value = 0_u64;

        for _ in 0..MAX_VARUINT64_BYTES {
            let byte = self.tag()?;
            value = value
                .checked_mul(0x80)
                .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
                .ok_or_else(|| anyhow!("{what} overflows u64"))?;

            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }

        Err(anyhow!("{what} exceeds {MAX_VARUINT64_BYTES} bytes"))
    }

    fn short_bytes(&mut self) -> anyhow::Result<&'a [u8]> {
        let len = self.varuint("mempack varuint")? as usize;
        self.take(len)
    }

    fn hash32(&mut self) -> anyhow::Result<Hash<32>> {
        Ok(Hash::new(self.take_array()?))
    }

    fn packed_hash32(&mut self) -> anyhow::Result<Hash<32>> {
        let mut hash = self.take_array::<32>()?;
        for chunk in hash.as_chunks_mut::<8>().0 {
            chunk.reverse();
        }
        Ok(Hash::new(hash))
    }
}

fn decode_compact_address(decoder: &mut Decoder<'_>) -> anyhow::Result<Address> {
    let bytes = decoder.short_bytes()?;
    let address = Address::from_bytes(bytes).ok_or_else(|| anyhow!("invalid compact address"))?;
    if let Address::Shelley(shelley) = &address
        && matches!(shelley.delegation(), Some(StakeReference::Pointer(_)))
    {
        // The address parser clamps overflow; mempack requires bounded decoding before normalization.
        let mut decoder = Decoder::new(&bytes[29..]);
        let slot = decoder.varuint("compact address slot")?;
        let transaction = decoder.varuint("compact address tx index")?;
        let certificate = decoder.varuint("compact address certificate index")?;
        let pointer = if u32::try_from(slot).is_ok()
            && u16::try_from(transaction).is_ok()
            && u16::try_from(certificate).is_ok()
        {
            AddressPointer { slot, transaction, certificate }
        } else {
            AddressPointer { slot: 0, transaction: 0, certificate: 0 }
        };
        return Ok(Address::Shelley(ShelleyAddress::new(
            shelley.network(),
            *shelley.payment(),
            Some(StakeReference::Pointer(pointer)),
        )));
    }
    Ok(address)
}

fn decode_stake_credential(decoder: &mut Decoder<'_>) -> anyhow::Result<Credential> {
    let tag = decoder.tag()?;
    let hash = Hash::new(decoder.take_array()?);
    match tag {
        0 => Ok(Credential::ScriptHash(hash)),
        1 => Ok(Credential::KeyHash(hash)),
        other => Err(anyhow!("unsupported stake credential tag {other}")),
    }
}

fn decode_address28(decoder: &mut Decoder<'_>, stake: Credential) -> anyhow::Result<Address> {
    let extra = decoder.take_array::<32>()?;
    let flags = u32::from_le_bytes(extra[24..28].try_into().map_err(|_| anyhow!("slice length checked"))?);

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
        if flags & 0b1 != 0 { Credential::KeyHash(payment_hash) } else { Credential::ScriptHash(payment_hash) };

    Ok(Address::Shelley(ShelleyAddress::new(network, payment, Some(StakeReference::Credential(stake)))))
}

fn decode_compact_coin(decoder: &mut Decoder<'_>) -> anyhow::Result<u64> {
    let tag = decoder.tag()?;
    if tag != 0 {
        return Err(anyhow!("unsupported compact coin tag {tag}"));
    }
    decoder.varuint("mempack varuint")
}

fn decode_compact_value(decoder: &mut Decoder<'_>) -> anyhow::Result<Value> {
    match decoder.tag()? {
        0 => Ok(Value::Coin(decoder.varuint("mempack varuint")?)),
        1 => {
            let coin = decoder.varuint("mempack varuint")?;
            let asset_count = decoder.varuint("mempack varuint")? as usize;
            Ok(Value::Multiasset(
                coin,
                decode_multiasset_rep(decoder.short_bytes()?, asset_count)
                    .context("invalid multiasset representation")?,
            ))
        }
        other => Err(anyhow!("unsupported compact value tag {other}")),
    }
}

fn decode_datum(decoder: &mut Decoder<'_>) -> anyhow::Result<MemoizedDatum> {
    match decoder.tag()? {
        0 => Ok(MemoizedDatum::None),
        1 => Ok(MemoizedDatum::from(decoder.hash32()?)),
        2 => Ok(MemoizedDatum::from(decode_inline_plutus_data(decoder)?)),
        other => Err(anyhow!("unsupported datum tag {other}")),
    }
}

fn decode_inline_plutus_data(decoder: &mut Decoder<'_>) -> anyhow::Result<MemoizedPlutusData> {
    from_cbor(decoder.short_bytes()?).context("failed to decode inline PlutusData from CBOR")
}

fn decode_script(decoder: &mut Decoder<'_>) -> anyhow::Result<MemoizedScript> {
    match decoder.tag()? {
        0 => {
            let native = from_cbor(decoder.short_bytes()?).context("failed to decode native Script from CBOR")?;
            Ok(MemoizedScript::NativeScript(native))
        }
        1 => {
            let tag = decoder.tag()?;
            let bytes = Bytes::from(decoder.short_bytes()?.to_vec());
            match tag {
                0 => Ok(MemoizedScript::PlutusV1Script(PlutusScript(bytes))),
                1 => Ok(MemoizedScript::PlutusV2Script(PlutusScript(bytes))),
                2 => Ok(MemoizedScript::PlutusV3Script(PlutusScript(bytes))),
                other => Err(anyhow!("unsupported plutus script tag {other}")),
            }
        }
        other => Err(anyhow!("unsupported script tag {other}")),
    }
}

fn decode_multiasset_rep(rep: &[u8], asset_count: usize) -> anyhow::Result<Multiasset<PositiveCoin>> {
    let quantity_region_end =
        asset_count.checked_mul(8).ok_or_else(|| anyhow!("multiasset quantity region overflow"))?;
    let policy_region_end =
        quantity_region_end.checked_add(asset_count * 2).ok_or_else(|| anyhow!("multiasset policy region overflow"))?;
    let asset_region_end = policy_region_end
        .checked_add(asset_count * 2)
        .ok_or_else(|| anyhow!("multiasset asset-name region overflow"))?;

    if rep.len() < asset_region_end {
        return Err(anyhow!("multiasset representation is truncated"));
    }

    let mut triples = Vec::with_capacity(asset_count);
    for index in 0..asset_count {
        let quantity = u64::from_le_bytes(
            rep[index * 8..index * 8 + 8].try_into().map_err(|_| anyhow!("invalid quantity bytes"))?,
        );
        let policy_offset = u16::from_le_bytes(
            rep[quantity_region_end + index * 2..quantity_region_end + index * 2 + 2]
                .try_into()
                .map_err(|_| anyhow!("invalid policy offset bytes"))?,
        ) as usize;
        let asset_offset = u16::from_le_bytes(
            rep[policy_region_end + index * 2..policy_region_end + index * 2 + 2]
                .try_into()
                .map_err(|_| anyhow!("invalid asset-name offset bytes"))?,
        ) as usize;

        triples.push((policy_offset, asset_offset, quantity));
    }

    let mut asset_offsets: Vec<_> = triples.iter().map(|(_, offset, _)| *offset).collect();
    asset_offsets.dedup();
    asset_offsets.push(rep.len());
    let asset_names = asset_offsets
        .windows(2)
        .map(|bounds| {
            let bytes =
                rep.get(bounds[0]..bounds[1]).context("invalid asset-name offsets in multiasset representation")?;
            let name = AssetName::try_from(bytes).map_err(|_| anyhow!("invalid asset name at offset {}", bounds[0]))?;
            Ok((bounds[0], name))
        })
        .collect::<anyhow::Result<BTreeMap<_, _>>>()?;

    let mut bundles: BTreeMap<Hash<28>, BTreeMap<AssetName, PositiveCoin>> = BTreeMap::new();
    for (policy_offset, asset_offset, quantity) in triples {
        let policy = rep.get(policy_offset..policy_offset + 28).context("policy id offset is out of bounds")?;
        let policy_id =
            Hash::try_from(policy).map_err(|e| anyhow!("invalid policy id in multiasset representation: {e}"))?;
        let asset = *asset_names.get(&asset_offset).context("missing asset name")?;
        let quantity = quantity.try_into().map_err(|_| anyhow!("invalid non-positive asset quantity {quantity}"))?;
        bundles.entry(policy_id).or_default().insert(asset, quantity);
    }

    let mut policies = BTreeMap::new();
    for (policy_id, assets) in bundles {
        let assets = Assets::try_from(assets).map_err(|e| anyhow!("policy {policy_id}: {e}"))?;
        policies.insert(policy_id, assets);
    }

    Ok(policies.into())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Address, MemoizedDatum, MemoizedScript, Value, cbor};

    use super::{Decoder, decode_multiasset_rep, decode_transaction_output};

    #[test]
    fn decodes_legacy_and_modern_outputs() {
        let address = Address::from_hex("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335").unwrap();
        let address_bytes = address.to_vec();
        // A legacy output is written as an array of its fields, a modern one as a map of the fields it
        // carries; both in the definite-length form the ledger uses.
        for (tag, suffix, head) in
            [(0, vec![], 0x82), (1, vec![3; 32], 0x83), (4, vec![1, 0], 0xa3), (5, vec![0, 1, 2, 1, 42], 0xa3)]
        {
            let mut bytes = vec![tag, address_bytes.len() as u8];
            bytes.extend_from_slice(&address_bytes);
            bytes.extend_from_slice(&[0, 42]);
            bytes.extend_from_slice(&suffix);

            let output = decode_transaction_output(&bytes).unwrap();
            assert_eq!(output.address, address);
            assert_eq!(output.value, Value::Coin(42));
            match tag {
                1 => assert_eq!(output.datum, MemoizedDatum::from(amaru_kernel::Hash::new([3; 32]))),
                4 => assert!(matches!(output.datum, MemoizedDatum::Inline(_))),
                _ => assert_eq!(output.datum, MemoizedDatum::None),
            }
            if tag == 5 {
                assert!(matches!(output.script.as_deref(), Some(MemoizedScript::PlutusV3Script(_))));
            } else {
                assert!(output.script.is_none());
            }
            assert_eq!(cbor::to_vec(output).unwrap()[0], head);
        }
    }

    #[test]
    fn normalizes_pointer_compact_addresses() {
        let prefix = "412813b99a80cfb3f1cf95653b169b17035963544837b7ce33d30710a8";
        for (pointer, expected) in [
            ("e710a09072c78ccf01", Some("000000")),
            ("81000102", Some("81000102")),
            ("8fffffff7f83ff7f83ff7f", Some("8fffffff7f83ff7f83ff7f")),
            ("828080808080808080000000", None),
            ("80808080808080808080000000", None),
        ] {
            let compact_address = hex::decode(format!("{prefix}{pointer}")).unwrap();
            let mut bytes = vec![0, compact_address.len() as u8];
            bytes.extend_from_slice(&compact_address);
            bytes.extend_from_slice(&[0, 42]);
            let output = decode_transaction_output(&bytes);
            match expected {
                Some(pointer) => assert_eq!(output.unwrap().address.to_hex(), format!("{prefix}{pointer}")),
                None => assert!(output.is_err()),
            }
        }
    }

    #[test]
    fn decodes_shared_multiasset_names_and_rejects_invalid_offsets() {
        let mut rep: Vec<u8> = [7_u64, 11, 13].into_iter().flat_map(u64::to_le_bytes).collect();
        rep.extend([36_u16, 64, 36, 92, 92, 93].into_iter().flat_map(u16::to_le_bytes));
        rep.extend([1; 28]);
        rep.extend([2; 28]);
        rep.extend(b"ab");
        let assets = decode_multiasset_rep(&rep, 3).unwrap();
        for (policy, name, quantity) in [(1, b'a', 7), (2, b'a', 11), (1, b'b', 13)] {
            let policy = assets.get(&amaru_kernel::Hash::new([policy; 28])).unwrap();
            assert_eq!(u64::from(policy.get(&[name].as_slice().try_into().unwrap()).unwrap()), quantity);
        }
        for (offset, invalid) in [(24, 100_u16), (30, 95), (34, 91)] {
            let mut corrupt = rep.clone();
            corrupt[offset..offset + 2].copy_from_slice(&invalid.to_le_bytes());
            assert!(decode_multiasset_rep(&corrupt, 3).is_err());
        }
        assert!(decode_multiasset_rep(&rep[..35], 3).is_err());
    }

    #[test]
    fn rejects_overlong_mempack_varuints() {
        let mut decoder = Decoder::new(&[0x80; 10]);

        assert_eq!(decoder.varuint("mempack varuint").unwrap_err().to_string(), "mempack varuint exceeds 10 bytes");
    }

    #[test]
    fn rejects_overflowing_pointer_varuints() {
        let bytes = [0x82, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00];

        assert_eq!(
            Decoder::new(&bytes).varuint("compact address slot").unwrap_err().to_string(),
            "compact address slot overflows u64"
        );
    }
}
