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

use crate::{Block, BlockHeader, RawBlock, cbor};
use amaru_minicbor_extra::to_cbor;
use std::fmt::{Debug, Formatter};
use std::sync::LazyLock;

/// A network block contains:
///  - An era tag identifying the Cardano era of the block, which determines its exact encoding.
///  - The block itself, encoded in CBOR format as per the Cardano network protocol.
///
/// From a `NetworkBlock` we can obtain:
///
///  - The raw CBOR bytes of the block, wrapped in a `RawBlock`.
///  - The decoded `Block` structure, by decoding the inner CBOR bytes.
///  - The decoded `BlockHeader`, by decoding only the header part of the inner CBOR bytes.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct NetworkBlock {
    era_tag: u16,
    encoded_block: Vec<u8>,
}

impl NetworkBlock {
    /// This fake block is only to deserialize network block references from trace buffer messages.
    pub fn fake() -> Self {
        NETWORK_BLOCK.clone()
    }

    pub fn era_tag(&self) -> u16 {
        self.era_tag
    }

    pub fn raw_block(&self) -> RawBlock {
        RawBlock::from(to_cbor(self).as_slice())
    }

    /// Decode the inner block from its raw CBOR representation.
    pub fn decode_block(&self) -> Result<Block, minicbor::decode::Error> {
        minicbor::decode(&self.encoded_block)
    }

    /// Decode only the header from the raw CBOR representation of the block.
    pub fn decode_header(&self) -> Result<BlockHeader, cbor::decode::Error> {
        let mut decoder = minicbor::Decoder::new(&self.encoded_block);
        // format: [header, tx_bodies, witnesses, auxiliary_data?, invalid_transactions?]
        decoder.array()?;
        decoder.decode()
    }
}

#[expect(clippy::expect_used)]
pub static CONWAY_BLOCK: LazyLock<Vec<u8>> = LazyLock::new(|| {
    // These bytes are Conway3.block from Pallas https://github.com/txpipe/pallas/blob/main/test_data/conway3.block
    hex::decode("820785828a1a00153df41a01aa8a0458201bbf3961f179735b68d8f85bcff85b1eaaa6ec3fa6218e4b6f4be7c6129e37ba5820472a53a312467a3b66ede974399b40d1ea428017bc83cf9647d421b21d1cb74358206ee6456894a5931829207e497e0be77898d090d0ac0477a276712dee34e51e05825840d35e871ff75c9a243b02c648bccc5edf2860edba0cc2014c264bbbdb51b2df50eff2db2da1803aa55c9797e0cc25bdb4486a4059c4687364ad66ed15b4ec199f58508af7f535948fac488dc74123d19c205ea2b02cbbf91104bbad140d4ba4bb4d75f7fdb762586802f116bdba3ecaa0840614a2b96d619006c3274b590bcd2599e39a17951cbc3db6348fa2688158384f081901965820d8038b5679ffc770b060578bcd7b33045f2c3aa5acc7bd8cde8b705cfe673d7584582030449be32ae7b8363fde830fc9624945862b281e481ec7f5997c75d1f2316c560018ca5840f5d96ce2055a67709c8e6809c882f71ebd7fc6350018d36d803a55b9230ec6c4cbcd41a09255db45214e278f89b39005ac0f213473acbf455165cdcaa9558e0c8209005901c02ba5dda40daa84b3f9c524016c21d7ce13f585062e35298aa31ea590fee809e75ae999dff9b3ee188e01cfcecc384faba50ca673af2388c3cf7407206019920e99e195bc8e6d1a42ef2b7fb549a8da0591180da17db7a24334b098bfef839334761ec51c2bd8a044fd1785b4e216f811dbdcba63eb853a477d3ea87a3b2d61ccfeae74765c51ec1313ffb121573bae4fc3a742825168760f615a0b2b6ef8a42084f9465501774310772de17a574d8d6bef6b14f4277c8b792b4f60f6408262e7aee5e95b8539df07f953d16b209b6d8fa598a6c51ab90659523720c98ffd254bf305106c0b9c6938c33323e191b5afbad8939270c76a82dc2124525aab11396b9de746be6d7fae2c1592c6546474cebe07d1f48c05f36f762d218d9d2ca3e67c27f0a3d82cdd1bab4afa7f3f5d3ecb10c6449300c01b55e5d83f6cefc6a12382577fc7f3de09146b5f9d78f48113622ee923c3484e53bff74df65895ec0ddd43bc9f00bf330681811d5d20d0e30eed4e0d4cc2c75d1499e05572b13fb4e7b0dabf6e36d1988b47fbdecffc01316885f802cd6c60e044bf50a15418530d628cffd506d4eb0db6155be94ce84fbf6529ee06ec78e9c3009c0f5504978dd150926281a400d90102828258202e6b2226fd74ab0cadc53aaa18759752752bd9b616ea48c0e7b7be77d1af4bf400825820d5dc99581e5f479d006aca0cd836c2bb7ddcd4a243f8e9485d3c969df66462cb00018182583900bbe56449ba4ee08c471d69978e01db384d31e29133af4546e6057335061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965411b0000000ba4332169021a0002c71d14d9010281841b0000000ba43b7400581de0061771ead84921c0ca49a4b48ab03c2ad1b45a182a46485ed1c965418400f6a2001bffffffffffffffff09d81e821bfffffffffffffffe1bfffffffffffffffff68275687474703a2f2f636f73746d646c732e74657374735820931f1d8cdfdc82050bd2baadfe384df8bf99b00e36cb12bfb8795beab3ac7fe581a100d9010281825820794ff60d3c35b97f55896d1b2a455fe5e89b77fb8094d27063ff1f260d21a67358403894a10bf9fca0592391cdeabd39891fc2f960fae5a2743c73391c495dfdf4ba4f1cb5ede761bebd7996eba6bbe4c126bcd1849afb9504f4ae7fb4544a93ff0ea080").expect("Failed to decode Conway3.block hex")
});

#[expect(clippy::expect_used)]
pub static NETWORK_BLOCK: LazyLock<NetworkBlock> = LazyLock::new(|| {
    NetworkBlock::try_from(RawBlock::from(CONWAY_BLOCK.as_slice()))
        .expect("Failed to parse Conway3.block hex")
});

impl minicbor::Encode<()> for NetworkBlock {
    fn encode<W: minicbor::encode::Write>(
        &self,
        e: &mut minicbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), minicbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.u16(self.era_tag)?;
        // Write the already-encoded CBOR term directly into the output stream.
        // This does NOT add any CBOR envelope (no bytestring tag, no extra array, etc).
        e.writer_mut()
            .write_all(&self.encoded_block)
            .map_err(minicbor::encode::Error::write)?;
        Ok(())
    }
}

impl<'b> minicbor::Decode<'b, ()> for NetworkBlock {
    fn decode(
        d: &mut minicbor::Decoder<'b>,
        _ctx: &mut (),
    ) -> Result<Self, minicbor::decode::Error> {
        let len = d.array()?;
        if len != Some(2) {
            return Err(minicbor::decode::Error::message(format!(
                "invalid NetworkBlock array length. Expected 2, got {len:?}"
            )));
        }
        let era_tag = d.u16()?;
        let start = d.position();
        d.skip()?; // skip exactly one CBOR item (the block term)
        let end = d.position();
        let bytes = &d.input()[start..end];
        Ok(NetworkBlock {
            era_tag,
            encoded_block: bytes.to_vec(),
        })
    }
}

impl Debug for NetworkBlock {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NetworkBlock")
            .field("era_tag", &self.era_tag)
            .field("raw_block", &format!("{:?}", self.encoded_block))
            .finish()
    }
}

impl TryFrom<RawBlock> for NetworkBlock {
    type Error = minicbor::decode::Error;

    fn try_from(value: RawBlock) -> Result<Self, minicbor::decode::Error> {
        minicbor::decode(&value)
    }
}

#[cfg(any(test, feature = "test-utils"))]
mod tests;

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(test)]
mod network_block_tests {
    use super::*;

    mod network_block_cbor_roundtrip {
        use super::*;
        use crate::prop_cbor_roundtrip;
        prop_cbor_roundtrip!(NetworkBlock, any_network_block());
    }

    #[test]
    fn decode_network_block() {
        let as_hex = "820785828a1a002cc8f51a04994d195820f27eddec5e782552e6ef408cff7c4a27e505fe54c20a717027d97e1c91da9d7c5820064effe4fa426184a911159fa803a9c1092459cd0b8f3e584ef9513955be0f5558201e5d0dcf77643d89a94353493859a21b47672015fb652b51f922617e4b27da8982584042d0edd71e6cac29e45f61eabbcce4f803f2ff78bce9fa295d11cb7c3cddb60f7694faaea787183fd604267d8114b57453493c963c7485405838cd79a261013a5850bc8672b4ff2db478e5b21364bfa9f0a2f5265e5ac56b261ce3dcb7ac57301a8362573eef2ae23eb2540915704534d1c0af8eace59a25c130629af7600b175b5e234b376961e2fd12b37de5213e8eff0304582029571d16f081709b3c48651860077bebf9340abb3fc7133443c54f1f5a5edcf1845820ee1d7c2bd6978e3bc8a47fc478424a9efd797f16813164db292320e3728f6de5091902465840f69f8974108be5df23dd0dad2f0e888e5c1702c35c678f3b7a2802f272666ea8a7c9b9f6e786e761d4cb747159d68b7d8f43bceae6ab4e543795d8aded59c302820a005901c06063a37f6f01765b34bceb2651e40a69e3bc31b35fd6c952415175844132250cdcbafd19c39952f471f7318a5cc3e45f54dadc9067bb6d25dac8b76f0bea5106c2f45235fac710d3e78d259af37fd617ed9e372626c5b080359ba1bf5150df764365e0faedfe66ab7e338f7aec558e0a192f4f744b473fbe669013ade2cd144c7742c3ff1d78002af59b0f1b45807bce21f592d23596c54d37095b52a8f942c763f5f014aa161fc18123054a618e8ecb9256c392c3bebcb30e10b2c4bef64f4c3b0aea29a4378a53b6d061c9000b510c0bf76d87171fb357faeb54087718fea0ee33e048d4a1aa8a831f7f9148ebbbb2d79f58c61268e1e1369ae88e2369e65e57169cc477726944790423f9dee584fb9eceeee79a447c075ada7bceb6a28699f0721415d3d0ab8f20b77410bc5faf296ce126cb73b9aaab208b9844d95d127ccaefac37c323cc1957aad3350c2d176916593aa854be50e7c36857adcf51800d490ce082908c5a1aceb8fd51fffc67abaf2c09c1f957bc2e009b8a76394402211eac5ff26c2e5d69aa2c6f4a0e4f2ac28c1482b4706916a0c876d56952b1db18af64658f6249db7fe7e7e366fd2a0f869472d38edb6145404f556025ea0066228080a080";
        let bytes = hex::decode(as_hex).expect("valid hex");
        let network_block: NetworkBlock = minicbor::decode(&bytes).expect("a valid network block");
        assert_eq!(network_block.era_tag, 7);
    }
}
