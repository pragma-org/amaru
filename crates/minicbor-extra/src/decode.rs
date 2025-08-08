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

use crate::cbor;
use std::fmt::Display;

// Misc
// ----------------------------------------------------------------------------

fn decode_break<'d>(
    d: &mut cbor::Decoder<'d>,
    len: Option<u64>,
) -> Result<bool, cbor::decode::Error> {
    if d.datatype()? == cbor::data::Type::Break {
        // NOTE: If we encounter a rogue Break while decoding a definite map, that's an error.
        if len.is_some() {
            return Err(cbor::decode::Error::type_mismatch(cbor::data::Type::Break));
        }

        d.skip()?;

        return Ok(true);
    }

    Ok(false)
}

// Array
// ----------------------------------------------------------------------------

/// Decode any heterogenous CBOR array, irrespective of whether they're indefinite or definite.
pub fn heterogenous_array<'d, A>(
    d: &mut cbor::Decoder<'d>,
    expected_len: u64,
    elems: impl FnOnce(&mut cbor::Decoder<'d>) -> Result<A, cbor::decode::Error>,
) -> Result<A, cbor::decode::Error> {
    let len = d.array()?;

    let result = elems(d)?;

    match len {
        None => {
            decode_break(d, len)?;
        }
        Some(len) if len != expected_len => {
            return Err(cbor::decode::Error::message(format!(
                "array length mismatch: expected {} got {}",
                expected_len, len
            )));
        }
        Some(_len) => (),
    }

    Ok(result)
}

// Map
// ----------------------------------------------------------------------------

/// Decode any heterogenous CBOR map, irrespective of whether they're indefinite or definite.
///
/// A good choice for `S` is generally to pick a tuple of `PartialDecoder<_>` for each field item
/// that needs decoding. For example:
///
/// ```rs
/// let (address, value, datum, script) = decode_map(
///     d,
///     (
///         missing_field::<Output, _>(0),
///         missing_field::<Output, _>(1),
///         with_default_value(MemoizedDatum::None),
///         with_default_value(None),
///     ),
///     |d| d.u8(),
///     |d, state, field| {
///         match field {
///             0 => state.0 = decode_chunk(d, |d| decode_address(d.bytes()?)),
///             1 => state.1 = decode_chunk(d, |d| d.decode()),
///             2 => state.2 = decode_chunk(d, decode_datum),
///             3 => state.3 = decode_chunk(d, decode_reference_script),
///             _ => {
///                 return Err(cbor::decode::Error::message(
///                     "unexpected key in transaction output map",
///                 ))
///             }
///         }
///         Ok(())
///     },
/// )?;
/// ```
pub fn heterogenous_map<K, S>(
    d: &mut cbor::Decoder<'_>,
    mut state: S,
    decode_key: impl Fn(&mut cbor::Decoder<'_>) -> Result<K, cbor::decode::Error>,
    mut decode_value: impl FnMut(&mut cbor::Decoder<'_>, &mut S, K) -> Result<(), cbor::decode::Error>,
) -> Result<S, cbor::decode::Error> {
    let len = d.map()?;

    let mut n = 0;
    while len.is_none() || Some(n) < len {
        if decode_break(d, len)? {
            break;
        }

        let k = decode_key(d)?;
        decode_value(d, &mut state, k)?;

        n += 1;
    }

    Ok(state)
}

// PartialDecoder
// ----------------------------------------------------------------------------

/// A decoder that is part of another larger one. This is particularly useful to decode map
/// key/value in an arbitrary order; while logically recomposing them in a readable order.
type PartialDecoder<A> = Box<dyn FnOnce() -> Result<A, cbor::decode::Error>>;

/// Wrap a decoder as a `PartialDecoder`; this is mostly a convenient utility to avoid boilerplate.
pub fn decode_chunk<A: 'static>(
    d: &mut cbor::Decoder<'_>,
    decode: impl FnOnce(&mut cbor::Decoder<'_>) -> Result<A, cbor::decode::Error>,
) -> PartialDecoder<A> {
    // NOTE: It is crucial that this happens *outside* of the boxed closure, to ensure bytes are consumed
    // when the closure is created; not when it is invoked!
    let a = decode(d);
    Box::new(|| a)
}

/// Yield a `PartialDecoder` that fails with a comprehensible error message.
pub fn missing_field<C: ?Sized, A>(field_tag: impl Display) -> PartialDecoder<A> {
    let msg = format!(
        "missing {} at field {field_tag} in {} map",
        std::any::type_name::<A>(),
        std::any::type_name::<C>(),
    );
    Box::new(move || Err(cbor::decode::Error::message(msg)))
}

/// Yield a `PartialDecoder` that always succeeds with the given default value.
pub fn with_default_value<A: 'static>(default: A) -> PartialDecoder<A> {
    Box::new(move || Ok(default))
}
