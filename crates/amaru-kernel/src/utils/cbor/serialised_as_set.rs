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

use std::collections::BTreeSet;

use crate::{cbor, data_structures::non_empty_unique_vec::has_duplicate};

/// Decode a tagged set into a collection that is not itself a set type.
///
/// The `#6.258` tag is optional on the wire below protocol version 12, so it is accepted either
/// way; a repeated element is a malformed set and is turned down.
#[derive(Debug)]
#[repr(transparent)]
pub struct SerialisedAsSet<T>(pub T);

const HAS_DUPLICATE: &str = "found duplicate elements in a set";

/// A set kept in a `Vec`, so that a small one does not pay for a `BTreeSet`'s nodes.
/// Duplicated elements are rejected.
impl<'d, C, E: Eq + cbor::Decode<'d, C>> cbor::Decode<'d, C> for SerialisedAsSet<Vec<E>> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        decode_set_tag(d)?;

        let position = d.position();
        let elements: Vec<E> = d.decode_with(ctx)?;

        if has_duplicate(elements.as_slice()) {
            return Err(cbor::decode::Error::message(HAS_DUPLICATE).at(position));
        }

        Ok(Self(elements))
    }
}

/// A set kept in a `BTreeSet`. Duplicated elements are rejected.
impl<'d, C, E: Ord + cbor::Decode<'d, C>> cbor::Decode<'d, C> for SerialisedAsSet<BTreeSet<E>> {
    fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        decode_set_tag(d)?;

        let mut elements = BTreeSet::new();
        match d.array()? {
            // definite form
            Some(len) => {
                for _ in 0..len {
                    insert_element(d, ctx, &mut elements)?;
                }
            }
            // indefinite form
            None => {
                while d.datatype()? != cbor::Type::Break {
                    insert_element(d, ctx, &mut elements)?;
                }
                // skip the break
                d.skip()?;
            }
        }
        Ok(Self(elements))
    }
}

/// Decode one element into `elements` and raise an error for a duplicated element
fn insert_element<'d, C, E: Ord + cbor::Decode<'d, C>>(
    d: &mut cbor::Decoder<'d>,
    ctx: &mut C,
    elements: &mut BTreeSet<E>,
) -> Result<(), cbor::decode::Error> {
    let position = d.position();

    if !elements.insert(d.decode_with(ctx)?) {
        return Err(cbor::decode::Error::message(HAS_DUPLICATE).at(position));
    }

    Ok(())
}

fn decode_set_tag(d: &mut cbor::Decoder<'_>) -> Result<(), cbor::decode::Error> {
    if d.datatype()? == cbor::Type::Tag {
        let tag = d.tag()?;
        if tag != cbor::TAG_SET_258 {
            return Err(cbor::decode::Error::message(format!(
                "invalid set tag; expect={}, found={tag}",
                cbor::TAG_SET_258
            )));
        }
    }

    Ok(())
}
