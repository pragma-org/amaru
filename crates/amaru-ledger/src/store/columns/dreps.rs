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
use amaru_kernel::{cbor, Anchor, CertificatePointer, Epoch, Lovelace, StakeCredential};
use iter_borrow::IterBorrow;

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
    pub last_interaction: Epoch,
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
        e.array(4)?;
        e.encode_with(self.deposit, ctx)?;
        e.encode_with(self.anchor.clone(), ctx)?;
        e.encode_with(self.registered_at, ctx)?;
        e.encode_with(self.last_interaction, ctx)?;
        Ok(())
    }
}

impl<'a, C> cbor::decode::Decode<'a, C> for Row {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        d.array()?;
        Ok(Row {
            deposit: d.decode_with(ctx)?,
            anchor: d.decode_with(ctx)?,
            registered_at: d.decode_with(ctx)?,
            last_interaction: d.decode_with(ctx)?,
        })
    }
}

#[cfg(test)]
pub mod test {
    use super::Row;
    use crate::store::columns::tests::any_certificate_pointer;
    use amaru_kernel::{prop_cbor_roundtrip, Anchor, Epoch, Hash, Lovelace};
    use proptest::{option, prelude::*, string};

    prop_cbor_roundtrip!(Row, any_row());

    prop_compose! {
        fn any_row()(
            deposit in any::<Lovelace>(),
            anchor in option::of(any_anchor()),
            registered_at in any_certificate_pointer(),
            last_interaction in any::<Epoch>(),
        ) -> Row {
            Row {
                deposit,
                anchor,
                registered_at,
                last_interaction,
            }
        }
    }

    prop_compose! {
        pub(crate) fn any_anchor()(
            // NOTE: This can be any string really, but it's cute to generate URLs, isn't it?
            url in string::string_regex("(https:)?[a-zA-Z0-9]{2,}(\\.[a-zA-Z0-9]{2,})(\\.[a-zA-Z0-9]{2,})?").unwrap(),
            content_hash in any::<[u8; 32]>(),
        ) -> Anchor {
            Anchor {
                url,
                content_hash: Hash::from(content_hash),
            }
        }

    }
}
