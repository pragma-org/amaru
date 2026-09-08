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

use std::{
    fmt::Display,
    ops::{BitAnd, BitOr, Not},
};

use amaru_kernel::{NetworkMagic, cbor};

use crate::protocol_messages::version_number::VersionNumber;

/// Node-to-node handshake peer-sharing willingness (`0` disabled, `1` enabled on the wire).
///
/// Combined with [`BitAnd`]: enabled only when both sides offer it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub enum PeerSharing {
    Disabled,
    Enabled,
}

impl From<bool> for PeerSharing {
    fn from(enabled: bool) -> Self {
        if enabled { Self::Enabled } else { Self::Disabled }
    }
}

impl From<PeerSharing> for bool {
    fn from(value: PeerSharing) -> Self {
        match value {
            PeerSharing::Enabled => true,
            PeerSharing::Disabled => false,
        }
    }
}

impl BitAnd for PeerSharing {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        (bool::from(self) && bool::from(rhs)).into()
    }
}

impl BitOr for PeerSharing {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        (bool::from(self) || bool::from(rhs)).into()
    }
}

impl Not for PeerSharing {
    type Output = Self;

    fn not(self) -> Self::Output {
        (!bool::from(self)).into()
    }
}

impl Display for PeerSharing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl<C> cbor::Encode<C> for PeerSharing {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::Disabled => e.u8(0)?,
            Self::Enabled => e.u8(1)?,
        };
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for PeerSharing {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        match d.u8()? {
            0 => Ok(Self::Disabled),
            1 => Ok(Self::Enabled),
            n => Err(cbor::decode::Error::message(format!("invalid peer sharing: expected 0 or 1, got {n}"))),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct VersionData {
    network_magic: NetworkMagic,
    initiator_only_diffusion_mode: bool,
    peer_sharing: PeerSharing,
    query: bool,
}

impl VersionData {
    pub fn new(
        network_magic: NetworkMagic,
        initiator_only_diffusion_mode: bool,
        peer_sharing: PeerSharing,
        query: bool,
    ) -> Self {
        VersionData { network_magic, initiator_only_diffusion_mode, peer_sharing, query }
    }

    pub fn network_magic(&self) -> NetworkMagic {
        self.network_magic
    }

    pub fn initiator_only_diffusion_mode(&self) -> bool {
        self.initiator_only_diffusion_mode
    }

    pub fn peer_sharing(&self) -> PeerSharing {
        self.peer_sharing
    }

    pub fn query(&self) -> bool {
        self.query
    }

    /// Returns whether this peer can act as both initiator and responder (full duplex).
    /// See initiator_only_diffusion_mode in the handshake spec.
    pub fn is_full_duplex_capable(&self) -> bool {
        !self.initiator_only_diffusion_mode
    }

    /// Whether the remote peer is willing to be advertised via peer sharing.
    pub fn is_advertisable(&self) -> bool {
        self.peer_sharing == PeerSharing::Enabled
    }

    /// Combine two version-data records for the same NTN version.
    ///
    /// `networkMagic` must match. `initiatorOnlyDiffusionMode` and `query` are OR;
    /// `peerSharing` is enabled only if both offers are enabled.
    pub(crate) fn combine(&self, other: &Self) -> Result<Self, &'static str> {
        if self.network_magic != other.network_magic {
            return Err("network magic mismatch");
        }
        Ok(Self {
            network_magic: self.network_magic,
            initiator_only_diffusion_mode: self.initiator_only_diffusion_mode || other.initiator_only_diffusion_mode,
            peer_sharing: self.peer_sharing & other.peer_sharing,
            query: self.query || other.query,
        })
    }
}

impl Display for VersionData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ network_magic: {}, initiator_only_diffusion_mode: {}, peer_sharing: {}, query: {} }}",
            self.network_magic, self.initiator_only_diffusion_mode, self.peer_sharing, self.query
        )
    }
}

impl<T: AsRef<VersionNumber>> cbor::Encode<T> for VersionData {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut T,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        if ctx.as_ref().has_query_and_peer_sharing() {
            e.array(4)?
                .encode(self.network_magic)?
                .bool(self.initiator_only_diffusion_mode)?
                .encode(self.peer_sharing)?
                .bool(self.query)?;
        } else {
            e.array(2)?.encode(self.network_magic)?.bool(self.initiator_only_diffusion_mode)?;
        }
        Ok(())
    }
}

impl<'b, T: AsRef<VersionNumber>> cbor::Decode<'b, T> for VersionData {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut T) -> Result<Self, cbor::decode::Error> {
        if ctx.as_ref().has_query_and_peer_sharing() {
            let len = d.array()?;
            cbor::check_tagged_array_length(0, len, 4)?;
            let network_magic = d.decode()?;
            let initiator_only_diffusion_mode = d.bool()?;
            let peer_sharing = d.decode()?;
            let query = d.bool()?;
            Ok(Self { network_magic, initiator_only_diffusion_mode, peer_sharing, query })
        } else {
            let len = d.array()?;
            cbor::check_tagged_array_length(0, len, 2)?;
            let network_magic = d.decode()?;
            let initiator_only_diffusion_mode = d.bool()?;
            Ok(Self { network_magic, initiator_only_diffusion_mode, peer_sharing: PeerSharing::Disabled, query: false })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use amaru_kernel::any_network_magic;
    use proptest::{prelude::any, prop_compose, strategy::Strategy};

    use super::*;

    prop_compose! {
        pub fn any_version_data()(network_magic in any_network_magic(),
            initiator_only_diffusion_mode in any::<bool>(),
            peer_sharing in any::<bool>().prop_map(PeerSharing::from),
            query in any::<bool>()) -> VersionData {
            VersionData::new(network_magic, initiator_only_diffusion_mode, peer_sharing, query)
        }
    }

    #[test]
    fn peer_sharing_boolean_ops() {
        use PeerSharing::{Disabled, Enabled};

        assert_eq!(Enabled & Enabled, Enabled);
        assert_eq!(Enabled & Disabled, Disabled);
        assert_eq!(Disabled & Enabled, Disabled);
        assert_eq!(Disabled & Disabled, Disabled);

        assert_eq!(Enabled | Enabled, Enabled);
        assert_eq!(Enabled | Disabled, Enabled);
        assert_eq!(Disabled | Enabled, Enabled);
        assert_eq!(Disabled | Disabled, Disabled);

        assert_eq!(!Enabled, Disabled);
        assert_eq!(!Disabled, Enabled);
    }

    #[test]
    fn rejects_unknown_peer_sharing_wire_value() {
        let err = cbor::decode::<PeerSharing>(&[0x02]).unwrap_err();
        assert!(err.to_string().contains("invalid peer sharing"), "{err}");
    }
}
