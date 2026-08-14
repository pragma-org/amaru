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
    type Item = (TransactionIndex, Transaction, u64);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(mut self) -> Self::IntoIter {
        (0..)
            .zip(self.transaction_bodies)
            .zip(self.transaction_witnesses)
            .map(|((i, body), witnesses)| {
                let is_expected_valid =
                    !self.invalid_transactions.as_ref().map(|set| set.contains(&i)).unwrap_or(false);

                let (auxiliary_data_len, auxiliary_data) = match self.auxiliary_data.remove(&i) {
                    Some(auxiliary_data) => (auxiliary_data.len(), Some(auxiliary_data)),
                    None => (1, None),
                };

                // NOTE: Transaction size calculation
                //
                // Due to how the transactions are serialised in blocks (with seggregated witnesses
                // and auxiliary data), we have to calculate the size from multiple pieces and add
                // an extra 'cbor framing byte' which corresponds to the declaration of the
                // top-level array of size 3 (`0x83`). Importantly, the validity of the transaction
                // is not taken into account for the size calculation (rationale being that this
                // the logic is then preserved between pre-alonzo and post-alonzo eras).
                //
                // See also: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/Tx.hs#L351-L362>
                let size = 1 + body.len() + witnesses.len() as u64 + auxiliary_data_len;

                (i, Transaction { body, witnesses: witnesses.into_inner(), auxiliary_data, is_expected_valid }, size)
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

impl<'a> IntoIterator for &'a Block {
    type Item = (u16, TransactionRef<'a>, u64);
    type IntoIter = Box<dyn Iterator<Item = Self::Item> + 'a>;

    fn into_iter(self) -> Self::IntoIter {
        Box::new((0u16..).zip(self.transaction_bodies.iter()).zip(&self.transaction_witnesses).map(
            |((i, body), witnesses)| {
                let is_expected_valid =
                    !self.invalid_transactions.as_ref().map(|set| set.contains(&i)).unwrap_or(false);

                let (auxiliary_data_len, auxiliary_data) = match self.auxiliary_data.get(&i) {
                    Some(auxiliary_data) => (auxiliary_data.len(), Some(auxiliary_data)),
                    None => (1, None),
                };

                // NOTE: Transaction size calculation
                //
                // Due to how the transactions are serialised in blocks (with seggregated witnesses
                // and auxiliary data), we have to calculate the size from multiple pieces and add
                // an extra 'cbor framing byte' which corresponds to the declaration of the
                // top-level array of size 3 (`0x83`). Importantly, the validity of the transaction
                // is not taken into account for the size calculation (rationale being that this
                // the logic is then preserved between pre-alonzo and post-alonzo eras).
                //
                // See also: <https://github.com/IntersectMBO/cardano-ledger/blob/0cfbf861cfb456660a7b73281c6fb714a53d40f9/eras/alonzo/impl/src/Cardano/Ledger/Alonzo/Tx.hs#L351-L362>
                let size = 1 + body.len() + witnesses.len() as u64 + auxiliary_data_len;

                (i, TransactionRef { body, witnesses: witnesses.as_ref(), auxiliary_data, is_expected_valid }, size)
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
impl<'b, C> cbor::Decode<'b, C> for Block {
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

            let mut block_body_hash = Vec::with_capacity(4 * BLOCK_BODY);
            for component in [
                transaction_bodies_bytes,
                transaction_witnesses_bytes,
                auxiliary_data_bytes,
                invalid_transactions_bytes,
            ] {
                let body_part = Hasher::<{ 8 * BLOCK_BODY }>::hash(component);
                block_body_hash.extend_from_slice(&body_part[..]);
            }

            Ok(Block {
                original_body_size: (transaction_bodies_bytes.len()
                    + transaction_witnesses_bytes.len()
                    + auxiliary_data_bytes.len()
                    + invalid_transactions_bytes.len()) as u64,
                original_header_size: header_bytes.len() as u64,
                hash: Hasher::<{ 8 * BLOCK_BODY }>::hash(&block_body_hash[..]),
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
