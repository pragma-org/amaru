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
    AuxiliaryData, Hash, Hasher, Header, HeaderHash, Transaction, TransactionBody, WitnessSet,
    cbor,
    size::{BLOCK_BODY, HEADER},
};
use std::collections::{BTreeMap, BTreeSet};

/// Hard-fork combinator discriminator.
///
/// See: <https://github.com/IntersectMBO/ouroboros-consensus/blob/7b150c85af56c8ded78151eaf5ec8675b7acbd8a/ouroboros-consensus-cardano/src/ouroboros-consensus-cardano/Ouroboros/Consensus/Cardano/Node.hs#L173-L179>
pub const ERA_VERSION_CONWAY: u16 = 7;

#[derive(Debug, Clone, PartialEq, cbor::Encode)]
pub struct Block {
    #[cbor(skip)]
    original_body_size: u64,

    #[cbor(skip)]
    original_header_size: u64,

    #[cbor(skip)]
    hash: Hash<BLOCK_BODY>,

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
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(5)?;

            let (header, header_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;

            let (transaction_bodies, transaction_bodies_bytes) =
                cbor::tee(d, |d| d.decode_with(ctx))?;

            let (transaction_witnesses, transaction_witnesses_bytes) =
                cbor::tee(d, |d| d.decode_with(ctx))?;

            let (auxiliary_data, auxiliary_data_bytes) = cbor::tee(d, |d| d.decode_with(ctx))?;

            let (invalid_transactions, invalid_transactions_bytes) =
                cbor::tee(d, |d| d.decode_with(ctx))?;

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
                header_hash: Hasher::<{ 8 * HEADER }>::hash(header_bytes),
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
    use super::*;
    use test_case::test_case;

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
    fn decode_wellformed(
        slot: u64,
        (id, result): (Hash<HEADER>, Result<(u16, Block), cbor::decode::Error>),
    ) {
        match result {
            Err(err) => panic!("{err}"),
            Ok((era_version, block)) => {
                assert_eq!(era_version, ERA_VERSION_CONWAY);

                assert_eq!(
                    hex::encode(&block.hash[..]),
                    hex::encode(&block.header.header_body.block_body_hash[..]),
                );

                assert_eq!(block.header_hash, id);
                assert_eq!(block.header.header_body.slot, slot);
            }
        }
    }
}
