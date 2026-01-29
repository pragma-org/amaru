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

use crate::{
    AuxiliaryData, Hasher, Header, HeaderHash, Transaction, TransactionBody, WitnessSet, cbor,
};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, PartialEq, cbor::Encode)]
pub struct Block {
    #[cbor(skip)]
    original_body_size: u64,

    #[cbor(skip)]
    original_header_size: u64,

    #[cbor(skip)]
    header_hash: HeaderHash,

    #[n(0)]
    pub header: Header,

    #[b(1)]
    pub transaction_bodies: Vec<TransactionBody>,

    #[n(2)]
    pub transaction_witnesses: Vec<WitnessSet>,

    #[n(3)]
    pub auxiliary_data: BTreeMap<u32, AuxiliaryData>,

    #[n(4)]
    pub invalid_transactions: Option<BTreeSet<u32>>,
}

impl Block {
    /// Get the size in bytes of the serialised block.
    pub fn body_len(&self) -> u64 {
        self.original_body_size
    }

    /// Get the size in bytes of the serialised block's header
    pub fn header_len(&self) -> u64 {
        self.original_header_size
    }

    pub fn header_hash(&self) -> HeaderHash {
        self.header_hash
    }
}

impl IntoIterator for Block {
    type Item = (u32, Transaction);
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(mut self) -> Self::IntoIter {
        (0u32..)
            .zip(self.transaction_bodies)
            .zip(self.transaction_witnesses)
            .map(|((i, body), witnesses)| {
                let is_expected_valid = !self
                    .invalid_transactions
                    .as_ref()
                    .map(|set| set.contains(&i))
                    .unwrap_or(false);

                // TODO: Do not re-hash here, but get the hash while parsing.
                let auxiliary_data: Option<AuxiliaryData> = self.auxiliary_data.remove(&i);

                (
                    i,
                    Transaction {
                        body,
                        witnesses,
                        is_expected_valid,
                        auxiliary_data,
                    },
                )
            })
            .collect::<Vec<_>>()
            .into_iter()
    }
}

// FIXME: Constraints & multi-era decoding
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
        let original_bytes = d.input();
        let start_position = d.position();

        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(5)?;

            let header = d.decode_with(ctx)?;
            let end_position_header = d.position();

            let transaction_bodies = d.decode_with(ctx)?;
            let transaction_witnesses = d.decode_with(ctx)?;
            let auxiliary_data = d.decode_with(ctx)?;
            let invalid_transactions = d.decode_with(ctx)?;

            let end_position = d.position();

            Ok(Block {
                original_body_size: (end_position - end_position_header) as u64, // from usize
                original_header_size: (end_position_header - start_position) as u64, // from usize
                header_hash: Hasher::<256>::hash(
                    &original_bytes[start_position..end_position_header],
                ),
                header,
                transaction_bodies,
                transaction_witnesses,
                auxiliary_data,
                invalid_transactions,
            })
        })
    }
}
