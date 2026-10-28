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

use std::convert::Infallible;

use amaru_kernel::{
    Block, ProtocolVersion, TransactionIndex,
    cbor::{self, data::Type},
};
use amaru_minicbor_extra::{from_cbor_no_leftovers, from_cbor_no_leftovers_with, to_cbor, to_cbor_with};

use crate::AUXILIARY_DATA_IN_BLOCK;

/// Decode a sample and re-encode it, yielding the re-encoded bytes.
pub type RoundTrip = fn(&[u8], ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>;

pub fn round_trip<T>(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>
where
    T: for<'b> cbor::Decode<'b, ProtocolVersion> + cbor::Encode<ProtocolVersion>,
{
    let mut version = version;
    let value: T = from_cbor_no_leftovers_with(bytes, &mut version)?;
    Ok(to_cbor_with(&value, &mut version))
}

/// Decode a sample and re-encode it, for a type whose codec takes no context.
pub fn round_trip_without_context<T>(bytes: &[u8], _version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error>
where
    T: for<'b> cbor::Decode<'b, ()> + cbor::Encode<()>,
{
    let value: T = from_cbor_no_leftovers(bytes)?;
    Ok(to_cbor(&value))
}

/// Decode a block and re-encode it, writing its auxiliary data entries in the order of the sample.
///
/// The CDDL sets no order for the transaction indexes of the auxiliary data map. amaru holds that map in a `BTreeMap`
/// and writes it in key order, while the Haskell node replays the original bytes of the block body. Only the order
/// of the entries comes from the sample: every key and value is written from the decoded block.
pub fn round_trip_block(bytes: &[u8], version: ProtocolVersion) -> Result<Vec<u8>, cbor::decode::Error> {
    let mut version = version;
    let block: Block = from_cbor_no_leftovers_with(bytes, &mut version)?;
    let order = auxiliary_data_order(bytes)?;
    encode_block(&block, &order, &mut version)
        .map_err(|e| cbor::decode::Error::message(format!("re-encoding the block: {e}")))
}

fn encode_block(
    block: &Block,
    auxiliary_data_order: &[TransactionIndex],
    version: &mut ProtocolVersion,
) -> Result<Vec<u8>, cbor::encode::Error<Infallible>> {
    let mut buffer = Vec::new();
    let mut e = cbor::Encoder::new(&mut buffer);
    e.array(Block::CBOR_FIELD_COUNT)?;
    e.encode_with(&block.header, version)?;
    e.encode_with(&block.transaction_bodies, version)?;
    e.encode_with(&block.transaction_witnesses, version)?;
    e.map(auxiliary_data_order.len() as u64)?;
    for index in auxiliary_data_order {
        e.u16(*index)?;
        e.encode_with(&block.auxiliary_data[index], version)?;
    }
    e.encode_with(&block.invalid_transactions, version)?;
    Ok(buffer)
}

/// The transaction indexes of the auxiliary data map of a block, in the order the bytes give them.
fn auxiliary_data_order(bytes: &[u8]) -> Result<Vec<TransactionIndex>, cbor::decode::Error> {
    let mut d = cbor::Decoder::new(bytes);
    d.array()?;
    for _ in 0..AUXILIARY_DATA_IN_BLOCK {
        d.skip()?;
    }

    let len = d.map()?;
    let mut indexes = Vec::new();
    loop {
        let done = match len {
            Some(len) => indexes.len() as u64 == len,
            None => d.datatype()? == Type::Break,
        };
        if done {
            return Ok(indexes);
        }
        indexes.push(d.u16()?);
        d.skip()?;
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::auxiliary_data_order;

    #[test_case("85008080a201a000a080", vec![1, 0]; "definite map")]
    #[test_case("85008080bf01a000a0ff80", vec![1, 0]; "indefinite map")]
    #[test_case("9f008080a0ff", vec![]; "indefinite block and empty map")]
    fn reads_the_auxiliary_data_order_of_a_block(input: &str, expected: Vec<u16>) {
        let actual = auxiliary_data_order(&hex::decode(input).unwrap()).unwrap();

        assert_eq!(actual, expected);
    }
}
