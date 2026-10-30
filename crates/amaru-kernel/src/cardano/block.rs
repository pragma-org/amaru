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

use std::collections::{BTreeMap, BTreeSet};

use super::block_body_parts::hash_body_cbor;
use crate::{
    AuxiliaryData, Hash, Header, HeaderHash, Point, ProtocolVersion, Transaction, TransactionBody, TransactionIndex,
    TransactionRef, WitnessSet, cbor, cbor::WithSize, protocol_version::PROTOCOL_VERSION_12, size::BLOCK_BODY,
    traits::is_header::IsHeader,
};

#[derive(Debug, Clone, PartialEq, cbor::Encode)]
#[cbor(context_bound = "crate::cbor::HasProtocolVersion")]
pub struct Block {
    #[cbor(skip)]
    original_body_size: u64,

    #[cbor(skip)]
    original_header_size: u64,

    #[cbor(skip)]
    hash: Hash<BLOCK_BODY>,

    #[n(0)]
    pub header: Header,

    #[b(1)]
    pub transaction_bodies: Vec<TransactionBody>,

    #[n(2)]
    pub transaction_witnesses: Vec<WithSize<WitnessSet>>,

    #[n(3)]
    pub auxiliary_data: BTreeMap<TransactionIndex, AuxiliaryData>,

    #[n(4)]
    pub invalid_transactions: BTreeSet<TransactionIndex>,
}

impl Block {
    /// Number of top-level CBOR fields in a serialized block.
    pub const CBOR_FIELD_COUNT: u64 = 5;

    /// Get the hash of the block's body
    pub fn body_hash(&self) -> Hash<BLOCK_BODY> {
        self.hash
    }

    /// Hash of the four body CBOR items, as stored in [`HeaderBody::block_body_hash`](crate::HeaderBody).
    ///
    /// The array must contain the slices of the four CBOR items in the order they appear in the
    /// block body: `[bodies, witnesses, aux, invalid]`.
    /// Get the size in bytes of the serialised block.
    pub fn body_len(&self) -> u64 {
        self.original_body_size
    }

    /// Get the size in bytes of the serialised block's header
    pub fn header_len(&self) -> u64 {
        self.original_header_size
    }

    pub fn header_hash(&self) -> HeaderHash {
        self.header.hash()
    }

    pub fn point(&self) -> Point {
        self.header.point()
    }
}

impl IntoIterator for Block {
    type Item = (TransactionIndex, Transaction);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(mut self) -> Self::IntoIter {
        (0..)
            .zip(self.transaction_bodies)
            .zip(self.transaction_witnesses)
            .map(|((i, body), witnesses)| {
                let is_expected_valid = !self.invalid_transactions.contains(&i);

                let auxiliary_data = self.auxiliary_data.remove(&i);

                (i, Transaction { body, witnesses, auxiliary_data, is_expected_valid })
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

impl<'a> IntoIterator for &'a Block {
    type Item = (u16, TransactionRef<'a>);
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new((0u16..).zip(self.transaction_bodies.iter()).zip(&self.transaction_witnesses).map(
            |((i, body), witnesses)| {
                let is_expected_valid = !self.invalid_transactions.contains(&i);

                let auxiliary_data = self.auxiliary_data.get(&i);

                (i, TransactionRef { body, witnesses: witnesses.as_ref(), auxiliary_data, is_expected_valid })
            },
        ))
    }
}

// FIXME(cbor): Multi-era decoding
//
// We will likely require multi-era decoding too here. Even if we don't expect blocks from
// previous eras in normal operation (albeit, to be confirmed...), we will require to re-validate
// that a given chain is indeed at least well-formed, and that means drilling through headers to
// ensure they form a chain. So at least *some level* of multi-era decoding is necessary.
impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Block {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(Block::CBOR_FIELD_COUNT)?;

            let (header, header_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;

            let (transaction_bodies, transaction_bodies_bytes) = cbor::tee(d, |d| d.decode_with::<_, Vec<_>>(ctx))?;

            let (transaction_witnesses, transaction_witnesses_bytes) =
                cbor::tee(d, |d| d.decode_with::<_, Vec<_>>(ctx))?;

            let transaction_count = transaction_bodies.len();
            if transaction_count != transaction_witnesses.len() {
                return Err(cbor::decode::Error::message(format!(
                    "inconsistent block: {transaction_count} transaction bodies but {} witness sets",
                    transaction_witnesses.len()
                )));
            }

            let (auxiliary_data, auxiliary_data_bytes) = cbor::tee(d, |d| {
                cbor::heterogeneous_map(
                    d,
                    BTreeMap::new(),
                    |d| d.u16(),
                    |d, st, field| {
                        if st.contains_key(&field) {
                            return Err(cbor::decode::Error::message(format!(
                                "duplicate auxiliary data entry with transaction index {field}"
                            )));
                        }
                        if usize::from(field) >= transaction_count {
                            return Err(cbor::decode::Error::message(format!(
                                "auxiliary data index {field} out of bounds for {transaction_count} transaction bodies"
                            )));
                        }
                        st.insert(field, d.decode_with(ctx)?);
                        Ok(())
                    },
                )
            })?;

            let (invalid_transactions, invalid_transactions_bytes) =
                cbor::tee(d, |d| decode_invalid_transactions(d, transaction_count, ctx.protocol_version()))?;

            Ok(Block {
                original_body_size: (transaction_bodies_bytes.len()
                    + transaction_witnesses_bytes.len()
                    + auxiliary_data_bytes.len()
                    + invalid_transactions_bytes.len()) as u64,
                original_header_size: header_bytes.len() as u64,
                hash: hash_body_cbor([
                    transaction_bodies_bytes,
                    transaction_witnesses_bytes,
                    auxiliary_data_bytes,
                    invalid_transactions_bytes,
                ]),
                header,
                transaction_bodies,
                transaction_witnesses,
                auxiliary_data,
                invalid_transactions,
            })
        })
    }
}

/// Decode the indexes of the transactions that failed phase-2 validation.
///
/// Up to protocol version 11, the Haskell node turns this list into one validity flag per transaction
/// (`alignedValidFlags`), which works only for indexes in strictly increasing order. A repeated or an unsorted index
/// makes it raise an exception, so the node refuses the block. That exception belongs to the Conway block body: from
/// protocol version 12 the indexes are read as a set.
fn decode_invalid_transactions(
    d: &mut cbor::Decoder<'_>,
    transaction_count: usize,
    protocol_version: ProtocolVersion,
) -> Result<BTreeSet<TransactionIndex>, cbor::decode::Error> {
    let strictly_increasing = protocol_version < PROTOCOL_VERSION_12;
    let mut indexes = BTreeSet::new();
    for index in d.array_iter::<TransactionIndex>()? {
        let index = index?;
        if usize::from(index) >= transaction_count {
            return Err(cbor::decode::Error::message(format!(
                "invalid transaction index {index} out of bounds for {transaction_count} transaction bodies"
            )));
        }
        if strictly_increasing
            && let Some(previous) = indexes.last()
            && *previous >= index
        {
            return Err(cbor::decode::Error::message(format!(
                "invalid transaction indexes must be strictly increasing: {index} comes after {previous}"
            )));
        }
        indexes.insert(index);
    }
    Ok(indexes)
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::{EraName, protocol_version::PROTOCOL_VERSION_11, size::HEADER};

    #[test_case(PROTOCOL_VERSION_11, "820001" => Some(vec![0, 1]) ; "sorted indexes")]
    #[test_case(PROTOCOL_VERSION_11, "9f0001ff" => Some(vec![0, 1]) ; "indefinite sorted indexes")]
    #[test_case(PROTOCOL_VERSION_11, "820000" => None ; "repeated index before v12")]
    #[test_case(PROTOCOL_VERSION_11, "820100" => None ; "unsorted indexes before v12")]
    #[test_case(PROTOCOL_VERSION_11, "8102" => None ; "index out of bounds before v12")]
    #[test_case(PROTOCOL_VERSION_12, "820000" => Some(vec![0]) ; "repeated index from v12")]
    #[test_case(PROTOCOL_VERSION_12, "820100" => Some(vec![0, 1]) ; "unsorted indexes from v12")]
    #[test_case(PROTOCOL_VERSION_12, "8102" => None ; "index out of bounds from v12")]
    fn invalid_transactions_for_two_transactions(
        protocol_version: ProtocolVersion,
        hex_bytes: &str,
    ) -> Option<Vec<TransactionIndex>> {
        let bytes = hex::decode(hex_bytes).unwrap();
        decode_invalid_transactions(&mut cbor::Decoder::new(&bytes), 2, protocol_version)
            .ok()
            .map(|indexes| indexes.into_iter().collect())
    }

    macro_rules! fixture {
        ($id:expr) => {{
            (
                Hash::try_from(&hex::decode($id).unwrap()[..]).unwrap(),
                $crate::try_include_cbor!(concat!("cbor.decode/block/", $id, "/sample.cbor")),
            )
        }};
    }

    #[test_case(
        70175999,
        fixture!("b9bef52dd8dedf992837d20c18399a284d80fde0ae9435f2a33649aaee7c5698")
    )]
    #[test_case(
        70206662,
        fixture!("b99a61170fcdb5bade252be2cb0fa6e3ac550b9f5cc4e9d001eda88291eb9de7")
    )]
    #[test_case(
        70225763,
        fixture!("313e774e32c23b3691751e62d6b57181538cf3164b242505919bce29226de19f")
    )]
    #[test_case(
        70582226,
        fixture!("e1b90d83d6ae89860e2d1a0f398355cd4ed6defddb028dd610748d1f5610b546")
    )]
    #[test_case(
        71419349,
        fixture!("0df40008e40348c40cdc3b92a1e31d0e55675ddf2bb05ff7683b38a837048bca")
    )]
    fn decode_wellformed(slot: u64, (id, result): (Hash<HEADER>, Result<(EraName, Block), cbor::decode::Error>)) {
        match result {
            Err(err) => panic!("{err}"),
            Ok((era_version, block)) => {
                assert_eq!(era_version, EraName::Conway);

                assert_eq!(hex::encode(&block.hash[..]), hex::encode(&block.header.body().block_body_hash[..]),);

                assert_eq!(block.header.hash(), id);
                assert_eq!(block.header.slot().as_u64(), slot);
            }
        }
    }
}
