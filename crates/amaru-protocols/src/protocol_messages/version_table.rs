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
    collections::BTreeMap,
    fmt,
    fmt::{Debug, Display},
};

use amaru_kernel::{NetworkMagic, cbor};

use crate::protocol_messages::{
    version_data::{PeerSharing, VersionData},
    version_number::VersionNumber,
};

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct VersionTable<T> {
    pub values: BTreeMap<VersionNumber, T>,
    /// Versions this node does not implement, each preserved as its original CBOR data item.
    ///
    /// A peer may offer a newer node-to-node version beside versions both sides speak. The extra
    /// item stays in the table so negotiation can still choose a version this node decodes.
    #[serde(default)]
    pub unknown: BTreeMap<VersionNumber, Vec<u8>>,
}

impl VersionTable<VersionData> {
    pub fn empty() -> VersionTable<VersionData> {
        VersionTable { values: BTreeMap::new(), unknown: BTreeMap::new() }
    }

    pub fn query(network_magic: NetworkMagic) -> VersionTable<VersionData> {
        let data = VersionData::new(network_magic, false, PeerSharing::Disabled, true);
        Self::from_v11_through(VersionNumber::CURRENT, data)
    }

    /// Handshake offer from V11 through [`VersionNumber::CURRENT`] (V15).
    pub fn v11_and_above(
        network_magic: NetworkMagic,
        initiator_only_diffusion_mode: bool,
        advertisable: bool,
    ) -> VersionTable<VersionData> {
        Self::v11_through(VersionNumber::CURRENT, network_magic, initiator_only_diffusion_mode, advertisable)
    }

    /// Handshake offer from V11 up to and including `max` (clamped to [`VersionNumber::CURRENT`]).
    pub fn v11_through(
        max: VersionNumber,
        network_magic: NetworkMagic,
        initiator_only_diffusion_mode: bool,
        advertisable: bool,
    ) -> VersionTable<VersionData> {
        let data = VersionData::new(network_magic, initiator_only_diffusion_mode, advertisable.into(), false);
        Self::from_v11_through(max, data)
    }

    fn from_v11_through(max: VersionNumber, data: VersionData) -> VersionTable<VersionData> {
        let values = VersionNumber::SUPPORTED
            .into_iter()
            .filter(|version| *version <= max)
            .map(|version| (version, data.clone()))
            .collect();
        VersionTable { values, unknown: BTreeMap::new() }
    }
}

impl<T: Display + Ord> Display for VersionTable<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut versions: Vec<VersionNumber> =
            self.values.keys().copied().chain(self.unknown.keys().copied()).collect();
        versions.sort();
        versions.dedup();
        for (idx, version) in versions.into_iter().enumerate() {
            if idx > 0 {
                write!(f, ", ")?;
            }
            write!(f, "{}: ", version.as_u64())?;
            if let Some(data) = self.values.get(&version) {
                write!(f, "{data}")?;
            } else if let Some(bytes) = self.unknown.get(&version) {
                write!(f, "{}", cbor_data::Cbor::unchecked(bytes))?;
            }
        }
        Ok(())
    }
}

impl<T> cbor::Encode<()> for VersionTable<T>
where
    T: fmt::Debug + Clone + cbor::Encode<VersionNumber>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        let mut versions: Vec<VersionNumber> =
            self.values.keys().copied().chain(self.unknown.keys().copied()).collect();
        versions.sort();
        versions.dedup();
        e.map(versions.len() as u64)?;

        for version in versions {
            e.encode(version)?;
            if let Some(data) = self.values.get(&version) {
                let mut ctx = version;
                e.encode_with(data, &mut ctx)?;
            } else if let Some(bytes) = self.unknown.get(&version) {
                e.writer_mut().write_all(bytes).map_err(cbor::encode::Error::write)?;
            }
        }

        Ok(())
    }
}

impl<'b, T> cbor::Decode<'b, ()> for VersionTable<T>
where
    T: fmt::Debug + Clone + cbor::Decode<'b, VersionNumber>,
{
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.map()?.ok_or(cbor::decode::Error::message("expected def-length map for versiontable"))?;
        let mut values = BTreeMap::new();
        let mut unknown = BTreeMap::new();

        for _ in 0..len {
            let key: VersionNumber = d.decode()?;
            // Haskell decodes each version's parameters as one opaque CBOR item, then interprets
            // that item with the codec for a version it knows. A version this node does not
            // implement keeps the item bytes so the rest of the table still decodes.
            if key.is_supported() {
                let mut ctx = key;
                let value = d.decode_with(&mut ctx)?;
                values.insert(key, value);
            } else {
                let (_, item) = cbor::tee(d, |decoder| decoder.skip())?;
                unknown.insert(key, item.to_vec());
            }
        }
        Ok(VersionTable { values, unknown })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use amaru_kernel::{
        cbor::{self, encode::Write},
        prop_cbor_roundtrip,
    };
    use proptest::prop_compose;

    use super::*;
    use crate::protocol_messages::{
        version_data::{VersionData, tests::any_version_data},
        version_number::tests::any_version_number,
    };

    prop_cbor_roundtrip!(VersionTable<VersionData>, any_version_table());

    prop_compose! {
        pub fn any_version_table()(values in proptest::collection::btree_map(any_version_number(), any_version_data(), 0..3)) -> VersionTable<VersionData> {
            VersionTable { values, unknown: BTreeMap::new() }
        }
    }

    /// Node-to-node version data as sent by `cardano-cli ping`: initiator-only, peer sharing off, query off.
    ///
    /// Version 16 appends `perasSupport = true`.
    pub(crate) fn n2n_version_data_cbor(version: u64, magic: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = cbor::Encoder::new(&mut bytes);
        encoder.array(if version >= 16 { 5 } else { 4 }).unwrap();
        encoder.u64(magic).unwrap();
        encoder.bool(true).unwrap();
        encoder.u8(0).unwrap();
        encoder.bool(false).unwrap();
        if version >= 16 {
            encoder.bool(true).unwrap();
        }
        bytes
    }

    /// `MsgProposeVersions` for node-to-node versions 14, 15, and 16, matching current `cardano-cli ping`.
    pub(crate) fn haskell_ping_propose(magic: u64) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut encoder = cbor::Encoder::new(&mut bytes);
        encoder.array(2).unwrap();
        encoder.u8(0).unwrap();
        encoder.map(3).unwrap();
        for version in [14, 15, 16] {
            encoder.u64(version).unwrap();
            encoder.writer_mut().write_all(&n2n_version_data_cbor(version, magic)).unwrap();
        }
        bytes
    }

    #[test]
    fn v11_and_above_offers_current_version() {
        let table = VersionTable::v11_and_above(NetworkMagic::PREPROD, true, true);
        assert_eq!(table.values.keys().copied().collect::<Vec<_>>(), VersionNumber::SUPPORTED.to_vec());
        assert_eq!(table.values.keys().next_back().copied(), Some(VersionNumber::CURRENT));
    }

    #[test]
    fn v11_through_v14_excludes_v15() {
        let table = VersionTable::v11_through(VersionNumber::V14, NetworkMagic::PREPROD, false, true);
        assert!(table.values.contains_key(&VersionNumber::V14));
        assert!(!table.values.contains_key(&VersionNumber::V15));
    }
}
