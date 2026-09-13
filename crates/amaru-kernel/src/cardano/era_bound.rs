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

use std::time::Duration;

#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::{Arbitrary, BoxedStrategy, Just, Strategy, any};

use crate::{Epoch, Slot, cbor, utils::cbor::SerialisedAsPico};

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct EraBound {
    #[serde(deserialize_with = "SerialisedAsPico::deserialize")]
    #[serde(serialize_with = "SerialisedAsPico::serialize")]
    pub time: Duration,
    pub slot: Slot,
    pub epoch: Epoch,
}

impl<C> cbor::Encode<C> for EraBound {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(3)?;
        SerialisedAsPico::from(self.time).encode(e, ctx)?;
        self.slot.encode(e, ctx)?;
        self.epoch.encode(e, ctx)?;
        Ok(())
    }
}

impl<'b, C> cbor::Decode<'b, C> for EraBound {
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        cbor::heterogeneous_array(d, |d, assert_len| {
            assert_len(3)?;
            let time = From::<SerialisedAsPico>::from(d.decode()?);
            let slot = d.decode()?;
            let epoch = d.decode_with(ctx)?;
            Ok(EraBound { time, slot, epoch })
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl Arbitrary for EraBound {
    type Parameters = Option<Epoch>;
    type Strategy = BoxedStrategy<Self>;

    fn arbitrary_with(epoch: Self::Parameters) -> Self::Strategy {
        let any_epoch = match epoch {
            Some(epoch) => Just(epoch).boxed(),
            None => any::<Epoch>().boxed(),
        };

        (any::<u32>(), any::<u32>(), any_epoch)
            .prop_map(|(secs, slot, epoch)| EraBound {
                time: Duration::from_secs(secs as u64),
                slot: Slot::new(slot as u64),
                epoch,
            })
            .boxed()
    }
}

#[cfg(test)]
mod tests {
    use super::EraBound;
    use crate::prop_cbor_roundtrip;

    prop_cbor_roundtrip!(EraBound);
}
