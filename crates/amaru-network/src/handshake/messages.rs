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

use amaru_kernel::protocol_messages::{
    handshake::RefuseReason,
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

#[derive(Debug, Clone)]
pub enum Message<D>
where
    D: fmt::Debug + Clone,
{
    Propose(VersionTable<D>),
    Accept(VersionNumber, D),
    Refuse(RefuseReason),
}

impl<D> Encode<()> for Message<D>
where
    D: fmt::Debug + Clone + Encode<VersionNumber>,
    VersionTable<D>: Encode<()>,
{
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            Message::Propose(version_table) => {
                e.array(2)?.u16(0)?;
                e.encode(version_table)?;
            }
            Message::Accept(version_number, version_data) => {
                e.array(3)?.u16(1)?;
                e.encode(version_number)?;
                let mut ctx = *version_number;
                e.encode_with(version_data, &mut ctx)?;
            }
            Message::Refuse(reason) => {
                e.array(2)?.u16(2)?;
                e.encode(reason)?;
            }
        };

        Ok(())
    }
}

impl<'b, D> Decode<'b, ()> for Message<D>
where
    D: Decode<'b, VersionNumber> + fmt::Debug + Clone,
    VersionTable<D>: Decode<'b, ()>,
{
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        d.array()?;

        match d.u16()? {
            0 => {
                let version_table = d.decode()?;
                Ok(Message::Propose(version_table))
            }
            1 => {
                let version_number = d.decode()?;
                let mut ctx = version_number;
                let version_data = d.decode_with(&mut ctx)?;
                Ok(Message::Accept(version_number, version_data))
            }
            2 => {
                let reason: RefuseReason = d.decode()?;
                Ok(Message::Refuse(reason))
            }
            n => Err(decode::Error::message(format!(
                "unknown variant for handshake message: {}",
                n,
            ))),
        }
    }
}
