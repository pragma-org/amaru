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
    Address, Bytes, Hash, MemoizedDatum, MemoizedNativeScript, MemoizedPlutusData, MemoizedScript,
    MemoizedTransactionOutput, MemoizedValue, Multiasset, MultiassetKeyValuePairs, Network, PlutusScript, PositiveCoin,
    ShelleyAddress, ShelleyDelegationPart, ShelleyPaymentPart, StakeCredential, Value,
};

const INDEFINITE_MAP_THRESHOLD: usize = 23;

pub fn decode_transaction_output(bytes: &[u8]) -> Result<MemoizedTransactionOutput, String> {
    let mut decoder = Decoder::new(bytes);

    match decoder.tag()? {
        0 => make_transaction_output(
            true,
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            MemoizedDatum::None,
            None,
        ),
        1 => make_transaction_output(
            true,
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            MemoizedDatum::Hash(decoder.hash32()?),
            None,
        ),
        2 => {
            let stake = decode_stake_credential(&mut decoder)?;
            make_transaction_output(
                true,
                decode_address28(&mut decoder, stake)?,
                Value::Coin(decode_compact_coin(&mut decoder)?),
                MemoizedDatum::None,
                None,
            )
        }
        3 => {
            let stake = decode_stake_credential(&mut decoder)?;
            make_transaction_output(
                true,
                decode_address28(&mut decoder, stake)?,
                Value::Coin(decode_compact_coin(&mut decoder)?),
                MemoizedDatum::Hash(decoder.packed_hash32()?),
                None,
            )
        }
        4 => make_transaction_output(
            false,
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            MemoizedDatum::Inline(decode_inline_plutus_data(&mut decoder)?),
            None,
        ),
        5 => make_transaction_output(
            false,
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            decode_datum(&mut decoder)?,
            Some(decode_script(&mut decoder)?),
        ),
        tag => Err(format!("unsupported BabbageTxOut mempack tag {tag}")),
    }
}

fn make_transaction_output(
    is_legacy: bool,
    address: Address,
    value: Value,
    datum: MemoizedDatum,
    script: Option<MemoizedScript>,
) -> Result<MemoizedTransactionOutput, String> {
    let value = MemoizedValue::new(value)?;

    Ok(MemoizedTransactionOutput::new(is_legacy, address, value, datum, script))
}

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
    let hash = Hash::new(decoder.take_array()?);
    match tag {
        0 => Ok(StakeCredential::ScriptHash(hash)),
        1 => Ok(StakeCredential::AddrKeyhash(hash)),
        other => Err(format!("unsupported stake credential tag {other}")),
    }
}

fn decode_address28(decoder: &mut Decoder<'_>, stake: StakeCredential) -> Result<Address, String> {
    let extra = decoder.take_array::<32>()?;
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

#[cfg(test)]
mod tests {
    use amaru_kernel::{Address, MemoizedDatum, Value, cbor};

    use super::decode_transaction_output;

    #[test]
    fn decodes_tag_zero_outputs_as_legacy() {
        let address = Address::from_hex("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335").unwrap();
        let address_bytes = address.to_vec();

        let mut bytes = vec![0, address_bytes.len() as u8];
        bytes.extend_from_slice(&address_bytes);
        bytes.push(0);
        bytes.push(42);

        let output = decode_transaction_output(&bytes).unwrap();

        assert!(output.is_legacy);
        assert_eq!(output.address, address);
        assert_eq!(output.value.as_ref(), &Value::Coin(42));
        assert_eq!(cbor::to_vec(output).unwrap()[0], 0x9f);
    }

    #[test]
    fn decodes_tag_four_outputs_as_modern() {
        let address = Address::from_hex("61bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335").unwrap();
        let address_bytes = address.to_vec();

        let mut bytes = vec![4, address_bytes.len() as u8];
        bytes.extend_from_slice(&address_bytes);
        bytes.push(0);
        bytes.push(99);
        bytes.push(1);
        bytes.push(0);

        let output = decode_transaction_output(&bytes).unwrap();

        assert!(!output.is_legacy);
        assert_eq!(output.address, address);
        assert_eq!(output.value.as_ref(), &Value::Coin(99));
        assert!(matches!(output.datum, MemoizedDatum::Inline(_)));
        assert_eq!(cbor::to_vec(output).unwrap()[0], 0xbf);
    }
}
