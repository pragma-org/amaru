// Copyright 2024 PRAGMA
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

/// This modules captures blocks made by slot leaders throughout epochs.
use amaru_kernel::{
    cbor, {PoolId, Slot},
};
use iter_borrow::IterBorrow;

pub type Key = Slot;

pub type Value = Row;

/// Iterator used to browse rows from the Pools column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Value>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub slot_leader: PoolId,
}

impl Row {
    pub fn new(slot_leader: PoolId) -> Self {
        Self { slot_leader }
    }
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.encode_with(self.slot_leader, ctx)?;
        e.end()?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let slot_leader = d.decode_with(ctx)?;
        Ok(Row::new(slot_leader))
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use amaru_kernel::{prop_cbor_roundtrip, PoolId, Slot};
    use proptest::{prelude::*, prop_compose};

    prop_compose! {
        pub fn any_slot()(
            n in 0u64..87782400,
        ) -> Slot {
            Slot::from(n)
        }
    }

    prop_compose! {
        pub fn any_row()(
            slot_leader in any::<[u8; 28]>().prop_map(PoolId::new),
        ) -> Row {
            Row::new(slot_leader)
        }
    }

    prop_cbor_roundtrip!(prop_cbor_roundtrip_row, Row, any_row());
}
