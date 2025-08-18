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

use crate::state::diff_bind::Resettable;
use amaru_kernel::{cbor, Anchor, CertificatePointer, Lovelace, StakeCredential};
use iter_borrow::IterBorrow;
use slot_arithmetic::Epoch;

pub const EVENT_TARGET: &str = "amaru::ledger::store::dreps";

/// Iterator used to browse rows from the DRep column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (
    Resettable<Anchor>,
    Option<(Lovelace, CertificatePointer)>,
    Epoch,
);

pub type Key = StakeCredential;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub deposit: Lovelace,
    pub anchor: Option<Anchor>,
    pub registered_at: CertificatePointer,
    pub last_interaction: Option<Epoch>,
    /// This field is *temporary* and only necessary to re-implement a bug present in the Cardano
    /// ledger in the protocol version 9.
    ///
    /// It is temporary in the sense that, it is no longer required if bootstrapping from snapshots
    /// that starts in protocol version 10 or later; and thus shall be dropped entirely when relevant.
    pub previous_deregistration: Option<CertificatePointer>,
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(5)?;
        e.encode_with(self.deposit, ctx)?;
        e.encode_with(self.anchor.clone(), ctx)?;
        e.encode_with(self.registered_at, ctx)?;
        e.encode_with(self.last_interaction, ctx)?;
        e.encode_with(self.previous_deregistration, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        Ok(Row {
            deposit: d.decode_with(ctx)?,
            anchor: d.decode_with(ctx)?,
            registered_at: d.decode_with(ctx)?,
            last_interaction: d.decode_with(ctx)?,
            previous_deregistration: if len > Some(4) {
                d.decode_with(ctx)?
            } else {
                None
            },
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use amaru_kernel::{
        prop_cbor_roundtrip,
        tests::{any_anchor, any_certificate_pointer},
    };
    use proptest::{option, prelude::*, prop_compose};

    prop_compose! {
        pub fn any_row(max_slot: u64)(
            deposit in any::<Lovelace>(),
            anchor in option::of(any_anchor()),
            registered_at in any_certificate_pointer(max_slot),
            last_interaction in option::of(any::<Epoch>()),
            previous_deregistration in option::of(any_certificate_pointer(max_slot)),
        ) -> Row {
            Row {
                deposit,
                anchor,
                registered_at,
                last_interaction,
                previous_deregistration,
            }
        }
    }

    prop_cbor_roundtrip!(Row, any_row(u64::MAX));
}
