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

#[cfg(any(test, feature = "test-utils"))]
use proptest::{
    collection,
    prelude::{Arbitrary, BoxedStrategy, Strategy, any},
};

use crate::{
    Hash, Lovelace, PoolId, PoolMetadata, Relay, RewardAccount, UnitRationalNumber, VecSet, cbor,
    size::{KEY, VRF_KEY},
};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PoolParams {
    pub id: PoolId,
    pub vrf: Hash<VRF_KEY>,
    pub pledge: Lovelace,
    pub cost: Lovelace,
    pub margin: UnitRationalNumber,
    pub reward_account: RewardAccount,
    pub owners: VecSet<Hash<KEY>>,
    pub relays: Vec<Relay>,
    pub metadata: Option<PoolMetadata>,
}

impl<C: cbor::HasProtocolVersion> cbor::encode::Encode<C> for PoolParams {
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
        e.encode_with(self.reward_account, ctx)?;
        e.encode_with(&self.owners, ctx)?;
        e.encode_with(&self.relays, ctx)?;
        e.encode_with(&self.metadata, ctx)?;
        Ok(())
    }
}

impl<'b, C: cbor::HasProtocolVersion> cbor::decode::Decode<'b, C> for PoolParams {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(9)?;
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
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for PoolParams {
    type Parameters = ();
    type Strategy = BoxedStrategy<Self>;

    #[expect(clippy::expect_used)]
    fn arbitrary_with(_: Self::Parameters) -> Self::Strategy {
        (
            any::<PoolId>(),
            any::<Hash<VRF_KEY>>(),
            any::<u64>(),
            any::<u64>(),
            0..100u64,
            any::<RewardAccount>(),
            collection::btree_set(any::<Hash<KEY>>(), 1..3),
            collection::vec(any::<Relay>(), 0..10),
        )
            .prop_map(|(id, vrf, pledge, cost, margin, reward_account, owners, relays)| PoolParams {
                id,
                vrf,
                pledge,
                cost,
                margin: UnitRationalNumber::new(margin, 100).expect("margin is always less than 100"),
                reward_account,
                owners: VecSet::from(owners),
                relays,
                metadata: None,
            })
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use crate::{PoolParams, prop_cbor_roundtrip};

    prop_cbor_roundtrip!(PoolParams);
}
