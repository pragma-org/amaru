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

use amaru_iter_borrow::IterBorrow;
use amaru_kernel::{CertificatePointer, DRepRegistration, Epoch, Lovelace, StakeCredential, cbor};

pub const EVENT_TARGET: &str = "amaru::ledger::store::dreps";

/// Iterator used to browse rows from the DRep column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = Option<DRepRegistration>;

pub type Key = StakeCredential;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub deposit: Lovelace,
    pub registered_at: CertificatePointer,
    pub valid_until: Epoch,

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
        e.encode_with(self.registered_at, ctx)?;
        e.encode_with(self.valid_until, ctx)?;
        e.encode_with(self.previous_deregistration, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            deposit: d.decode_with(ctx)?,
            registered_at: d.decode_with(ctx)?,
            valid_until: d.decode_with(ctx)?,
            previous_deregistration: d.decode_with(ctx)?,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use amaru_kernel::{prop_cbor_roundtrip, tests::any_certificate_pointer};
    use proptest::{option, prelude::*, prop_compose};

    prop_cbor_roundtrip!(Row, any_row(u64::MAX));

    prop_compose! {
        pub fn any_row(max_slot: u64)(
            deposit in any::<Lovelace>(),
            registered_at in any_certificate_pointer(max_slot),
            valid_until in any::<Epoch>(),
            previous_deregistration in option::of(any_certificate_pointer(max_slot)),
        ) -> Row {
            Row {
                deposit,
                registered_at,
                valid_until,
                previous_deregistration,
            }
        }
    }
}
