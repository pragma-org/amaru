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

use crate::{
    AuxiliaryData, Hash, Hasher, Header, HeaderHash, Point, Transaction, TransactionBody, TransactionRef, WitnessSet,
    cbor, cbor::WithSize, size::BLOCK_BODY, traits::is_header::IsHeader,
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
    pub invalid_transactions: Option<BTreeSet<TransactionIndex>>,
}

/// Position of a transaction within a block.
/// There can only be a maximum of 65535 transactions in a block, so this is a `u16`.
pub type TransactionIndex = u16;

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
    pub fn hash_body_cbor(components: [&[u8]; 4]) -> Hash<BLOCK_BODY> {
        let mut concat = [0u8; 4 * BLOCK_BODY];
        for (i, component) in components.iter().enumerate() {
            let part = Hasher::<{ 8 * BLOCK_BODY }>::hash(component);
            concat[i * BLOCK_BODY..(i + 1) * BLOCK_BODY].copy_from_slice(part.as_ref());
        }
        Hasher::<{ 8 * BLOCK_BODY }>::hash(&concat)
    }

    /// Hash the body of an already-encoded block term `[header, bodies, witnesses, aux, invalid]`.
    pub fn hash_encoded_body(encoded_block: &[u8]) -> Result<Hash<BLOCK_BODY>, cbor::decode::Error> {
        let mut decoder = cbor::Decoder::new(encoded_block);
        let len = decoder.array()?;
        if len != Some(Self::CBOR_FIELD_COUNT) {
            return Err(cbor::decode::Error::message(format!(
                "invalid Block array length. Expected {}, got {len:?}",
                Self::CBOR_FIELD_COUNT
            )));
        }
        decoder.skip()?;
        let mut ranges = [(0usize, 0usize); 4];
        for range in &mut ranges {
            let start = decoder.position();
            decoder.skip()?;
            *range = (start, decoder.position());
        }
        if decoder.position() != encoded_block.len() {
            return Err(cbor::decode::Error::message("trailing data after block body"));
        }
        Ok(Self::hash_body_cbor([
            &encoded_block[ranges[0].0..ranges[0].1],
            &encoded_block[ranges[1].0..ranges[1].1],
            &encoded_block[ranges[2].0..ranges[2].1],
            &encoded_block[ranges[3].0..ranges[3].1],
        ]))
    }

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

    /// Compare two `Block`s by their CBOR-encoded forms.
    ///
    /// The derived `PartialEq` compares every field, including ones that are derived from the
    /// input bytes at decode time.
    ///
    /// Those field could legitimately differ after a re-encode -> re-decode round-trip.
    /// This method bypasses the issue by encoding both blocks and comparing the resulting bytes.
    /// (the `#[cbor(skip)]` fields are then excluded from the equality by construction)
    pub fn cbor_eq(&self, other: &Self) -> bool {
        amaru_minicbor_extra::to_cbor(self) == amaru_minicbor_extra::to_cbor(other)
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
                let is_expected_valid =
                    !self.invalid_transactions.as_ref().map(|set| set.contains(&i)).unwrap_or(false);

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
                let is_expected_valid =
                    !self.invalid_transactions.as_ref().map(|set| set.contains(&i)).unwrap_or(false);

                let auxiliary_data = self.auxiliary_data.get(&i);

                (i, TransactionRef { body, witnesses: witnesses.as_ref(), auxiliary_data, is_expected_valid })
            },
        ))
    }
}

// FIXME(cbor): Constraints & multi-era decoding
//
// There are various decoding rules that aren't enforced but that should be; for example (and
// non-exhaustively):
//
// - indices are constrained by the maximum number of elements in each arrays
// - there must be exactly the same number of witnesses and bodies
// - ...
//
// Also, we will likely require multi-era decoding too here. Even if we don't expect blocks from
// previous eras in normal operation (albeit, to be confirmed...), we will require to re-validate
// that a given chain is indeed at least well-formed, and that means drilling through headers to
// ensure they form a chain. So at least *some level* of multi-era decoding is necessary.
impl<'b, C: cbor::HasProtocolVersion> cbor::Decode<'b, C> for Block {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(Block::CBOR_FIELD_COUNT)?;

            let (header, header_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;

            let (transaction_bodies, transaction_bodies_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;

            let (transaction_witnesses, transaction_witnesses_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;

            let (auxiliary_data, auxiliary_data_bytes) =
                // FIXME(cbor): duplicate keys in aux data top-level map?
                //
                // We must double-check and confirm (i.e. have tests for) the behaviour of the
                // decoder regarding duplicate keys: if allowed, should they overwrite a previously
                // decoded value or give precedence to the first value decoded? If not allowed,
                // we should loudly fail.
                //
                // See #866.
                cbor::tee(d, |d| cbor::heterogeneous_map(d, BTreeMap::new(), |d| d.u16(), |d, st, field| {
                    st.insert(field, d.decode_with(ctx)?);
                    Ok(())
                }))?;

            let (invalid_transactions, invalid_transactions_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;

            Ok(Block {
                original_body_size: (transaction_bodies_bytes.len()
                    + transaction_witnesses_bytes.len()
                    + auxiliary_data_bytes.len()
                    + invalid_transactions_bytes.len()) as u64,
                original_header_size: header_bytes.len() as u64,
                hash: Self::hash_body_cbor([
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

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::*;
    use crate::{EraName, size::HEADER};

    macro_rules! fixture {
        ($id:expr) => {{
            (
                Hash::from(&hex::decode($id).unwrap()[..]),
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
