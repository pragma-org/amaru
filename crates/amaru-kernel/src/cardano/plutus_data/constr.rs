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

use std::cmp::Ordering;

use crate::cbor;

// TODO: Constr internal representation.
//
// This type's internal make no sense. This is really just a tag and a value; albeit with a peculiar
// encoding.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Constr<A> {
    pub tag: u64,
    pub any_constructor: Option<u64>,
    pub fields: Vec<A>,
}

impl<A: Ord> PartialOrd for Constr<A> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<A: Ord> Ord for Constr<A> {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.constr_index().cmp(&other.constr_index()) {
            Ordering::Equal => self.fields.cmp(&other.fields),
            ordering @ Ordering::Less | ordering @ Ordering::Greater => ordering,
        }
    }
}

impl<A> Constr<A> {
    #[expect(clippy::panic)]
    pub fn constr_index(&self) -> u64 {
        match self.tag {
            121..=127 => self.tag - 121,
            1280..=1400 => self.tag - 1280 + 7,
            102 => self.any_constructor.unwrap_or_else(|| panic!("malformed Constr: missing 'any_constructor'")),
            tag => panic!("malformed Constr: invalid tag {tag:?}"),
        }
    }
}

impl<'b, C, A> cbor::Decode<'b, C> for Constr<A>
where
    A: cbor::Decode<'b, C>,
{
    fn decode(d: &mut cbor::Decoder<'b>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let tag = d.tag()?;
        let x = tag.as_u64();
        match x {
            121..=127 | 1280..=1400 => Ok(Constr { tag: x, fields: d.decode_with(ctx)?, any_constructor: None }),
            102 => cbor::heterogeneous_array(d, |d, assert_len| {
                assert_len(2)?;
                Ok(Constr { tag: x, any_constructor: Some(d.decode_with(ctx)?), fields: d.decode_with(ctx)? })
            }),
            _ => Err(cbor::decode::Error::message("bad tag code for plutus data")),
        }
    }
}

impl<C, A> cbor::Encode<C> for Constr<A>
where
    A: cbor::Encode<C>,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.tag(cbor::Tag::new(self.tag))?;

        if self.tag == 102 {
            e.array(2)?;
            e.encode_with(self.any_constructor.unwrap_or_default(), ctx)?;
        }

        if self.fields.is_empty() {
            e.array(0)?;
        } else {
            e.begin_array()?;
            for field in &self.fields {
                e.encode_with(field, ctx)?;
            }
            e.end()?;
        }

        Ok(())
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::{prelude::*, strategy::Just};

    use super::Constr;
    use crate::{PlutusData, any_plutus_data, cbor, memoized::VariableEncodingPlutusData, utils::cbor::CborArray};

    // ---------------------------------------------------------------------------------------------
    // Constr
    // ---------------------------------------------------------------------------------------------

    pub fn any_constr(depth: u8) -> impl Strategy<Value = Constr<PlutusData>> {
        let any_constr_tag = prop_oneof![
            (Just(102), any::<u64>().prop_map(Some)),
            (121_u64..=127, Just(None)),
            (1280_u64..=1400, Just(None))
        ];

        let any_fields = prop::collection::vec(any_plutus_data(depth - 1), 0..depth as usize);

        (any_constr_tag, any_fields).prop_map(|((tag, any_constructor), fields)| Constr {
            tag,
            any_constructor,
            fields,
        })
    }

    // ---------------------------------------------------------------------------------------------
    // VariableEncodingConstr
    // ---------------------------------------------------------------------------------------------

    #[derive(Debug, Clone)]
    pub struct VariableEncodingConstr<A> {
        pub tag: u64,
        pub any_constructor: Option<u64>,
        pub fields: CborArray<A>,
    }

    impl VariableEncodingConstr<VariableEncodingPlutusData> {
        pub fn any(depth: u8) -> impl Strategy<Value = Self> {
            let any_constr_tag = prop_oneof![
                (Just(102), any::<u64>().prop_map(Some)),
                (121_u64..=127, Just(None)),
                (1280_u64..=1400, Just(None))
            ];

            let any_fields = prop::collection::vec(VariableEncodingPlutusData::any(depth - 1), 0..depth as usize);

            (any_constr_tag, any_fields, any::<bool>()).prop_map(|((tag, any_constructor), fields, is_def)| Self {
                tag,
                any_constructor,
                fields: if is_def { CborArray::Def(fields) } else { CborArray::Indef(fields) },
            })
        }
    }

    impl<C, A: cbor::Encode<C>> cbor::Encode<C> for VariableEncodingConstr<A> {
        fn encode<W: cbor::encode::Write>(
            &self,
            e: &mut cbor::Encoder<W>,
            ctx: &mut C,
        ) -> Result<(), cbor::encode::Error<W::Error>> {
            e.tag(cbor::Tag::new(self.tag))?;
            match self.tag {
                102 => {
                    let x = (self.any_constructor.unwrap_or_default(), &self.fields);
                    e.encode_with(x, ctx)?;
                }
                _ => {
                    e.encode_with(&self.fields, ctx)?;
                }
            }
            Ok(())
        }
    }
}
