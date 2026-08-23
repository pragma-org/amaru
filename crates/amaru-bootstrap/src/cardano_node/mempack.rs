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
    Address, AssetName, Bytes, Hash, MemoizedDatum, MemoizedPlutusData, MemoizedScript, MemoizedTransactionOutput,
    Multiasset, Network, NonEmptyKeyValuePairs, PlutusScript, PositiveCoin, ShelleyAddress, ShelleyDelegationPart,
    ShelleyPaymentPart, StakeCredential, Value, from_cbor,
};
use anyhow::anyhow;

const MAX_VARUINT64_BYTES: usize = 10;

pub fn decode_transaction_output(bytes: &[u8]) -> anyhow::Result<MemoizedTransactionOutput> {
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
            MemoizedDatum::from(decoder.hash32()?),
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
                MemoizedDatum::from(decoder.packed_hash32()?),
                None,
            )
        }
        4 => make_transaction_output(
            false,
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            MemoizedDatum::from(decode_inline_plutus_data(&mut decoder)?),
            None,
        ),
        5 => make_transaction_output(
            false,
            decode_compact_address(&mut decoder)?,
            decode_compact_value(&mut decoder)?,
            decode_datum(&mut decoder)?,
            Some(decode_script(&mut decoder)?),
        ),
        tag => Err(anyhow!("unsupported BabbageTxOut mempack tag {tag}")),
    }
}

fn make_transaction_output(
    is_legacy: bool,
    address: Address,
    value: Value,
    datum: MemoizedDatum,
    script: Option<MemoizedScript>,
) -> anyhow::Result<MemoizedTransactionOutput> {
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

    fn varuint(&mut self) -> anyhow::Result<u64> {
        decode_varuint64_with(|| self.tag(), "mempack varuint")
    }

    fn short_bytes(&mut self) -> anyhow::Result<&'a [u8]> {
        let len = self.varuint()? as usize;
        self.take(len)
    }

    fn short_bytes_vec(&mut self) -> anyhow::Result<Vec<u8>> {
        Ok(self.short_bytes()?.to_vec())
    }

    fn decode_from_short_bytes<T, E, F>(&mut self, what: &str, decode: F) -> anyhow::Result<T>
    where
        E: std::fmt::Display,
        F: FnOnce(Vec<u8>) -> Result<T, E>,
    {
        decode(self.short_bytes_vec()?).map_err(|err| anyhow!("invalid {what}: {err}"))
    }

    fn decode_from_short_bytes_ref<T, E, F>(&mut self, what: &str, decode: F) -> anyhow::Result<T>
    where
        E: std::fmt::Display,
        F: FnOnce(&[u8]) -> Result<T, E>,
    {
        decode(self.short_bytes()?).map_err(|err| anyhow!("invalid {what}: {err}"))
    }

    fn hash32(&mut self) -> anyhow::Result<Hash<32>> {
        Ok(Hash::new(self.take_array()?))
    }

    fn packed_hash32(&mut self) -> anyhow::Result<Hash<32>> {
        let bytes = self.take_array::<32>()?;
        let mut hash = [0u8; 32];
        for (index, chunk) in bytes.chunks(8).enumerate() {
            hash[index * 8..(index + 1) * 8].copy_from_slice(chunk);
            hash[index * 8..(index + 1) * 8].reverse();
        }
        Ok(Hash::new(hash))
    }
}

fn decode_compact_address(decoder: &mut Decoder<'_>) -> anyhow::Result<Address> {
    let bytes = decoder.short_bytes()?;
    let normalized = normalize_compact_address(bytes)?;

    Address::from_bytes(&normalized).ok_or_else(|| anyhow!("invalid compact address"))
}

fn normalize_compact_address(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    if !is_pointer_compact_address(bytes) {
        return Ok(bytes.to_vec());
    }

    let mut offset = 29;
    let slot = decode_varuint64(bytes, &mut offset, "slot")?;
    let tx_index = decode_varuint64(bytes, &mut offset, "tx index")?;
    let cert_index = decode_varuint64(bytes, &mut offset, "certificate index")?;

    let (slot, tx_index, cert_index) = normalize_pointer(slot, tx_index, cert_index);

    let mut normalized = Vec::with_capacity(bytes.len() + 8);
    normalized.extend_from_slice(&bytes[..29]);
    encode_varuint64(slot, &mut normalized);
    encode_varuint64(tx_index, &mut normalized);
    encode_varuint64(cert_index, &mut normalized);

    Ok(normalized)
}

fn is_pointer_compact_address(bytes: &[u8]) -> bool {
    if bytes.len() < 29 {
        return false;
    }

    let header = bytes[0];
    let is_byron = header == 0x82;
    let is_account = header & 0b1110_1110 == 0b1110_0000;
    let is_not_base = header & 0b0100_0000 != 0;
    let is_enterprise = header & 0b0010_0000 != 0;

    !is_byron && !is_account && is_not_base && !is_enterprise
}

fn decode_varuint64(bytes: &[u8], offset: &mut usize, field_name: &str) -> anyhow::Result<u64> {
    decode_varuint64_with(
        || {
            if *offset >= bytes.len() {
                return Err(anyhow!("unexpected end of compact address while decoding {field_name}"));
            }

            let byte = bytes[*offset];
            *offset += 1;
            Ok(byte)
        },
        &format!("compact address {field_name}"),
    )
}

fn decode_varuint64_with<F>(mut next_byte: F, what: &str) -> anyhow::Result<u64>
where
    F: FnMut() -> anyhow::Result<u8>,
{
    let mut value = 0_u64;

    for byte_index in 0..MAX_VARUINT64_BYTES {
        let byte = next_byte()?;
        value = value
            .checked_mul(0x80)
            .and_then(|value| value.checked_add(u64::from(byte & 0x7f)))
            .ok_or_else(|| anyhow!("{what} overflows u64"))?;

        if byte & 0x80 == 0 {
            return Ok(value);
        }

        if byte_index + 1 == MAX_VARUINT64_BYTES {
            return Err(anyhow!("{what} exceeds {MAX_VARUINT64_BYTES} bytes"));
        }
    }

    unreachable!()
}

fn normalize_pointer(slot: u64, tx_index: u64, cert_index: u64) -> (u64, u64, u64) {
    if u32::try_from(slot).is_ok() && u16::try_from(tx_index).is_ok() && u16::try_from(cert_index).is_ok() {
        (slot, tx_index, cert_index)
    } else {
        (0, 0, 0)
    }
}

fn encode_varuint64(mut value: u64, out: &mut Vec<u8>) {
    let mut chunks = [0_u8; 10];
    let mut len = 0;

    loop {
        chunks[len] = (value & 0x7f) as u8;
        len += 1;
        value >>= 7;

        if value == 0 {
            break;
        }
    }

    for index in (0..len).rev() {
        let mut byte = chunks[index];
        if index != 0 {
            byte |= 0x80;
        }
        out.push(byte);
    }
}

fn decode_stake_credential(decoder: &mut Decoder<'_>) -> anyhow::Result<StakeCredential> {
    let tag = decoder.tag()?;
    let hash = Hash::new(decoder.take_array()?);
    match tag {
        0 => Ok(StakeCredential::ScriptHash(hash)),
        1 => Ok(StakeCredential::AddrKeyhash(hash)),
        other => Err(anyhow!("unsupported stake credential tag {other}")),
    }
}

fn decode_address28(decoder: &mut Decoder<'_>, stake: StakeCredential) -> anyhow::Result<Address> {
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
        if flags & 0b1 != 0 { ShelleyPaymentPart::Key(payment_hash) } else { ShelleyPaymentPart::Script(payment_hash) };
    let delegation = match stake {
        StakeCredential::AddrKeyhash(hash) => ShelleyDelegationPart::Key(hash),
        StakeCredential::ScriptHash(hash) => ShelleyDelegationPart::Script(hash),
    };

    Ok(Address::Shelley(ShelleyAddress::new(network, payment, delegation)))
}

fn decode_compact_coin(decoder: &mut Decoder<'_>) -> anyhow::Result<u64> {
    let tag = decoder.tag()?;
    if tag != 0 {
        return Err(anyhow!("unsupported compact coin tag {tag}"));
    }
    decoder.varuint()
}

fn decode_compact_value(decoder: &mut Decoder<'_>) -> anyhow::Result<Value> {
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
    decoder.decode_from_short_bytes("inline datum", |bytes| {
        from_cbor(&bytes).ok_or_else(|| anyhow!("failed to decode PlutusData from CBOR"))
    })
}

fn decode_script(decoder: &mut Decoder<'_>) -> anyhow::Result<MemoizedScript> {
    match decoder.tag()? {
        0 => {
            let native = decoder.decode_from_short_bytes("native script", |bytes| {
                from_cbor(&bytes).ok_or_else(|| anyhow!("failed to decode Script from CBOR"))
            })?;
            Ok(MemoizedScript::NativeScript(native))
        }
        1 => {
            let tag = decoder.tag()?;
            let bytes = Bytes::from(decoder.short_bytes_vec()?);
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
            return Err(anyhow!("invalid asset-name offsets in multiasset representation"));
        }
        asset_lengths.insert(*asset_offset, next_offset - *asset_offset);
    }

    let mut bundles: BTreeMap<Hash<28>, Vec<(AssetName, PositiveCoin)>> = BTreeMap::new();
    for (policy_offset, asset_offset, quantity) in triples {
        let policy_end = policy_offset + 28;
        if policy_end > rep.len() {
            return Err(anyhow!("policy id offset is out of bounds"));
        }

        let asset_len = *asset_lengths.get(&asset_offset).ok_or_else(|| anyhow!("missing asset-name length"))?;
        let asset_end = asset_offset + asset_len;
        if asset_end > rep.len() {
            return Err(anyhow!("asset-name offset is out of bounds"));
        }

        let quantity: PositiveCoin =
            quantity.try_into().map_err(|_| anyhow!("invalid non-positive asset quantity {quantity}"))?;
        bundles.entry(Hash::from(&rep[policy_offset..policy_end])).or_default().push((
            AssetName::try_from(&rep[asset_offset..asset_end])
                .map_err(|_| anyhow!("invalid asset name for offset {asset_offset} and end {asset_end}"))?,
            quantity,
        ));
    }

    let mut policies = BTreeMap::new();
    for (policy_id, mut assets) in bundles {
        assets.sort_by_key(|(a, _)| *a);
        policies.insert(policy_id, NonEmptyKeyValuePairs::try_from(assets).map_err(|e| anyhow!("{e}"))?);
    }

    Ok(policies.into())
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{Address, MemoizedDatum, Value, cbor};

    use super::{Decoder, decode_transaction_output, decode_varuint64};

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
        assert_eq!(output.value, Value::Coin(42));
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
        assert_eq!(output.value, Value::Coin(99));
        assert!(matches!(output.datum, MemoizedDatum::Inline(_)));
        assert_eq!(cbor::to_vec(output).unwrap()[0], 0xbf);
    }

    #[test]
    fn normalizes_pointer_compact_addresses() {
        let compact_address =
            hex::decode("412813b99a80cfb3f1cf95653b169b17035963544837b7ce33d30710a8e710a09072c78ccf01").unwrap();
        let expected = Address::from_hex("412813b99a80cfb3f1cf95653b169b17035963544837b7ce33d30710a8000000").unwrap();

        let mut bytes = vec![0, compact_address.len() as u8];
        bytes.extend_from_slice(&compact_address);
        bytes.push(0);
        bytes.push(42);

        let output = decode_transaction_output(&bytes).unwrap();

        assert_eq!(output.address, expected);
        assert_eq!(output.address.to_vec(), expected.to_vec());
    }

    #[test]
    fn rejects_overlong_mempack_varuints() {
        let mut decoder = Decoder::new(&[0x80; 10]);

        assert_eq!(decoder.varuint().unwrap_err().to_string(), "mempack varuint exceeds 10 bytes");
    }

    #[test]
    fn rejects_overflowing_pointer_varuints() {
        let bytes = [0x82, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x80, 0x00];
        let mut offset = 0;

        assert_eq!(
            decode_varuint64(&bytes, &mut offset, "slot").unwrap_err().to_string(),
            "compact address slot overflows u64"
        );
    }
}
