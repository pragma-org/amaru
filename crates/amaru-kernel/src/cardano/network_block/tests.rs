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

use proptest::prelude::*;

use super::*;
use crate::{EraHistory, Header, HeaderHash, any_header};

/// A network-block blob together with the header actually encoded in it.
///
/// `make_block_with_header` rewrites `block_body_hash`, so a seed header is not a valid
/// content address for the resulting blob. Tests that store blocks must use this header
/// (and key the blob by `header.hash()`), not the seed they started from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EncodedTestBlock {
    pub header: Header,
    pub raw: RawBlock,
}

impl EncodedTestBlock {
    /// Encode `seed` as a network block and take the header from that encoding.
    #[expect(clippy::expect_used)]
    pub fn from_seed(seed: &Header, era_history: &EraHistory) -> Self {
        let raw = make_encoded_block(seed, era_history);
        let header = raw.decode_header().expect("encoded test block must contain a header");
        Self { header, raw }
    }
}

/// Encode `seeds` as a parent-linked chain of network blocks.
///
/// Each header is taken from its encoded block. Block `n+1`'s `prev_hash` is block `n`'s
/// header hash (the first seed keeps its original parent).
pub fn make_encoded_chain(seeds: impl IntoIterator<Item = Header>, era_history: &EraHistory) -> Vec<EncodedTestBlock> {
    let mut parent: Option<HeaderHash> = None;
    let mut first = true;
    seeds
        .into_iter()
        .map(|mut seed| {
            if first {
                first = false;
            } else {
                seed.body_mut().prev_hash = parent;
            }
            let block = EncodedTestBlock::from_seed(&seed, era_history);
            parent = Some(block.header.hash());
            block
        })
        .collect()
}

/// Create a network block with the correct era tag based on the header's slot and era history.
#[expect(clippy::expect_used)]
pub fn make_network_block(header: &Header, era_history: &EraHistory) -> NetworkBlock {
    let block = make_block_with_header(header);
    NetworkBlock::new(era_history, &block).expect("make network block")
}

/// Create an encoded block with the correct era tag based on the header's slot and era history.
///
/// The header encoded in the blob may hash differently from `header` because
/// [`make_block_with_header`] rewrites `block_body_hash`. Prefer [`EncodedTestBlock::from_seed`]
/// when the result will be stored under a header hash.
pub fn make_encoded_block(header: &Header, era_history: &EraHistory) -> RawBlock {
    let network_block = make_network_block(header, era_history);
    RawBlock::from(to_cbor(&network_block).as_slice())
}

#[expect(clippy::expect_used)]
pub fn make_block_with_header(header: &Header) -> Block {
    let mut block = make_block();
    block.header = header.clone();
    // Re-encode and decode to rebuild the cached metadata fields.
    let bytes = to_cbor(&block);
    let mut block = cbor::decode::<Block>(bytes.as_slice()).expect("block encoding should round-trip");
    block.header.body_mut().block_body_hash = block.body_hash();
    cbor::decode(&to_cbor(&block)).expect("block encoding should round-trip")
}

/// Generate an arbitrary network block at Conway era for property-based testing.
pub fn any_network_block() -> impl Strategy<Value = NetworkBlock> {
    any_header().prop_map(|header| {
        let block = make_block_with_header(&header);
        NetworkBlock { era_tag: EraName::Conway, encoded_block: to_cbor(&block) }
    })
}

#[expect(clippy::expect_used)]
pub fn make_block() -> Block {
    // These bytes are Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
    let bytes = hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex");
    let (_, block): (u16, Block) = cbor::decode(bytes.as_slice()).expect("Failed to parse Conway3.block bytes");
    block
}

#[cfg(test)]
mod encoded_test_block {
    use super::*;
    use crate::{IsHeader, make_header};

    #[test]
    fn extracted_header_addresses_the_blob() {
        let era = EraHistory::default();
        let block = EncodedTestBlock::from_seed(&make_header(1, 0, None), &era);
        assert_eq!(block.header, block.raw.decode_header().unwrap());
        assert_eq!(block.header.body().block_body_hash, block.raw.body_hash().unwrap());

        let again = EncodedTestBlock::from_seed(&block.header, &era);
        assert_eq!(again.header.hash(), block.header.hash());
        assert_eq!(again.raw, block.raw);
    }

    #[test]
    fn chain_parent_hashes_match_extracted_headers() {
        let era = EraHistory::default();
        let seeds = (0..3).map(|i| make_header(i + 1, i + 1, None));
        let chain = make_encoded_chain(seeds, &era);
        assert_eq!(chain[1].header.parent(), Some(chain[0].header.hash()));
        assert_eq!(chain[2].header.parent(), Some(chain[1].header.hash()));
    }
}
