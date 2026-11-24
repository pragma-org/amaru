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

use std::{
    fmt,
    net::{Ipv4Addr, Ipv6Addr},
};

#[cfg(any(test, feature = "test-utils"))]
use proptest::{
    option,
    prelude::{Arbitrary, BoxedStrategy, Strategy, any, prop_oneof},
};
use serde::ser::SerializeStruct;

use crate::{MaxString128, cardano::fixed_bytes::FixedBytes, cbor};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Relay {
    SingleHostAddr(Option<Port>, Option<IPv4>, Option<IPv6>),
    SingleHostName(Option<Port>, MaxString128),
    MultiHostName(MaxString128),
}

/// A TCP port.
pub type Port = u16;

/// An IPv4 address stored in network byte order, as in the ledger encoding.
pub type IPv4 = FixedBytes<4>;
/// An IPv6 address stored as four little-endian 32-bit words, as in the ledger encoding.
pub type IPv6 = FixedBytes<16>;

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(remote = "Relay", rename = "Relay")]
enum RelaySerde {
    SingleHostAddr(Option<Port>, Option<IPv4>, Option<IPv6>),
    SingleHostName(Option<Port>, MaxString128),
    MultiHostName(MaxString128),
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct IpRelay {
    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "deserialize_port")]
    port: Option<Port>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_ipv4",
        deserialize_with = "deserialize_ipv4"
    )]
    ipv4: Option<IPv4>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_ipv6",
        deserialize_with = "deserialize_ipv6"
    )]
    ipv6: Option<IPv6>,
}

impl serde::Serialize for Relay {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable()
            && let Self::SingleHostAddr(port, ipv4, ipv6) = self
        {
            return IpRelay { port: *port, ipv4: *ipv4, ipv6: *ipv6 }.serialize(serializer);
        }
        RelaySerde::serialize(self, serializer)
    }
}

impl<'de> serde::Deserialize<'de> for Relay {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        if !deserializer.is_human_readable() {
            return RelaySerde::deserialize(deserializer);
        }

        #[derive(serde::Deserialize)]
        enum RelayJson {
            SingleHostName(Option<Port>, MaxString128),
            MultiHostName(MaxString128),
            #[serde(untagged)]
            SingleHostAddr(IpRelay),
        }

        Ok(match RelayJson::deserialize(deserializer)? {
            RelayJson::SingleHostAddr(IpRelay { port, ipv4, ipv6 }) => Self::SingleHostAddr(port, ipv4, ipv6),
            RelayJson::SingleHostName(port, hostname) => Self::SingleHostName(port, hostname),
            RelayJson::MultiHostName(hostname) => Self::MultiHostName(hostname),
        })
    }
}

fn deserialize_port<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<Port>, D::Error> {
    <Port as serde::Deserialize>::deserialize(deserializer).map(Some)
}

fn deserialize_ipv4<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<IPv4>, D::Error> {
    let address: Ipv4Addr = serde::Deserialize::deserialize(deserializer)?;
    Ok(Some(address.octets().into()))
}

fn serialize_ipv4<S: serde::Serializer>(address: &Option<IPv4>, serializer: S) -> Result<S::Ok, S::Error> {
    let address = address.as_ref().map(ipv4_from_ledger_bytes);
    serde::Serialize::serialize(&address, serializer)
}

fn ipv4_from_ledger_bytes(address: &IPv4) -> Ipv4Addr {
    Ipv4Addr::from_octets(*address.as_array())
}

fn deserialize_ipv6<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Option<IPv6>, D::Error> {
    let address: Ipv6Addr = serde::Deserialize::deserialize(deserializer)?;
    let mut bytes = address.octets();
    for word in bytes.as_chunks_mut::<4>().0 {
        word.reverse();
    }
    Ok(Some(bytes.into()))
}

fn serialize_ipv6<S: serde::Serializer>(address: &Option<IPv6>, serializer: S) -> Result<S::Ok, S::Error> {
    let address = address.map(|ip| {
        let mut bytes = *ip.as_array();
        for word in bytes.as_chunks_mut::<4>().0 {
            word.reverse();
        }
        Ipv6Addr::from(bytes)
    });
    serde::Serialize::serialize(&address, serializer)
}

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
                        "{}{}",
                        ipv4_from_ledger_bytes(ipv4),
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

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for Relay {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            let variant = d.u16()?;
            match variant {
                0 => {
                    assert_len(4)?;
                    Ok(Self::SingleHostAddr(d.decode_with(ctx)?, d.decode_with(ctx)?, d.decode_with(ctx)?))
                }
                1 => {
                    assert_len(3)?;
                    Ok(Self::SingleHostName(d.decode_with(ctx)?, d.decode_with(ctx)?))
                }
                2 => {
                    assert_len(2)?;
                    Ok(Self::MultiHostName(d.decode_with(ctx)?))
                }
                _ => Err(cbor::decode::Error::message("invalid variant id for Relay")),
            }
        })
    }
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for Relay {
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
                s.serialize_field("ipv4", &ipv4_from_ledger_bytes(ipv4).to_string())?;
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

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for Relay {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    #[expect(clippy::unwrap_used)]
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        let any_port = || option::of(any::<Port>());
        let any_dns_name = || {
            any::<String>().prop_filter_map("DNS name must fit MaxString128", |name| MaxString128::try_from(name).ok())
        };
        let any_ipv4 = option::of(any::<[u8; 4]>().prop_map(|octets| IPv4::try_from(Vec::from(octets)).unwrap()));
        let any_ipv6 = option::of(any::<[u8; 16]>().prop_map(|octets| IPv6::try_from(Vec::from(octets)).unwrap()));

        let single_host_addr =
            (any_port(), any_ipv4, any_ipv6).prop_map(|(port, ipv4, ipv6)| Relay::SingleHostAddr(port, ipv4, ipv6));
        let single_host_name =
            (any_port(), any_dns_name()).prop_map(|(port, dns_name)| Relay::SingleHostName(port, dns_name));
        let multi_host_name = any_dns_name().prop_map(Relay::MultiHostName);

        prop_oneof![single_host_addr, single_host_name, multi_host_name].boxed()
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::{AsJson, Relay};

    #[test]
    fn dual_address_relay_preserves_ledger_encoding() {
        let cbor = hex::decode("8400190bb944c000020150d041012040ce08000000000001000000").unwrap();
        let relay = crate::cbor::from_cbor::<Relay>(&cbor).unwrap();
        let json = json!({
            "port": 3001, "ipv4": "192.0.2.1", "ipv6": "2001:41d0:8:ce40::1"
        });
        assert_eq!(serde_json::to_value(&relay).unwrap(), json);
        assert_eq!(relay.to_string(), "192.0.2.1:3001|2001:41d0:8:ce40::1:3001");
        assert_eq!(serde_json::to_value(AsJson(&relay)).unwrap()["ipv4"], json["ipv4"]);
        let from_json: Relay = serde_json::from_value(json).unwrap();
        assert_eq!(from_json, relay);
        assert_eq!(crate::cbor::to_cbor(&from_json), cbor);
    }

    #[test]
    fn single_address_json_omits_absent_family() {
        for json in [json!({"port": 3001, "ipv4": "192.0.2.1"}), json!({"port": 3001, "ipv6": "2001:41d0:8:ce40::1"})] {
            let relay: Relay = serde_json::from_value(json.clone()).unwrap();
            assert_eq!(serde_json::to_value(relay).unwrap(), json);
        }
    }

    #[test]
    fn ip_relay_json_rejects_unsupported_formats() {
        for json in [
            json!({"SingleHostAddr": [3001, "192.0.2.1", "2001:41d0:8:ce40::1"]}),
            json!({"port": 3001, "ipv4": [192, 0, 2, 1]}),
            json!({"port": 3001, "ipv6": "d041012040ce08000000000001000000"}),
            json!({"port": 3001, "ipv4": null, "ipv6": "::1"}),
            json!({"port": 3001, "ipv4": "192.0.2.1", "ipv6": null}),
            json!({"port": null, "ipv4": "192.0.2.1"}),
            json!({"port": 3001, "ipv4": "192.0.2.1", "ip6": "::1"}),
        ] {
            assert!(serde_json::from_value::<Relay>(json.clone()).is_err(), "unexpectedly accepted {json}");
        }
    }
}
