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

use crate::{
    Block, BlockHeight, EraName, Hasher, HeaderBody, HeaderHash, Point,
    cardano::{block::TransactionIndex, header::KES_SIGNATURE, network_block::NetworkBlock},
    cbor,
    utils::debug_bytes,
};

/// A block header borrowed directly from a raw, network-encoded block.
///
/// The outer block variant differs from the ChainSync header variant: Byron uses
/// two block variants, while later eras are offset by one.
#[derive(Debug, Clone, Copy, Eq)]
pub struct ParsedBlockHeader<'a> {
    block_variant: u8,
    era: EraName,
    cbor: &'a [u8],
    metadata: Option<HeaderMetadata>,
}

impl PartialEq for ParsedBlockHeader<'_> {
    fn eq(&self, other: &Self) -> bool {
        self.block_variant == other.block_variant && self.cbor == other.cbor
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HeaderMetadata {
    epoch: Option<u64>,
    slot: u64,
    height: BlockHeight,
    parent_hash: Option<HeaderHash>,
}

impl<'a> ParsedBlockHeader<'a> {
    /// Associate header CBOR with its network block variant, rejecting unknown variants.
    #[inline]
    pub fn from_cbor(block_variant: u8, cbor: &'a [u8]) -> Result<Self, cbor::decode::Error> {
        let era = match block_variant {
            0..=1 => EraName::Byron,
            variant @ 2..=7 => EraName::from_header_variant(variant - 1).map_err(cbor::decode::Error::custom)?,
            _ => return Err(cbor::decode::Error::message("unsupported historical block variant")),
        };
        Ok(Self { block_variant, era, cbor, metadata: None })
    }

    /// The outer block variant from the network block encoding.
    pub fn block_variant(&self) -> u8 {
        self.block_variant
    }

    /// The Cardano era represented by the outer block variant.
    pub fn era(&self) -> EraName {
        self.era
    }

    /// The original CBOR bytes of the nested header.
    pub fn cbor(&self) -> &'a [u8] {
        self.cbor
    }

    /// Whether this is a Byron epoch-boundary block.
    pub fn is_epoch_boundary(&self) -> bool {
        self.block_variant == 0
    }

    /// Calculate the header hash using the era-specific wire representation.
    pub fn hash(&self) -> HeaderHash {
        let mut hasher = Hasher::<256>::new();
        if self.era == EraName::Byron {
            hasher.input(&[0x82, self.block_variant]);
        }
        hasher.input(self.cbor);
        hasher.finalize()
    }

    /// Decode the chain point represented by the header, reusing metadata from complete block decoding.
    ///
    /// `byron_slots_per_epoch` is required because Byron headers contain an
    /// epoch-relative slot rather than the absolute slot used by [`Point`].
    pub fn point(&self, byron_slots_per_epoch: u64) -> Result<Point, cbor::decode::Error> {
        let metadata = match self.metadata {
            Some(metadata) => metadata,
            None => decode_header_metadata(&mut cbor::Decoder::new(self.cbor), self.block_variant)?,
        };
        let slot = if let Some(epoch) = metadata.epoch {
            if byron_slots_per_epoch == 0 {
                return Err(cbor::decode::Error::message("Byron epoch length must be nonzero"));
            }
            if metadata.slot >= byron_slots_per_epoch {
                return Err(cbor::decode::Error::message("Byron relative slot exceeds the epoch length"));
            }
            epoch
                .checked_mul(byron_slots_per_epoch)
                .and_then(|start| start.checked_add(metadata.slot))
                .ok_or_else(|| cbor::decode::Error::message("Byron slot overflow"))?
        } else {
            metadata.slot
        };
        Ok(Point::Specific(slot.into(), self.hash(), metadata.height))
    }
}

/// A borrowed network block decoded for chain inspection from Byron through Conway.
///
/// Decoding checks the era-specific block and header layout and reads the parent hash. The header
/// provides the slot, height and hash, including Byron epoch-boundary blocks. Transaction bodies,
/// witnesses and proofs retain their original CBOR; this does not validate ledger rules, signatures
/// or body commitments. The first block in a chain must be anchored separately by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MultiEraBlock<'a> {
    header: ParsedBlockHeader<'a>,
}

impl<'a> MultiEraBlock<'a> {
    /// Decode exactly one network block, rejecting missing fields and trailing bytes.
    pub fn decode(input: &'a [u8]) -> Result<Self, cbor::decode::Error> {
        let mut decoder = cbor::Decoder::new(input);
        let block = cbor::heterogeneous_array(&mut decoder, |d, assert_len| {
            assert_len(2)?;
            let variant = d.u8()?;
            Ok(Self { header: decode_multi_era_block(d, variant)? })
        })?;
        if decoder.position() != input.len() {
            return Err(cbor::decode::Error::message("trailing bytes after network block"));
        }
        Ok(block)
    }

    /// Header metadata and the original bytes used to compute its hash.
    pub fn header(&self) -> ParsedBlockHeader<'a> {
        self.header
    }

    /// The hash claimed as the predecessor, or `None` for a Shelley-based genesis header.
    /// Byron headers always carry a hash, including their network's genesis hash.
    pub fn parent_hash(&self) -> Option<HeaderHash> {
        self.header.metadata.and_then(|metadata| metadata.parent_hash)
    }
}

fn decode_multi_era_block<'a>(
    d: &mut cbor::Decoder<'a>,
    variant: u8,
) -> Result<ParsedBlockHeader<'a>, cbor::decode::Error> {
    cbor::heterogeneous_array(d, |d, assert_len| {
        let (metadata, bytes) = cbor::tee(d, |d| decode_header_metadata(d, variant))?;
        let mut header = ParsedBlockHeader::from_cbor(variant, bytes)?;
        header.metadata = Some(metadata);
        match variant {
            0..=1 => {
                assert_len(3)?;
                if header.is_epoch_boundary() {
                    cbor::skip_array(d)?;
                } else {
                    cbor::heterogeneous_array(d, |d, assert_len| {
                        assert_len(4)?;
                        for _ in 0..4 {
                            cbor::skip_array(d)?;
                        }
                        Ok(())
                    })?;
                }
                cbor::heterogeneous_array(d, |d, assert_len| {
                    assert_len(1)?;
                    cbor::skip_map(d)
                })?;
            }
            2..=7 => {
                let has_invalid_transactions = variant >= 5;
                assert_len(if has_invalid_transactions { 5 } else { 4 })?;
                cbor::skip_array(d)?;
                cbor::skip_array(d)?;
                cbor::skip_map(d)?;
                if has_invalid_transactions {
                    cbor::skip_array(d)?;
                }
            }
            _ => return Err(cbor::decode::Error::message("unsupported historical block variant")),
        }
        Ok(header)
    })
}

fn decode_header_metadata(d: &mut cbor::Decoder<'_>, block_variant: u8) -> Result<HeaderMetadata, cbor::decode::Error> {
    if block_variant <= 1 {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(5)?;
            d.u32()?;
            let parent_hash = Some(decode_hash(d)?);
            cbor::skip_value(d)?;
            let (epoch, slot, height) = cbor::heterogeneous_array(d, |d, assert_len| {
                if block_variant == 0 {
                    assert_len(2)?;
                    Ok((d.u64()?, 0, decode_difficulty(d)?))
                } else {
                    assert_len(4)?;
                    let (epoch, slot) = cbor::heterogeneous_array(d, |d, assert_len| {
                        assert_len(2)?;
                        Ok((d.u64()?, d.u64()?))
                    })?;
                    cbor::decode_bytes(d)?;
                    let height = decode_difficulty(d)?;
                    cbor::skip_array(d)?;
                    Ok((epoch, slot, height))
                }
            })?;
            cbor::skip_array(d)?;
            Ok(HeaderMetadata { epoch: Some(epoch), slot, height, parent_hash })
        })
    } else {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(2)?;
            let metadata = if matches!(block_variant, 6..=7) {
                let body: HeaderBody = d.decode()?;
                HeaderMetadata {
                    epoch: None,
                    slot: body.slot,
                    height: body.block_number.into(),
                    parent_hash: body.prev_hash,
                }
            } else {
                cbor::heterogeneous_array(d, |d, assert_len| {
                    let fields = match block_variant {
                        2..=5 => 15,
                        _ => return Err(cbor::decode::Error::message("unknown block variant")),
                    };
                    assert_len(fields)?;
                    let height = d.decode()?;
                    let slot = d.u64()?;
                    let parent_hash = if d.datatype()? == cbor::data::Type::Null {
                        d.null()?;
                        None
                    } else {
                        Some(decode_hash(d)?)
                    };
                    for _ in 3..fields {
                        cbor::skip_value(d)?;
                    }
                    Ok(HeaderMetadata { epoch: None, slot, height, parent_hash })
                })?
            };
            if cbor::decode_bytes(d)?.len() != KES_SIGNATURE {
                return Err(cbor::decode::Error::message("invalid KES signature length"));
            }
            Ok(metadata)
        })
    }
}

fn decode_hash(d: &mut cbor::Decoder<'_>) -> Result<HeaderHash, cbor::decode::Error> {
    HeaderHash::try_from(cbor::decode_bytes(d)?.as_ref()).map_err(cbor::decode::Error::custom)
}

fn decode_difficulty(d: &mut cbor::Decoder<'_>) -> Result<BlockHeight, cbor::decode::Error> {
    cbor::heterogeneous_array(d, |d, assert_len| {
        assert_len(1)?;
        d.decode()
    })
}

/// Cheaply cloneable block bytes
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RawBlock(Arc<[u8]>);

impl serde::Serialize for RawBlock {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        crate::utils::serde::bytes::serialize(&self.0, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for RawBlock {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        crate::utils::serde::bytes::deserialize_arc(deserializer).map(Self)
    }
}

impl schemars::JsonSchema for RawBlock {
    fn schema_name() -> String {
        "RawBlock".to_string()
    }

    fn json_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        crate::utils::serde::bytes::json_schema("hex-encoded block bytes")
    }

    fn is_referenceable() -> bool {
        false
    }
}

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
    /// Decode the block structure and chain metadata for any supported era.
    ///
    /// Transaction bodies remain borrowed CBOR. Use this when checking historical parent links;
    /// [`Self::decode`] decodes transactions for ledger processing.
    pub fn decode_multi_era(&self) -> Result<MultiEraBlock<'_>, decode::Error> {
        MultiEraBlock::decode(&self.0)
    }

    /// Decode the inner Block by first decoding the raw bytes as a NetworkBlock (which contains the
    /// era tag), then by decoding the Block.
    pub fn decode(&self) -> Result<Block, decode::Error> {
        let network_block: NetworkBlock = minicbor::decode(&self.0)?;
        network_block.decode_block()
    }

    /// Decode only the header from the stored network-block bytes.
    pub fn decode_header(&self) -> Result<crate::Header, decode::Error> {
        let network_block: NetworkBlock = minicbor::decode(&self.0)?;
        network_block.decode_header()
    }

    /// Hash of the four body CBOR items, matching [`Block::body_hash`].
    pub fn body_hash(&self) -> Result<crate::Hash<{ crate::size::BLOCK_BODY }>, decode::Error> {
        let network_block: NetworkBlock = minicbor::decode(&self.0)?;
        Block::hash_encoded_body(network_block.encoded_block())
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
        let auxiliary_data = cbor::collect_map_value_bytes(&mut decoder, |d| d.u16())?;
        let invalid_transactions: Option<BTreeSet<TransactionIndex>> = decoder.decode()?;

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

/// Extract the CBOR-encoded block header bytes from a raw network block.
///
/// Only the network wrapper and header array are inspected; the block body is not read.
#[inline]
pub fn extract_block_header_cbor(input: &[u8]) -> Result<&[u8], cbor::decode::Error> {
    Ok(parse_block_header(input)?.cbor())
}

/// Parse the era tag and nested header without reading the block body or decoding chain metadata.
#[inline(always)]
pub fn parse_block_header(input: &[u8]) -> Result<ParsedBlockHeader<'_>, cbor::decode::Error> {
    let mut decoder = cbor::Decoder::new(input);
    if decoder.array()?.is_some_and(|length| length != 2) {
        return Err(cbor::decode::Error::message("expected network block era and payload"));
    }
    let block_variant = decoder.u8()?;
    if decoder.array()? == Some(0) {
        return Err(cbor::decode::Error::message("missing block header"));
    }
    let (_, bytes) = cbor::tee(&mut decoder, cbor::skip_array)?;
    ParsedBlockHeader::from_cbor(block_variant, bytes)
}

/// This struct supports the iteration over serialized transactions contained in a block
pub struct RawBlockTransactions {
    bodies: std::vec::IntoIter<Vec<u8>>,
    witnesses: std::vec::IntoIter<Vec<u8>>,
    auxiliary_data: BTreeMap<TransactionIndex, Vec<u8>>,
    invalid_transactions: Option<BTreeSet<TransactionIndex>>,
    index: TransactionIndex,
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
    use super::{extract_block_header_cbor, parse_block_header};
    use crate::{
        Block, PREPROD_ERA_HISTORY, Transaction,
        cardano::network_block::{NetworkBlock, make_block_with_header},
        from_cbor, include_cbor, make_header, to_cbor,
    };

    #[test]
    fn extracts_header_bytes_without_decoding_chain_metadata() {
        let bytes = [0x82, 2, 0x81, 0x81, 0x82, 9, 42];
        let header = parse_block_header(&bytes).unwrap();
        assert_eq!(header.cbor(), &bytes[3..]);
        assert_eq!(extract_block_header_cbor(&bytes).unwrap(), header.cbor());
        assert!(header.point(0).is_err());
    }

    #[test]
    fn rejects_unknown_block_variant() {
        assert!(parse_block_header(&[0x82, 9, 0x81, 0x80]).is_err());
    }

    #[test]
    fn rejects_missing_headers_and_invalid_network_wrappers() {
        for bytes in [
            &[0x81, 7, 0x81, 0x80][..],
            &[0x82, 7, 0x80, 0x80],
            &[0x82, 7, 0x9f, 0xff],
            &[0x82, 7, 0x81, 0xff],
            &[0x82, 7, 0x81, 0xf6],
        ] {
            assert!(parse_block_header(bytes).is_err(), "{bytes:02x?}");
        }
    }

    #[test]
    fn decode_returns_inner_block() {
        let header = make_header(1, 42, None);
        let era_history = &*PREPROD_ERA_HISTORY;

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
        assert_eq!(raw_block.body_hash().expect("body hash"), decoded_block.body_hash());
        assert_eq!(decoded_block.body_hash(), decoded_block.header.body().block_body_hash);
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
        assert_eq!(tx.body.id().to_string(), "43f396b0d5c55e34b507cfe9964672586370cc09912a4790488fba4079f96429");
    }

    #[test]
    fn json_is_hex_string_and_cbor_is_byte_string() {
        let payload = [0x82u8, 0x07, 0x85];
        crate::utils::serde::bytes::assert_json_hex_and_cbor_bstr(&super::RawBlock::from(payload.as_slice()), &payload);
    }
}

#[cfg(test)]
mod multi_era_tests {
    use test_case::test_case;

    use super::*;
    use crate::{
        BlockHeight, Hash, Hasher, IsHeader, RawBlock, cardano::network_block::CONWAY_BLOCK, make_header, to_cbor,
    };

    fn network_block(variant: u8, parent: Option<&[u8]>, indefinite: bool) -> Vec<u8> {
        let mut e = cbor::Encoder::new(Vec::new());
        if indefinite {
            e.begin_array().unwrap();
        } else {
            e.array(2).unwrap();
        }
        e.u8(variant).unwrap();
        let block_fields = match variant {
            0..=1 => 3,
            2..=4 => 4,
            _ => 5,
        };
        if indefinite {
            e.begin_array().unwrap();
        } else {
            e.array(block_fields).unwrap();
        }
        if variant <= 1 {
            e.array(5).unwrap().u32(764_824_073).unwrap();
            match parent {
                Some(parent) => {
                    e.bytes(parent).unwrap();
                }
                None => {
                    e.null().unwrap();
                }
            }
            e.bytes(&[0; 32]).unwrap();
            if variant == 0 {
                e.array(2).unwrap().u64(1).unwrap().array(1).unwrap().u64(9).unwrap();
            } else {
                e.array(4).unwrap().array(2).unwrap().u64(1).unwrap().u64(2).unwrap();
                e.bytes(&[0; 64]).unwrap();
                e.array(1).unwrap().u64(9).unwrap().array(2).unwrap().u8(0).unwrap();
                e.bytes(&[0; 64]).unwrap();
            }
            e.array(1).unwrap().map(0).unwrap();
        } else {
            if indefinite {
                e.begin_array().unwrap();
            } else {
                e.array(2).unwrap();
            }
            let header_fields = match variant {
                2..=5 => 15,
                _ => 10,
            };
            if indefinite {
                e.begin_array().unwrap();
            } else {
                e.array(header_fields).unwrap();
            }
            e.u64(9).unwrap().u64(21_602).unwrap();
            match parent {
                Some(parent) => {
                    e.bytes(parent).unwrap();
                }
                None => {
                    e.null().unwrap();
                }
            }
            if matches!(variant, 6..=7) {
                let body = to_cbor(make_header(9, 21_602, None).body());
                let mut decoder = cbor::Decoder::new(&body);
                decoder.array().unwrap();
                for _ in 0..3 {
                    decoder.skip().unwrap();
                }
                e.writer_mut().extend_from_slice(&body[decoder.position()..]);
            } else {
                for _ in 3..header_fields {
                    e.null().unwrap();
                }
            }
            if indefinite {
                e.end().unwrap();
            }
            e.bytes(&[0; KES_SIGNATURE]).unwrap();
            if indefinite {
                e.end().unwrap();
            }
        }
        match variant {
            0..=1 => {
                if variant == 0 {
                    e.array(0).unwrap();
                } else {
                    e.array(4).unwrap();
                    for _ in 0..4 {
                        e.array(0).unwrap();
                    }
                }
                e.array(1).unwrap().map(0).unwrap();
            }
            _ => {
                e.array(0).unwrap().array(0).unwrap().map(0).unwrap();
                if variant >= 5 {
                    e.array(0).unwrap();
                }
            }
        }
        if indefinite {
            e.end().unwrap().end().unwrap();
        }
        e.into_writer()
    }

    #[test_case(0, EraName::Byron)]
    #[test_case(1, EraName::Byron)]
    #[test_case(2, EraName::Shelley)]
    #[test_case(3, EraName::Allegra)]
    #[test_case(4, EraName::Mary)]
    #[test_case(5, EraName::Alonzo)]
    #[test_case(6, EraName::Babbage)]
    #[test_case(7, EraName::Conway)]
    fn decodes_chain_metadata(variant: u8, era: EraName) {
        for indefinite in [false, true] {
            let parent = [42; 32];
            let bytes = network_block(variant, Some(&parent), indefinite);
            let raw = RawBlock::from(bytes.as_slice());
            let block = raw.decode_multi_era().unwrap();
            let header = block.header();
            assert_eq!(header.era(), era);
            assert_eq!(header.is_epoch_boundary(), variant == 0);
            assert_eq!(header.point(21_600).unwrap().block_height(), BlockHeight::from(9));
            assert_eq!(
                header.point(21_600).unwrap().slot_or_default().as_u64(),
                if variant == 0 { 21_600 } else { 21_602 }
            );
            assert_eq!(block.parent_hash(), Some(HeaderHash::from(parent)));

            let mut hashed_bytes = Vec::new();
            if era == EraName::Byron {
                hashed_bytes.extend_from_slice(&[0x82, variant]);
            }
            hashed_bytes.extend_from_slice(header.cbor());
            assert_eq!(header.hash(), Hash::from(*Hasher::<256>::hash(&hashed_bytes)));
        }
    }

    #[test]
    fn matches_existing_conway_decoder() {
        let raw = RawBlock::from(CONWAY_BLOCK.as_slice());
        let current = raw.decode().unwrap();
        let historical = raw.decode_multi_era().unwrap();
        assert_eq!(historical.header().point(0).unwrap(), current.header.point());
        assert_eq!(historical.parent_hash(), current.header.parent_hash());
    }

    #[test]
    fn rejects_invalid_conway_issuer_key_in_both_decoders() {
        let mut bytes = network_block(7, Some(&[0; 32]), false);
        let mut decoder = cbor::Decoder::new(&bytes);
        decoder.array().unwrap();
        decoder.u8().unwrap();
        decoder.array().unwrap();
        decoder.array().unwrap();
        decoder.array().unwrap();
        for _ in 0..3 {
            decoder.skip().unwrap();
        }
        let issuer_key = decoder.position();
        bytes[issuer_key] = 0xf6;

        let raw = RawBlock::from(bytes.as_slice());
        assert!(raw.decode().is_err());
        assert!(raw.decode_multi_era().is_err());
        assert!(parse_block_header(&bytes).unwrap().point(0).is_err());
    }

    #[test]
    fn standalone_headers_match_block_metadata() {
        for variant in 0..=7 {
            let bytes = network_block(variant, Some(&[42; 32]), false);
            let header = MultiEraBlock::decode(&bytes).unwrap().header();
            let standalone = ParsedBlockHeader::from_cbor(variant, header.cbor()).unwrap();
            assert_eq!(standalone, header);
            assert_eq!(standalone.point(21_600).unwrap(), header.point(21_600).unwrap());
        }
    }

    #[test]
    fn header_parsing_does_not_read_block_body() {
        for variant in 0..=7 {
            for indefinite in [false, true] {
                let bytes = network_block(variant, Some(&[42; 32]), indefinite);
                let complete = MultiEraBlock::decode(&bytes).unwrap().header();
                let mut decoder = cbor::Decoder::new(&bytes);
                decoder.array().unwrap();
                decoder.u8().unwrap();
                decoder.array().unwrap();
                decoder.skip().unwrap();
                let header_only = &bytes[..decoder.position()];

                assert!(MultiEraBlock::decode(header_only).is_err());
                let header = parse_block_header(header_only).unwrap();
                assert_eq!(header.cbor(), complete.cbor());
                assert_eq!(header.point(21_600).unwrap(), complete.point(21_600).unwrap());
                assert_eq!(extract_block_header_cbor(header_only).unwrap(), complete.cbor());
            }
        }
    }

    #[test]
    fn decodes_parent_links_across_era_boundaries() {
        let mut previous = HeaderHash::from([7; 32]);
        for variant in 0..=7 {
            let bytes = network_block(variant, Some(previous.as_ref()), false);
            let block = MultiEraBlock::decode(&bytes).unwrap();
            assert_eq!(block.parent_hash(), Some(previous));
            previous = block.header().hash();
        }
    }

    #[test]
    fn rejects_truncated_or_trailing_data_in_every_era() {
        for variant in 0..=7 {
            for indefinite in [false, true] {
                let bytes = network_block(variant, Some(&[0; 32]), indefinite);
                for length in 0..bytes.len() {
                    assert!(MultiEraBlock::decode(&bytes[..length]).is_err(), "variant={variant}, length={length}");
                }
                let mut extra = bytes.clone();
                extra.push(0);
                assert!(MultiEraBlock::decode(&extra).is_err());
            }
        }
    }

    #[test]
    fn validates_parent_hash_encoding() {
        for variant in 0..=7 {
            for length in [0, 31, 33] {
                assert!(MultiEraBlock::decode(&network_block(variant, Some(&vec![0; length]), false)).is_err());
            }
            let bytes = network_block(variant, None, false);
            let decoded = MultiEraBlock::decode(&bytes);
            if variant <= 1 {
                assert!(decoded.is_err());
            } else {
                assert_eq!(decoded.unwrap().parent_hash(), None);
            }
        }
    }

    #[test]
    fn rejects_wrong_block_layout_and_unknown_variants() {
        for variant in 0..=7 {
            let bytes = network_block(variant, Some(&[0; 32]), false);
            for offset in [0, 2, 3] {
                let mut invalid = bytes.clone();
                invalid[offset] += 1;
                assert!(MultiEraBlock::decode(&invalid).is_err(), "variant={variant}, offset={offset}");
            }
            let mut wrong_body = bytes.clone();
            let body_start = 3 + MultiEraBlock::decode(&bytes).unwrap().header().cbor().len();
            wrong_body[body_start] = 0xf6;
            assert!(MultiEraBlock::decode(&wrong_body).is_err());
        }
        let mut bytes = network_block(7, Some(&[0; 32]), false);
        for tag in [8, 9, 23] {
            bytes[1] = tag;
            assert!(MultiEraBlock::decode(&bytes).is_err());
        }
    }

    #[test]
    fn rejects_invalid_byron_epoch_length() {
        for variant in 0..=1 {
            let bytes = network_block(variant, Some(&[0; 32]), false);
            let header = MultiEraBlock::decode(&bytes).unwrap().header();
            assert!(header.point(0).is_err());
            if variant == 1 {
                assert!(header.point(2).is_err());
                assert!(header.point(u64::MAX).is_err());
            }
        }
    }
}
