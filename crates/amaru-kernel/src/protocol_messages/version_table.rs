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
    network_magic::NetworkMagic,
    version_data::{PEER_SHARING_DISABLED, VersionData},
    version_number::VersionNumber,
};
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};
use std::{collections::BTreeMap, fmt};

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
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

impl<T> Encode<()> for VersionTable<T>
where
    T: fmt::Debug + Clone + Encode<VersionNumber>,
{
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        e.map(self.values.len() as u64)?;

        for key in self.values.keys() {
            e.encode(key)?;
            let mut ctx = *key;
            e.encode_with(&self.values[key], &mut ctx)?;
        }

        Ok(())
    }
}

impl<'b, T> Decode<'b, ()> for VersionTable<T>
where
    T: fmt::Debug + Clone + Decode<'b, VersionNumber>,
{
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        let len = d.map()?.ok_or(decode::Error::message(
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
