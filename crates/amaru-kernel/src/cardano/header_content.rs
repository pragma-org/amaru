// Copyright 2025 PRAGMA
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

use crate::{BlockHeight, EraName, Header, cbor, extract_block_header_cbor, to_cbor, utils::debug_bytes};

/// Era-tagged header bytes served by ChainSync, preserving the original CBOR.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, Ord, PartialOrd)]
pub struct HeaderContent {
    pub variant: EraName,
    pub byron_prefix: Option<(u8, u64)>,
    #[serde(with = "crate::utils::serde::bytes")]
    pub cbor: Vec<u8>,
}

impl std::fmt::Debug for HeaderContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HeaderContent")
            .field("variant", &self.variant)
            .field("byron_prefix", &self.byron_prefix)
            .field("cbor", &debug_bytes(&self.cbor, 300))
            .field("cbor_len", &self.cbor.len())
            .finish()
    }
}

impl HeaderContent {
    /// Extract a header from a stored network block without decoding its body.
    pub fn from_block(block: &[u8]) -> Result<Self, cbor::decode::Error> {
        let mut decoder = cbor::Decoder::new(block);
        decoder.array()?;
        let tag = decoder.u8()?;
        let variant = EraName::from_header_variant(tag.saturating_sub(1)).map_err(cbor::decode::Error::custom)?;
        Ok(Self {
            variant,
            byron_prefix: (tag <= 1).then_some((tag, block.len() as u64)),
            cbor: extract_block_header_cbor(block)?.to_vec(),
        })
    }

    /// Block number, encoded as chain difficulty in Byron headers.
    pub fn block_height(&self) -> Result<BlockHeight, cbor::decode::Error> {
        let mut decoder = cbor::Decoder::new(&self.cbor);
        decoder.array()?;
        if let Some((tag, _)) = self.byron_prefix {
            decoder.skip()?; // Protocol magic.
            decoder.skip()?; // Parent hash.
            decoder.skip()?; // Body proof.
            decoder.array()?;
            decoder.skip()?; // Epoch or slot identifier.
            if tag == 1 {
                decoder.skip()?; // Issuer public key.
            }
        }
        decoder.array()?;
        decoder.u64().map(BlockHeight::from)
    }

    pub fn new(header: &Header, era: EraName) -> Self {
        Self { variant: era, byron_prefix: None, cbor: to_cbor(header) }
    }

    pub fn with_bytes(bytes: Vec<u8>, variant: EraName) -> Self {
        Self { variant, byron_prefix: None, cbor: bytes }
    }
}

impl<'b> cbor::Decode<'b, ()> for HeaderContent {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        let variant = EraName::from_header_variant(d.u8()?).map_err(cbor::decode::Error::custom)?;

        match variant {
            EraName::Byron => {
                cbor::check_tagged_array_length(0, len, 2)?;
                let len = d.array()?;
                cbor::check_tagged_array_length(0, len, 2)?;

                // Byron block type (boundary or regular) and encoded block size.
                let (a, b): (u8, u64) = d.decode()?;

                d.tag()?;
                // Conformance: the Haskell node unwraps CBOR-in-CBOR with cborg's `decodeBytes`,
                // which rejects indefinite-length byte strings, so we reject them too.
                #[allow(clippy::disallowed_methods)]
                let bytes = d.bytes()?;

                Ok(HeaderContent { variant, byron_prefix: Some((a, b)), cbor: Vec::from(bytes) })
            }
            EraName::Shelley
            | EraName::Allegra
            | EraName::Mary
            | EraName::Alonzo
            | EraName::Babbage
            | EraName::Conway
            | EraName::Dijkstra => {
                cbor::check_tagged_array_length(variant.header_variant().into(), len, 2)?;
                d.tag()?;
                // Conformance: the Haskell node unwraps CBOR-in-CBOR with cborg's `decodeBytes`,
                // which rejects indefinite-length byte strings, so we reject them too.
                #[allow(clippy::disallowed_methods)]
                let bytes = d.bytes()?;
                Ok(HeaderContent { variant, byron_prefix: None, cbor: Vec::from(bytes) })
            }
        }
    }
}

impl cbor::Encode<()> for HeaderContent {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.u8(self.variant.header_variant())?;

        if self.variant == EraName::Byron {
            e.array(2)?;

            if let Some((a, b)) = self.byron_prefix {
                e.array(2)?;
                e.u8(a)?;
                e.u64(b)?;
            } else {
                return Err(cbor::encode::Error::message("header variant 0 but no byron prefix"));
            }

            e.tag(cbor::IanaTag::Cbor)?;
            e.bytes(&self.cbor)?;
        } else {
            e.tag(cbor::IanaTag::Cbor)?;
            e.bytes(&self.cbor)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Hasher, RawBlock,
        cardano::{era_name::ERA_NAMES, network_block::CONWAY_BLOCK},
        from_cbor,
    };

    #[test]
    fn extracts_header_bytes_and_era_from_network_blocks() {
        for era in ERA_NAMES.into_iter().filter(|era| *era != EraName::Byron) {
            let mut bytes = CONWAY_BLOCK.clone();
            bytes[1] = era as u8;
            let block = RawBlock::from(bytes.into_boxed_slice());
            let header = HeaderContent::from_block(&block).unwrap();
            assert_eq!(header.variant, era);
            assert_eq!(header.byron_prefix, None);
            assert_eq!(header.cbor, extract_block_header_cbor(&block).unwrap());
            assert_eq!(header.block_height().unwrap(), BlockHeight::from(1392116));
            assert_eq!(from_cbor::<HeaderContent>(&to_cbor(&header)), Some(header));
        }
    }

    #[test]
    fn extracts_byron_regular_header() {
        // https://github.com/txpipe/pallas/blob/main/test_data/byron1.block
        let block = RawBlock::from(hex::decode("820183851a2d964a095820a5a5c235200bbe91f16cd3e5dcb67369c25dcfb1279c83d22021f5d960c485e684830058200e5751c026e543b2e8ab2eb06099daa1d1e5df47778f7787faab45cdf12fe3a85820afc0da64183bf2664f3d4eec7238d524ba607faeeab24fc100eb861dba69971b82035820d36a2619a672494604e11bb447cbcf5231e9f2ba25c2169177edc941bd50ad6c5820afc0da64183bf2664f3d4eec7238d524ba607faeeab24fc100eb861dba69971b58204e66280cd94d591072349bec0a3090a53aa945562efb6d08d56e53654b0e4098848218cf19545a584026566e86fc6b9b177c8480e275b2b112b573f6d073f9deea53b8d99c4ed976b335b2b3842f0e380001f090bc923caa9691ed9115e286da9421e2745c7acc87f1811a004485098202828400584026566e86fc6b9b177c8480e275b2b112b573f6d073f9deea53b8d99c4ed976b335b2b3842f0e380001f090bc923caa9691ed9115e286da9421e2745c7acc87f15840f14f712dc600d793052d4842d50cefa4e65884ea6cf83707079eb8ce302efc85dae922d5eb3838d2b91784f04824d26767bfb65bd36a36e74fec46d09d98858d58408ab43e904b06e799c1817c5ced4f3a7bbe15cdbf422dea9d2d5dc2c6105ce2f4d4c71e5d4779f6c44b770a133636109949e1f7786acb5a732bcdea0470fea4065840c15d273e3e0735777e2b38359224803aa4a0fcd1cde37ed1f2f399d7e095b18bdc698a2aff59b2118cbbe45498c7bdc6ccf3a88a806cbd3e73bcf2fc6d7364028483020000826a63617264616e6f2d736c01a058204ba92aa320c60acc9ad7b9a64f2eda55c4d2ec28e604faf186708b4f0c4e8edf849fff8203d90102809fff82809fff81a0").unwrap().into_boxed_slice());
        let header = HeaderContent::from_block(&block).unwrap();
        assert_eq!(header.variant, EraName::Byron);
        assert_eq!(header.byron_prefix, Some((1, 636)));
        assert_eq!(header.block_height().unwrap(), BlockHeight::from(4490505));
        assert_eq!(header.cbor.len(), 616);
        assert!(hex::encode(to_cbor(&header)).starts_with("820082820119027cd818590268"));
        assert_eq!(from_cbor::<HeaderContent>(&to_cbor(&header)), Some(header));
    }

    #[test]
    fn reads_genesis_boundary_header() {
        // Header from https://github.com/txpipe/pallas/blob/main/test_data/genesis.block
        let header = HeaderContent {
            variant: EraName::Byron,
            byron_prefix: Some((0, 648087)),
            cbor: hex::decode("851a2d964a0958205f20df933584822601f9e3f8c024eb5eb252fe8cefb24d1317dc3d432e940ebb582054f0972807b68698fcd704e3dae675dc43c8a6656aeaf4891e6e3f23be2de00a8200810081a0").unwrap(),
        };
        assert_eq!(header.block_height().unwrap(), BlockHeight::from(0));
        let mut hasher = Hasher::<256>::new();
        hasher.input(&[0x82, 0]);
        hasher.input(&header.cbor);
        assert_eq!(hasher.finalize().to_string(), "89d9b5a5b8ddc8d7e5a6795e9774d97faf1efea59b2caf7eaf9f8c5b32059df4");
        assert_eq!(from_cbor::<HeaderContent>(&to_cbor(&header)), Some(header));
    }
}
