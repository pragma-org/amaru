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

use crate::{
    Hash, Lovelace, Nullable, PoolId, PoolMetadata, RationalNumber, Relay, RewardAccount, Set,
    cbor,
    size::{KEY, VRF_KEY},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolParams {
    pub id: PoolId,
    pub vrf: Hash<VRF_KEY>,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin: RationalNumber,
    pub reward_account: RewardAccount,
    pub owners: Set<Hash<KEY>>,
    pub relays: Vec<Relay>,
    pub metadata: Nullable<PoolMetadata>,
}

impl<C> cbor::encode::Encode<C> for PoolParams {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(9)?;
        e.encode_with(self.id, ctx)?;
        e.encode_with(self.vrf, ctx)?;
        e.encode_with(self.pledge, ctx)?;
        e.encode_with(self.cost, ctx)?;
        e.encode_with(&self.margin, ctx)?;
        e.encode_with(&self.reward_account, ctx)?;
        e.encode_with(&self.owners, ctx)?;
        e.encode_with(&self.relays, ctx)?;
        e.encode_with(&self.metadata, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::decode::Decode<'b, C> for PoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let _len = d.array()?;
        Ok(PoolParams {
            id: d.decode_with(ctx)?,
            vrf: d.decode_with(ctx)?,
            pledge: d.decode_with(ctx)?,
            cost: d.decode_with(ctx)?,
            margin: d.decode_with(ctx)?,
            reward_account: d.decode_with(ctx)?,
            owners: d.decode_with(ctx)?,
            relays: d.decode_with(ctx)?,
            metadata: d.decode_with(ctx)?,
        })
    }
}

impl serde::Serialize for PoolParams {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use pallas_addresses::Address;
        use serde::ser::SerializeStruct;
        use std::collections::BTreeMap;

        fn as_lovelace_map(n: u64) -> BTreeMap<String, BTreeMap<String, u64>> {
            let mut lovelace = BTreeMap::new();
            lovelace.insert("lovelace".to_string(), n);
            let mut ada = BTreeMap::new();
            ada.insert("ada".to_string(), lovelace);
            ada
        }

        fn as_string_ratio(r: &RationalNumber) -> String {
            format!("{}/{}", r.numerator, r.denominator)
        }

        fn as_bech32_addr(bytes: &[u8]) -> Result<String, pallas_addresses::Error> {
            Address::from_bytes(bytes).and_then(|addr| addr.to_bech32())
        }

        struct WrapRelay<'a>(&'a Relay);

        impl serde::Serialize for WrapRelay<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                match self.0 {
                    Relay::SingleHostAddr(port, ipv4, ipv6) => {
                        let mut s = serializer.serialize_struct("Relay::SingleHostAddr", 4)?;
                        s.serialize_field("type", "ipAddress")?;
                        if let Nullable::Some(ipv4) = ipv4 {
                            s.serialize_field(
                                "ipv4",
                                &format!("{}.{}.{}.{}", ipv4[0], ipv4[1], ipv4[2], ipv4[3]),
                            )?;
                        }
                        if let Nullable::Some(ipv6) = ipv6 {
                            let bytes: [u8; 16] = [
                                ipv6[3], ipv6[2], ipv6[1], ipv6[0], // 1st fragment
                                ipv6[7], ipv6[6], ipv6[5], ipv6[4], // 2nd fragment
                                ipv6[11], ipv6[10], ipv6[9], ipv6[8], // 3rd fragment
                                ipv6[15], ipv6[14], ipv6[13], ipv6[12], // 4th fragment
                            ];
                            s.serialize_field(
                                "ipv6",
                                &format!("{}", std::net::Ipv6Addr::from(bytes)),
                            )?;
                        }
                        if let Nullable::Some(port) = port {
                            s.serialize_field("port", port)?;
                        }
                        s.end()
                    }
                    Relay::SingleHostName(port, hostname) => {
                        let mut s = serializer.serialize_struct("Relay::SingleHostName", 3)?;
                        s.serialize_field("type", "hostname")?;
                        s.serialize_field("hostname", hostname)?;
                        if let Nullable::Some(port) = port {
                            s.serialize_field("port", port)?;
                        }
                        s.end()
                    }
                    Relay::MultiHostName(hostname) => {
                        let mut s = serializer.serialize_struct("Relay::MultiHostName", 2)?;
                        s.serialize_field("type", "hostname")?;
                        s.serialize_field("hostname", hostname)?;
                        s.end()
                    }
                }
            }
        }

        let mut s = serializer.serialize_struct("PoolParams", 9)?;
        s.serialize_field("id", &hex::encode(self.id))?;
        s.serialize_field("vrfVerificationKeyHash", &hex::encode(self.vrf))?;
        s.serialize_field("pledge", &as_lovelace_map(self.pledge))?;
        s.serialize_field("cost", &as_lovelace_map(self.cost))?;
        s.serialize_field("margin", &as_string_ratio(&self.margin))?;
        s.serialize_field(
            "rewardAccount",
            &as_bech32_addr(&self.reward_account).map_err(serde::ser::Error::custom)?,
        )?;
        s.serialize_field(
            "owners",
            &self.owners.iter().map(hex::encode).collect::<Vec<String>>(),
        )?;
        s.serialize_field(
            "relays",
            &self
                .relays
                .iter()
                .map(WrapRelay)
                .collect::<Vec<WrapRelay<'_>>>(),
        )?;
        if let Nullable::Some(metadata) = &self.metadata {
            s.serialize_field("metadata", metadata)?;
        }
        s.end()
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::*;
    use crate::{
        Bytes, Hash, Nullable, RationalNumber, Relay, any_hash28, any_hash32, prop_cbor_roundtrip,
        size::{CREDENTIAL, KEY},
    };
    use proptest::{prelude::*, prop_compose};

    prop_cbor_roundtrip!(PoolParams, any_pool_params());

    fn any_nullable_port() -> impl Strategy<Value = Nullable<u32>> {
        prop_oneof![
            Just(Nullable::Undefined),
            Just(Nullable::Null),
            any::<u32>().prop_map(Nullable::Some),
        ]
    }

    fn any_nullable_ipv4() -> impl Strategy<Value = Nullable<Bytes>> {
        prop_oneof![
            Just(Nullable::Undefined),
            Just(Nullable::Null),
            any::<[u8; 4]>().prop_map(|a| Nullable::Some(Vec::from(a).into())),
        ]
    }

    fn any_nullable_ipv6() -> impl Strategy<Value = Nullable<Bytes>> {
        prop_oneof![
            Just(Nullable::Undefined),
            Just(Nullable::Null),
            any::<[u8; 16]>().prop_map(|a| Nullable::Some(Vec::from(a).into())),
        ]
    }

    prop_compose! {
        fn single_host_addr()(
            port in any_nullable_port(),
            ipv4 in any_nullable_ipv4(),
            ipv6 in any_nullable_ipv6()
        ) -> Relay {
            Relay::SingleHostAddr(port, ipv4, ipv6)
        }
    }

    prop_compose! {
        fn single_host_name()(
            port in any_nullable_port(),
            dnsname in any::<String>(),
        ) -> Relay {
            Relay::SingleHostName(port, dnsname)
        }
    }

    prop_compose! {
        fn multi_host_name()(
            dnsname in any::<String>(),
        ) -> Relay {
            Relay::MultiHostName(dnsname)
        }
    }

    fn any_relay() -> BoxedStrategy<Relay> {
        prop_oneof![single_host_addr(), single_host_name(), multi_host_name(),].boxed()
    }

    prop_compose! {
        pub fn any_pool_params()(
            id in any_hash28(),
            vrf in any_hash32(),
            pledge in any::<u64>(),
            cost in any::<u64>(),
            margin in 0..100u64,
            reward_account in any::<[u8; CREDENTIAL]>(),
            owners in any::<Vec<[u8; KEY]>>(),
            relays in proptest::collection::vec(any_relay(), 0..10),
        ) -> PoolParams {
            PoolParams {
                id,
                vrf,
                pledge,
                cost,
                margin: RationalNumber { numerator: margin, denominator: 100 },
                reward_account: [&[0xF0], &reward_account[..]].concat().into(),
                owners: owners.into_iter().map(|h| h.into()).collect::<Vec<Hash<KEY>>>().into(),
                relays,
                metadata: Nullable::Null,
            }
        }
    }
}
