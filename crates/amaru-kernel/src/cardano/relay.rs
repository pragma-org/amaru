// Copyright 2026 PRAGMA
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

use std::{fmt, net::Ipv6Addr};

use serde::ser::SerializeStruct;

use crate::{Bytes, cbor};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Relay {
    SingleHostAddr(Option<u32>, Option<IPv4>, Option<IPv6>),
    SingleHostName(Option<u32>, String),
    MultiHostName(String),
}

type IPv4 = Bytes;
type IPv6 = Bytes;

impl fmt::Display for Relay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MultiHostName(dns) => {
                write!(f, "{dns}")?;
            }

            Self::SingleHostName(port, dns) => {
                write!(f, "{dns}")?;

                if let Some(port) = port {
                    write!(f, ":{}", port)?;
                }
            }

            Self::SingleHostAddr(port, ipv4, ipv6) => {
                if let Some(ipv4) = ipv4 {
                    write!(
                        f,
                        "{}.{}.{}.{}{}",
                        ipv4[0],
                        ipv4[1],
                        ipv4[2],
                        ipv4[3],
                        if let Some(port) = port { format!(":{port}") } else { String::new() }
                    )?;
                }

                if let Some(ipv6) = ipv6 {
                    if ipv4.is_some() {
                        write!(f, "|")?;
                    }

                    write!(
                        f,
                        "{}{}",
                        Ipv6Addr::from([
                            ipv6[3], ipv6[2], ipv6[1], ipv6[0], // group 1
                            ipv6[7], ipv6[6], ipv6[5], ipv6[4], // group 2
                            ipv6[11], ipv6[10], ipv6[9], ipv6[8], // group 3
                            ipv6[15], ipv6[14], ipv6[13], ipv6[12], // group 4
                        ]),
                        if let Some(port) = port { format!(":{port}") } else { String::new() }
                    )?;
                }
            }
        }

        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for Relay {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        let variant = d.u16()?;
        match variant {
            0 => Ok(Self::SingleHostAddr(d.decode_with(ctx)?, d.decode_with(ctx)?, d.decode_with(ctx)?)),
            1 => Ok(Self::SingleHostName(d.decode_with(ctx)?, d.decode_with(ctx)?)),
            2 => Ok(Self::MultiHostName(d.decode_with(ctx)?)),
            _ => Err(cbor::decode::Error::message("invalid variant id for Relay")),
        }
    }
}

impl<C> cbor::encode::Encode<C> for Relay {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Self::SingleHostAddr(a, b, c) => {
                e.array(4)?;
                e.encode_with(0, ctx)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                e.encode_with(c, ctx)?;
                Ok(())
            }
            Self::SingleHostName(a, b) => {
                e.array(3)?;
                e.encode_with(1, ctx)?;
                e.encode_with(a, ctx)?;
                e.encode_with(b, ctx)?;
                Ok(())
            }
            Self::MultiHostName(a) => {
                e.array(2)?;
                e.encode_with(2, ctx)?;
                e.encode_with(a, ctx)?;
                Ok(())
            }
        }
    }
}

#[derive(serde::Serialize)]
#[serde(transparent)]
pub struct AsJson<'a>(#[serde(serialize_with = "serialize")] pub &'a Relay);

pub fn serialize<S: serde::Serializer>(relay: &Relay, serializer: S) -> Result<S::Ok, S::Error> {
    match relay {
        Relay::SingleHostAddr(port, ipv4, ipv6) => {
            let mut s = serializer.serialize_struct("Relay::SingleHostAddr", 4)?;
            // NOTE: keep fields in lexicographic order
            //
            // This instance is used for canonical ledger state comparisons.
            if let Some(ipv4) = ipv4 {
                s.serialize_field("ipv4", &format!("{}.{}.{}.{}", ipv4[0], ipv4[1], ipv4[2], ipv4[3]))?;
            }
            if let Some(ipv6) = ipv6 {
                let bytes: [u8; 16] = [
                    ipv6[3], ipv6[2], ipv6[1], ipv6[0], // 1st fragment
                    ipv6[7], ipv6[6], ipv6[5], ipv6[4], // 2nd fragment
                    ipv6[11], ipv6[10], ipv6[9], ipv6[8], // 3rd fragment
                    ipv6[15], ipv6[14], ipv6[13], ipv6[12], // 4th fragment
                ];
                s.serialize_field("ipv6", &format!("{}", std::net::Ipv6Addr::from(bytes)))?;
            }
            if let Some(port) = port {
                s.serialize_field("port", port)?;
            }
            s.serialize_field("type", "ip_address")?;
            s.end()
        }
        Relay::SingleHostName(port, hostname) => {
            let mut s = serializer.serialize_struct("Relay::SingleHostName", 3)?;
            // NOTE: keep fields in lexicographic order
            //
            // This instance is used for canonical ledger state comparisons.
            s.serialize_field("hostname", hostname)?;
            if let Some(port) = port {
                s.serialize_field("port", port)?;
            }
            s.serialize_field("type", "hostname")?;
            s.end()
        }
        Relay::MultiHostName(hostname) => {
            let mut s = serializer.serialize_struct("Relay::MultiHostName", 2)?;
            // NOTE: keep fields in lexicographic order
            //
            // This instance is used for canonical ledger state comparisons.
            s.serialize_field("hostname", hostname)?;
            s.serialize_field("type", "hostname")?;
            s.end()
        }
    }
}
