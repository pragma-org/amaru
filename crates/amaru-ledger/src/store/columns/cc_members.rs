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

use amaru_kernel::{cbor, StakeCredential};
use iter_borrow::IterBorrow;

use crate::state::diff_bind::Resettable;

pub const EVENT_TARGET: &str = "amaru::ledger::store::cc_members";

/// Iterator used to browse rows from the CC members column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = Resettable<StakeCredential>;

pub type Key = StakeCredential;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub hot_credential: Option<StakeCredential>,
}

impl Row {
    #[allow(clippy::panic)]
    pub fn unsafe_decode(bytes: Vec<u8>) -> Self {
        cbor::decode(&bytes).unwrap_or_else(|e| {
            panic!(
                "unable to decode account from CBOR ({}): {e:?}",
                hex::encode(&bytes)
            )
        })
    }
}

impl<C> cbor::encode::Encode<C> for Row {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
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
use amaru_kernel::{prop_cbor_roundtrip, Hash};
#[cfg(any(test, feature = "test-utils"))]
use proptest::prelude::*;
#[cfg(any(test, feature = "test-utils"))]
use proptest::{option, prop_compose};

#[cfg(any(test, feature = "test-utils"))]
prop_compose! {
    pub fn any_stake_credential()(
        is_script in any::<bool>(),
        credential in any::<[u8; 28]>(),
    ) -> StakeCredential {
        if is_script {
            StakeCredential::ScriptHash(Hash::from(credential))
        } else {
            StakeCredential::AddrKeyhash(Hash::from(credential))
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
prop_compose! {
    pub fn any_row()(
        hot_credential in option::of(any_stake_credential()),
    ) -> Row {
        Row {
            hot_credential,
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
prop_cbor_roundtrip!(Row, any_row());
