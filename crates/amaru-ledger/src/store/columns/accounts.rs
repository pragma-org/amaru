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

use crate::state::diff_bind::Resettable;
use amaru_iter_borrow::IterBorrow;
use amaru_kernel::{cbor, CertificatePointer, DRep, Lovelace, PoolId, StakeCredential};

pub const EVENT_TARGET: &str = "amaru::ledger::store::accounts";

/// Iterator used to browse rows from the Accounts column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (
    Resettable<PoolId>,
    Resettable<(DRep, CertificatePointer)>,
    Option<Lovelace>,
    Lovelace,
);

pub type Key = StakeCredential;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub delegatee: Option<PoolId>,
    pub deposit: Lovelace,
    pub drep: Option<(DRep, CertificatePointer)>,
    // FIXME: We probably want to use an arbitrarily-sized for rewards; Going
    // for a Lovelace (aliasing u64) for now as we are only demonstrating the
    // ledger-state storage capabilities and it doesn't *fundamentally* change
    // anything.
    pub rewards: Lovelace,
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(4)?;
        e.encode_with(self.delegatee, ctx)?;
        e.encode_with(self.deposit, ctx)?;
        e.encode_with(self.drep.as_ref(), ctx)?;
        e.encode_with(self.rewards, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            delegatee: d.decode_with(ctx)?,
            deposit: d.decode_with(ctx)?,
            drep: d.decode_with(ctx)?,
            rewards: d.decode_with(ctx)?,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::Row;
    use amaru_kernel::{
        prop_cbor_roundtrip,
        tests::{any_certificate_pointer, any_drep, any_pool_id},
        Lovelace,
    };
    use proptest::{option, prelude::*, prop_compose};

    prop_compose! {
        pub fn any_row(max_slot: u64)(
            delegatee in option::of(any_pool_id()),
            deposit in any::<Lovelace>(),
            drep in option::of(any_drep()),
            drep_registered_at in any_certificate_pointer(max_slot),
            rewards in any::<Lovelace>(),
        ) -> Row {
            Row {
                delegatee,
                deposit,
                drep: drep.map(|drep| (drep, drep_registered_at)),
                rewards,
            }
        }
    }

    prop_cbor_roundtrip!(Row, any_row(u64::MAX));
}
