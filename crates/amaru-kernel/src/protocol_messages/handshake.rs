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

use crate::protocol_messages::{version_data::VersionData, version_number::VersionNumber};
use minicbor::{Decode, Decoder, Encode, Encoder, decode, encode};

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum HandshakeResult {
    Accepted(VersionNumber, VersionData),
    Refused(RefuseReason),
}

#[derive(Debug, PartialEq, Eq, Clone, serde::Serialize, serde::Deserialize)]
pub enum RefuseReason {
    VersionMismatch(Vec<VersionNumber>),
    HandshakeDecodeError(VersionNumber, String),
    Refused(VersionNumber, String),
}

impl Encode<()> for RefuseReason {
    fn encode<W: encode::Write>(
        &self,
        e: &mut Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), encode::Error<W::Error>> {
        match self {
            RefuseReason::VersionMismatch(versions) => {
                e.array(2)?;
                e.u16(0)?;
                e.array(versions.len() as u64)?;
                for v in versions.iter() {
                    e.encode(v)?;
                }

                Ok(())
            }
            RefuseReason::HandshakeDecodeError(version, msg) => {
                e.array(3)?;
                e.u16(1)?;
                e.encode(version)?;
                e.str(msg)?;

                Ok(())
            }
            RefuseReason::Refused(version, msg) => {
                e.array(3)?;
                e.u16(2)?;
                e.encode(version)?;
                e.str(msg)?;

                Ok(())
            }
        }
    }
}

impl<'b> Decode<'b, ()> for RefuseReason {
    fn decode(d: &mut Decoder<'b>, _ctx: &mut ()) -> Result<Self, decode::Error> {
        d.array()?;

        match d.u16()? {
            0 => {
                let versions = d.array_iter::<VersionNumber>()?;
                let versions = versions.collect::<Result<_, _>>()?;
                Ok(RefuseReason::VersionMismatch(versions))
            }
            1 => {
                let version = d.decode()?;
                let msg = d.str()?;

                Ok(RefuseReason::HandshakeDecodeError(version, msg.to_string()))
            }
            2 => {
                let version = d.decode()?;
                let msg = d.str()?;

                Ok(RefuseReason::Refused(version, msg.to_string()))
            }
            _ => Err(decode::Error::message("unknown variant for refusereason")),
        }
    }
}
