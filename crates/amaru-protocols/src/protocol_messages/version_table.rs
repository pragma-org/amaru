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

use crate::protocol_messages::{
    version_data::{PEER_SHARING_DISABLED, VersionData},
    version_number::VersionNumber,
};
use amaru_kernel::{NetworkMagic, cbor};
use std::fmt::{Debug, Display};
use std::{collections::BTreeMap, fmt};

#[derive(Debug, PartialEq, Eq, Clone, PartialOrd, Ord, serde::Serialize, serde::Deserialize)]
pub struct VersionTable<T>
where
    T: fmt::Debug + Clone,
{
    pub values: BTreeMap<VersionNumber, T>,
}

impl VersionTable<VersionData> {
    pub fn empty() -> VersionTable<VersionData> {
        VersionTable {
            values: BTreeMap::new(),
        }
    }

    pub fn query(network_magic: NetworkMagic) -> VersionTable<VersionData> {
        VersionTable {
            values: vec![
                (
                    VersionNumber::V11,
                    VersionData::new(network_magic, false, PEER_SHARING_DISABLED, true),
                ),
                (
                    VersionNumber::V12,
                    VersionData::new(network_magic, false, PEER_SHARING_DISABLED, true),
                ),
                (
                    VersionNumber::V13,
                    VersionData::new(network_magic, false, PEER_SHARING_DISABLED, true),
                ),
                (
                    VersionNumber::V14,
                    VersionData::new(network_magic, false, PEER_SHARING_DISABLED, true),
                ),
            ]
            .into_iter()
            .collect::<BTreeMap<VersionNumber, VersionData>>(),
        }
    }

    pub fn v11_and_above(
        network_magic: NetworkMagic,
        initiator_only_diffusion_mode: bool,
    ) -> VersionTable<VersionData> {
        let values = vec![
            (
                VersionNumber::V11,
                VersionData::new(
                    network_magic,
                    initiator_only_diffusion_mode,
                    PEER_SHARING_DISABLED,
                    false,
                ),
            ),
            (
                VersionNumber::V12,
                VersionData::new(
                    network_magic,
                    initiator_only_diffusion_mode,
                    PEER_SHARING_DISABLED,
                    false,
                ),
            ),
            (
                VersionNumber::V13,
                VersionData::new(
                    network_magic,
                    initiator_only_diffusion_mode,
                    PEER_SHARING_DISABLED,
                    false,
                ),
            ),
            (
                VersionNumber::V14,
                VersionData::new(
                    network_magic,
                    initiator_only_diffusion_mode,
                    PEER_SHARING_DISABLED,
                    false,
                ),
            ),
        ]
        .into_iter()
        .collect::<BTreeMap<VersionNumber, VersionData>>();

        VersionTable { values }
    }
}

impl<T: Clone + Debug + Display> Display for VersionTable<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let mut entries = self
            .values
            .iter()
            .map(|(version, data)| format!("{}: {}", version.as_u64(), data))
            .collect::<Vec<String>>();

        entries.sort();
        write!(f, "{{{}}}", entries.join(", "))
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
        e.map(self.values.len() as u64)?;

        for key in self.values.keys() {
            e.encode(key)?;
            let mut ctx = *key;
            e.encode_with(&self.values[key], &mut ctx)?;
        }

        Ok(())
    }
}

impl<'b, T> cbor::Decode<'b, ()> for VersionTable<T>
where
    T: fmt::Debug + Clone + cbor::Decode<'b, VersionNumber>,
{
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.map()?.ok_or(cbor::decode::Error::message(
            "expected def-length map for versiontable",
        ))?;
        let mut values = BTreeMap::new();

        for _ in 0..len {
            let key = d.decode()?;
            let mut ctx = key;
            let value = d.decode_with(&mut ctx)?;
            values.insert(key, value);
        }
        Ok(VersionTable { values })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::protocol_messages::{
        version_data::{VersionData, tests::any_version_data},
        version_number::tests::any_version_number,
    };
    use amaru_kernel::prop_cbor_roundtrip;
    use proptest::prop_compose;

    prop_cbor_roundtrip!(VersionTable<VersionData>, any_version_table());

    prop_compose! {
        pub fn any_version_table()(values in proptest::collection::btree_map(any_version_number(), any_version_data(), 0..3)) -> VersionTable<VersionData> {
            VersionTable { values }
        }
    }
}
