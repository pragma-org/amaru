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

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    ops::Deref,
    sync::Arc,
};

use minicbor::decode;

use crate::{Block, cardano::network_block::NetworkBlock, cbor, utils::debug_bytes};

/// Cheaply cloneable block bytes
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct RawBlock(Arc<[u8]>);

impl fmt::Debug for RawBlock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let preview_hex = debug_bytes(&self.0, 32);
        let total_len = self.0.len();
        write!(f, "RawBlock({total_len}, {preview_hex})")
    }
}

impl Deref for RawBlock {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&[u8]> for RawBlock {
    fn from(bytes: &[u8]) -> Self {
        Self(Arc::from(bytes))
    }
}

impl From<Box<[u8]>> for RawBlock {
    fn from(bytes: Box<[u8]>) -> Self {
        Self(Arc::from(bytes))
    }
}

impl RawBlock {
    /// Decode the inner Block by first decoding the raw bytes as a NetworkBlock (which contains the
    /// era tag), then by decoding the Block.
    pub fn decode(&self) -> Result<Block, decode::Error> {
        let network_block: NetworkBlock = minicbor::decode(&self.0)?;
        network_block.decode_block()
    }

    /// Return an iterator over standalone CBOR-encoded transactions extracted from the block.
    pub fn transactions(&self) -> Result<RawBlockTransactions, decode::Error> {
        let network_block = NetworkBlock::try_from(self.clone())?;
        let mut decoder = cbor::Decoder::new(network_block.encoded_block());

        let len = decoder.array()?;
        if len != Some(Block::CBOR_FIELD_COUNT) {
            return Err(decode::Error::message(format!(
                "invalid Block array length. Expected {}, got {len:?}",
                Block::CBOR_FIELD_COUNT
            )));
        }

        decoder.skip()?;
        let bodies = cbor::collect_array_item_bytes(&mut decoder)?;
        let witnesses = cbor::collect_array_item_bytes(&mut decoder)?;
        let auxiliary_data = cbor::collect_map_value_bytes(&mut decoder, |d| d.u32())?;
        let invalid_transactions: Option<BTreeSet<u32>> = decoder.decode()?;

        if bodies.len() != witnesses.len() {
            return Err(decode::Error::message(format!(
                "inconsistent block: {} transaction bodies but {} witness sets",
                bodies.len(),
                witnesses.len()
            )));
        }

        Ok(RawBlockTransactions {
            bodies: bodies.into_iter(),
            witnesses: witnesses.into_iter(),
            auxiliary_data,
            invalid_transactions,
            index: 0,
        })
    }
}

/// This struct supports the iteration over serialized transactions contained in a block
pub struct RawBlockTransactions {
    bodies: std::vec::IntoIter<Vec<u8>>,
    witnesses: std::vec::IntoIter<Vec<u8>>,
    auxiliary_data: BTreeMap<u32, Vec<u8>>,
    invalid_transactions: Option<BTreeSet<u32>>,
    index: u32,
}

impl Iterator for RawBlockTransactions {
    type Item = Vec<u8>;

    fn next(&mut self) -> Option<Self::Item> {
        let body = self.bodies.next()?;
        let witnesses = self.witnesses.next()?;
        let tx_index = self.index;
        self.index += 1;

        Some(Self::serialize_transaction_from_components(
            &body,
            &witnesses,
            !self.invalid_transactions.as_ref().is_some_and(|set| set.contains(&tx_index)),
            self.auxiliary_data.get(&tx_index).map(Vec::as_slice),
        ))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.bodies.size_hint()
    }
}

impl ExactSizeIterator for RawBlockTransactions {
    fn len(&self) -> usize {
        self.bodies.len()
    }
}

impl RawBlockTransactions {
    /// Reconstruct a standalone CBOR transaction from the block's split transaction components.
    fn serialize_transaction_from_components(
        body: &[u8],
        witnesses: &[u8],
        is_expected_valid: bool,
        auxiliary_data: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut tx_bytes = Vec::with_capacity(body.len() + witnesses.len() + auxiliary_data.map_or(1, |x| x.len()) + 3);

        tx_bytes.push(0x84);
        tx_bytes.extend_from_slice(body);
        tx_bytes.extend_from_slice(witnesses);
        tx_bytes.push(if is_expected_valid { 0xf5 } else { 0xf4 });
        match auxiliary_data {
            Some(aux) => tx_bytes.extend_from_slice(aux),
            None => tx_bytes.push(0xf6),
        }

        tx_bytes
    }
}

#[cfg(test)]
mod tests {
    use amaru_minicbor_extra::{from_cbor, to_cbor};

    use crate::{
        Block, BlockHeader, TESTNET_ERA_HISTORY, Transaction,
        cardano::network_block::{NetworkBlock, make_block_with_header},
        include_cbor, make_header,
    };

    #[test]
    fn decode_returns_inner_block() {
        let header = BlockHeader::from(make_header(1, 42, None));
        let era_history = &*TESTNET_ERA_HISTORY;

        // make a network block from a block
        let block = make_block_with_header(&header);

        // first check the round-trip encoding / decoding for a block
        assert_eq!(block, from_cbor(to_cbor(&block).as_slice()).unwrap());

        // then check that the block can be retrieved from the network block
        let network_block = NetworkBlock::new(era_history, &block).expect("make network block");
        let decoded_block = network_block.decode_block().expect("network block should decode");
        assert_eq!(decoded_block, block);

        // finally check that the block can be retrieved from the raw block
        let raw_block = network_block.raw_block();
        let decoded_block = raw_block.decode().expect("raw block should decode");
        assert_eq!(decoded_block, block);
    }

    #[test]
    fn iterate_over_serialized_transactions() {
        let (_era, block): (crate::EraName, Block) = include_cbor!(
            "cbor.decode/block/b9bef52dd8dedf992837d20c18399a284d80fde0ae9435f2a33649aaee7c5698/sample.cbor"
        );
        let raw_block = NetworkBlock::new(&crate::PREPROD_ERA_HISTORY, &block).expect("make network block").raw_block();
        let mut txs = raw_block.transactions().expect("extract transactions");
        assert_eq!(txs.len(), 1);

        let tx_bytes = txs.next().expect("the first transaction");
        let tx: Transaction = minicbor::decode(&tx_bytes).expect("decode extracted transaction");
        assert!(!tx_bytes.is_empty());
        assert_eq!(tx.body.id().to_string(), "3741dc1d8f14f938904388bb257a05b361ac1e9f447db11032bb2577ff0cbd38");
    }
}
