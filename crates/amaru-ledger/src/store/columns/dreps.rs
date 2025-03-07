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

use amaru_kernel::{cbor, Anchor, Epoch, Lovelace, StakeCredential};
use iter_borrow::IterBorrow;

pub const EVENT_TARGET: &str = "amaru::ledger::store::dreps";

/// Iterator used to browse rows from the Accounts column. Meant to be referenced using qualified imports.
pub type Iter<'a, 'b> = IterBorrow<'a, 'b, Key, Option<Row>>;

pub type Value = (Option<Anchor>, Option<Lovelace>, Epoch);

pub type Key = StakeCredential;

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Row {
    pub deposit: Lovelace,
    pub anchor: Option<Anchor>,
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
        e.array(3)?;
        e.encode_with(self.deposit, ctx)?;
        e.encode_with(self.anchor.clone(), ctx)?;
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
            last_interaction: d.decode_with(ctx)?,
        })
    }
}

#[cfg(test)]
pub(crate) mod test {
    use super::Row;
    use amaru_kernel::{from_cbor, to_cbor, Anchor, Epoch, Hash, Lovelace};
    use proptest::{option, prelude::*, string};

    proptest! {
        #[test]
        fn prop_row_roundtrip_cbor(row in any_row()) {
            let bytes = to_cbor(&row);
            assert_eq!(Some(row), from_cbor::<Row>(&bytes))
        }
    }

    prop_compose! {
        fn any_row()(
            deposit in any::<Lovelace>(),
            anchor in option::of(any_anchor()),
            last_interaction in any::<Epoch>(),
        ) -> Row {
            Row {
                deposit,
                anchor,
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
