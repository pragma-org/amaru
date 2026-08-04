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
    Hash, Lovelace, PoolId, PoolMetadata, RationalNumber, Relay, RewardAccount, cbor,
    size::{KEY, VRF_KEY},
    utils::cbor::SerialisedAsSet,
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolParams {
    pub id: PoolId,
    pub vrf: Hash<VRF_KEY>,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin: RationalNumber,
    pub reward_account: RewardAccount,
    // NOTE: Small set too small for BTreeSet
    //
    // A BTreeSet allocates in nodes of ~400 bytes which can contain multiple elements. So when a
    // set would typically be small; a BTreeSet can easily kill us memory-wise; especially when
    // found in an object that gets reproduced many many times. Using a BTreeSet makes every pool
    // params 400 bytes bigger; even if most will have a single owner.
    //
    // Plus, if the set is small anyway, doing a binary search to find elements is cheap.
    //
    // Here, nothing guarantees that the set is small but we know that at worse, it cannot contain
    // much more than 500 elements due to the transaction max size.
    pub owners: Vec<Hash<KEY>>,
    pub relays: Vec<Relay>,
    pub metadata: Option<PoolMetadata>,
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
        e.encode_with(self.margin, ctx)?;
        e.encode_with(&self.reward_account, ctx)?;
        e.encode_with(SerialisedAsSet(&self.owners), ctx)?;
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
            owners: d.decode_with::<_, SerialisedAsSet<_>>(ctx)?.0,
            relays: d.decode_with(ctx)?,
            metadata: d.decode_with(ctx)?,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{option, prelude::*, prop_compose};

    use super::*;
    use crate::{Bytes, RationalNumber, Relay, any_hash28, any_hash32, prop_cbor_roundtrip, size::CREDENTIAL};

    prop_cbor_roundtrip!(PoolParams, any_pool_params());

    fn any_optional_port() -> impl Strategy<Value = Option<u32>> {
        option::of(any::<u32>())
    }

    fn any_optional_ipv4() -> impl Strategy<Value = Option<Bytes>> {
        option::of(any::<[u8; 4]>().prop_map(|a| Vec::from(a).into()))
    }

    fn any_optional_ipv6() -> impl Strategy<Value = Option<Bytes>> {
        option::of(any::<[u8; 16]>().prop_map(|a| Vec::from(a).into()))
    }

    prop_compose! {
        fn single_host_addr()(
            port in any_optional_port(),
            ipv4 in any_optional_ipv4(),
            ipv6 in any_optional_ipv6()
        ) -> Relay {
            Relay::SingleHostAddr(port, ipv4, ipv6)
        }
    }

    prop_compose! {
        fn single_host_name()(
            port in any_optional_port(),
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
            owners in proptest::collection::vec(any_hash28(), 1..3),
            relays in proptest::collection::vec(any_relay(), 0..10),
        ) -> PoolParams {
            PoolParams {
                id,
                vrf,
                pledge,
                cost,
                margin: RationalNumber { numerator: margin, denominator: 100 },
                reward_account: [&[0xF0], &reward_account[..]].concat().into(),
                owners,
                relays,
                metadata: None,
            }
        }
    }
}
