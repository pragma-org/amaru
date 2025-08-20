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
use amaru_kernel::{cbor, StakeCredential};

use crate::state::diff_bind::Resettable;

pub const EVENT_TARGET: &str = "amaru::ledger::store::cc_members";

/// Iterator used to browse rows from the CC members column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = Resettable<StakeCredential>;

pub type Key = StakeCredential;

#[derive(Debug, Default, Clone, PartialEq)]
pub struct Row {
    pub hot_credential: Option<StakeCredential>,
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(1)?;
        e.encode_with(self.hot_credential.clone(), ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            hot_credential: d.decode_with(ctx)?,
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::*;
    use crate::store::columns::accounts::tests::any_stake_credential;
    use amaru_kernel::prop_cbor_roundtrip;
    use proptest::{option, prop_compose};

    prop_compose! {
        pub fn any_row()(
            hot_credential in option::of(any_stake_credential()),
        ) -> Row {
            Row {
                hot_credential,
            }
        }
    }

    prop_cbor_roundtrip!(Row, any_row());
}
