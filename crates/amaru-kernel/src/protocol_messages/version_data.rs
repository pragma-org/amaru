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

use crate::protocol_messages::handshake::check_length;
use crate::protocol_messages::{network_magic::NetworkMagic, version_number::VersionNumber};
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct VersionData {
    network_magic: NetworkMagic,
    initiator_only_diffusion_mode: bool,
    /// range [0, 1]
    peer_sharing: u8,
    query: bool,
}

pub const PEER_SHARING_DISABLED: u8 = 0;
pub const PEER_SHARING_ENABLED: u8 = 1;

impl VersionData {
    pub fn new(
        network_magic: NetworkMagic,
        initiator_only_diffusion_mode: bool,
        peer_sharing: u8,
        query: bool,
    ) -> Self {
        VersionData {
            network_magic,
            initiator_only_diffusion_mode,
            peer_sharing,
            query,
        }
    }
}

impl<T: AsRef<VersionNumber>> Encode<T> for VersionData {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        ctx: &mut T,
    ) -> Result<(), encode::Error<W::Error>> {
        if ctx.as_ref().has_query_and_peer_sharing() {
            e.array(4)?
                .encode(self.network_magic)?
                .bool(self.initiator_only_diffusion_mode)?
                .u8(self.peer_sharing)?
                .bool(self.query)?;
        } else {
            e.array(2)?
                .encode(self.network_magic)?
                .bool(self.initiator_only_diffusion_mode)?;
        }
        Ok(())
    }
}

impl<'b, T: AsRef<VersionNumber>> Decode<'b, T> for VersionData {
    fn decode(d: &mut Decoder<'b>, ctx: &mut T) -> Result<Self, decode::Error> {
        if ctx.as_ref().has_query_and_peer_sharing() {
            let len = d.array()?;
            check_length(0, len, 4)?;
            let network_magic = d.decode()?;
            let initiator_only_diffusion_mode = d.bool()?;
            let peer_sharing = d.u8()?;
            let query = d.bool()?;
            Ok(Self {
                network_magic,
                initiator_only_diffusion_mode,
                peer_sharing,
                query,
            })
        } else {
            let len = d.array()?;
            check_length(0, len, 2)?;
            let network_magic = d.decode()?;
            let initiator_only_diffusion_mode = d.bool()?;
            Ok(Self {
                network_magic,
                initiator_only_diffusion_mode,
                peer_sharing: 0,
                query: false,
            })
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::protocol_messages::network_magic::tests::any_network_magic;
    use proptest::prelude::any;
    use proptest::prop_compose;

    prop_compose! {
        pub fn any_version_data()(network_magic in any_network_magic(),
            initiator_only_diffusion_mode in any::<bool>(),
            peer_sharing in any::<u8>(),
            query in any::<bool>()) -> VersionData {
            VersionData::new(network_magic, initiator_only_diffusion_mode, peer_sharing, query)
        }
    }
}
