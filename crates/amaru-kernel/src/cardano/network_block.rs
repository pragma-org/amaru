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

#[cfg(any(test, feature = "test-utils"))]
mod tests;

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize)]
pub struct NetworkBlock {
    era_tag: u16,
    encoded_block: Vec<u8>,
}

impl NetworkBlock {
    /// This fake block is only to deserialize network block references from trace buffer messages.
    pub fn fake() -> Self {
        NetworkBlock {
            era_tag: 1,
            encoded_block: vec![1],
        }
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
