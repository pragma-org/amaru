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
use minicbor::decode;
use std::fmt::Display;

pub mod lazy;

// Misc
// ----------------------------------------------------------------------------

pub fn decode_break<'d>(
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

/// Decode a chunk, but retain a reference to the decoded bytes.
pub fn tee<'d, A>(
    d: &mut cbor::Decoder<'d>,
    decoder: impl FnOnce(&mut cbor::Decoder<'d>) -> Result<A, cbor::decode::Error>,
) -> Result<(A, &'d [u8]), cbor::decode::Error> {
    let original_bytes = d.input();
    let start = d.position();
    let a = decoder(d)?;
    let end = d.position();
    Ok((a, &original_bytes[start..end]))
}

// Array
// ----------------------------------------------------------------------------

/// Decode any heterogeneous CBOR array, irrespective of whether they're indefinite or definite.
///
/// FIXME: Allow callers to check that the length is not static, but simply matches what is
/// advertised; e.g. using Option<u64> as a callback.
pub fn heterogeneous_array<'d, A>(
    d: &mut cbor::Decoder<'d>,
    elems: impl FnOnce(
        &mut cbor::Decoder<'d>,
        &dyn Fn(u64) -> Result<(), cbor::decode::Error>,
    ) -> Result<A, cbor::decode::Error>,
) -> Result<A, cbor::decode::Error> {
    let len = d.array()?;

    match len {
        None => {
            let result = elems(d, &|_| Ok(()))?;
            decode_break(d, len)?;
            Ok(result)
        }
        Some(len) => elems(
            d,
            &(move |expected_len| {
                if len != expected_len {
                    return Err(cbor::decode::Error::message(format!(
                        "CBOR array length mismatch: expected {} got {}",
                        expected_len, len
                    )));
                }

                Ok(())
            }),
        ),
    }
}

/// This function checks the size of an array containing a tagged value.
/// The `label` parameter is used to identify which variant is being checked.
///
/// FIXME: suspicious check_tagged_array_length
///
/// This function is a code smell and seems to indicate that we are manually decoding def
/// array somewhere, instead of using the heterogeneous_array above to also deal indef arrays.
/// There might be a good reason why this function exists; I haven't checked, but leaving a note
/// for later to check.
pub fn check_tagged_array_length(
    label: usize,
    actual: Option<u64>,
    expected: u64,
) -> Result<(), decode::Error> {
    if actual != Some(expected) {
        Err(decode::Error::message(format!(
            "expected array length {expected} for label {label}, got: {actual:?}"
        )))
    } else {
        Ok(())
    }
}

// Map
// ----------------------------------------------------------------------------

/// Decode any heterogeneous CBOR map, irrespective of whether they're indefinite or definite.
///
/// A good choice for `S` is generally to pick a tuple of `Option` for each field item
/// that needs decoding. For example:
///
/// ```rs
/// let (address, value, datum, script) = decode_map(
///     d,
///     (None, None, MemoizedDatum::None, None),
///     |d| d.u8(),
///     |d, state, field| {
///         match field {
///             0 => state.0 = Some(decode_address(d.bytes()?),
///             1 => state.1 = Some(d.decode()?),
///             2 => state.2 = decode_datum()?,
///             3 => state.3 = decode_reference_script()?,
///             _ => return unexpected_field::<Output, _>(field),
///         }
///         Ok(())
///     },
/// )?;
/// ```
pub fn heterogeneous_map<K, S>(
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

/// Yield a `PartialDecoder` that fails with a comprehensible error message when an expected field
/// is missing from the map.
pub fn missing_field<C: ?Sized, A>(field_tag: u8) -> cbor::decode::Error {
    let msg = format!(
        "missing <{}> at field .{field_tag} in <{}> CBOR map",
        std::any::type_name::<A>(),
        std::any::type_name::<C>(),
    );
    cbor::decode::Error::message(msg)
}

/// Yield a `Result<_, decode::Error>` that always fails with a comprehensible error message when a
/// map key is unexpected.
pub fn unexpected_field<C: ?Sized, A>(field_tag: impl Display) -> Result<A, cbor::decode::Error> {
    Err(cbor::decode::Error::message(format!(
        "unexpected field .{field_tag} in <{}> CBOR map",
        std::any::type_name::<C>(),
    )))
}

// Tests
// ----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use crate::{
        cbor, from_cbor, from_cbor_no_leftovers, heterogeneous_array, heterogeneous_map,
        missing_field,
        tests::{AsDefinite, AsIndefinite, AsMap, foo::Foo},
        to_cbor, unexpected_field,
    };
    use std::fmt::Debug;

    fn assert_ok<T: Eq + Debug + for<'d> cbor::decode::Decode<'d, ()>>(left: T, bytes: &[u8]) {
        assert_eq!(
            Ok(left),
            from_cbor_no_leftovers::<T>(bytes).map_err(|e| e.to_string())
        );
    }

    fn assert_err<T: Debug + for<'d> cbor::decode::Decode<'d, ()>>(msg: &str, bytes: &[u8]) {
        match from_cbor_no_leftovers::<T>(bytes).map_err(|e| e.to_string()) {
            Err(e) => assert!(e.contains(msg), "{e}"),
            Ok(ok) => panic!("expected error but got {:#?}", ok),
        }
    }

    const FIXTURE: Foo = Foo {
        field0: 14,
        field1: 42,
    };

    mod heterogeneous_array_tests {
        use super::*;

        #[test]
        fn happy_case() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            // A flexible decoder that can ingest both definite and indefinite arrays.
            impl<'d, C> cbor::decode::Decode<'d, C> for TestCase<Foo> {
                fn decode(
                    d: &mut cbor::Decoder<'d>,
                    ctx: &mut C,
                ) -> Result<Self, cbor::decode::Error> {
                    heterogeneous_array(d, |d, assert_len| {
                        assert_len(2)?;
                        Ok(TestCase(Foo {
                            field0: d.decode_with(ctx)?,
                            field1: d.decode_with(ctx)?,
                        }))
                    })
                }
            }

            assert_ok(TestCase(FIXTURE), &to_cbor(&AsDefinite(&FIXTURE)));
            assert_ok(TestCase(FIXTURE), &to_cbor(&AsIndefinite(&FIXTURE)));
        }

        #[test]
        fn smaller_definite_length() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            // A decoder which expects less elements than actually supplied.
            impl<'d, C> cbor::decode::Decode<'d, C> for TestCase<Foo> {
                fn decode(
                    d: &mut cbor::Decoder<'d>,
                    ctx: &mut C,
                ) -> Result<Self, cbor::decode::Error> {
                    heterogeneous_array(d, |d, assert_len| {
                        assert_len(1)?;
                        Ok(TestCase(Foo {
                            field0: d.decode_with(ctx)?,
                            field1: d.decode_with(ctx)?,
                        }))
                    })
                }
            }

            assert_err::<TestCase<Foo>>("array length mismatch", &to_cbor(&AsDefinite(&FIXTURE)));
        }

        #[test]
        fn larger_definite_length() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            // A decoder which expects more elements than actually supplied.
            impl<'d, C> cbor::decode::Decode<'d, C> for TestCase<Foo> {
                fn decode(
                    d: &mut cbor::Decoder<'d>,
                    ctx: &mut C,
                ) -> Result<Self, cbor::decode::Error> {
                    heterogeneous_array(d, |d, assert_len| {
                        assert_len(3)?;
                        Ok(TestCase(Foo {
                            field0: d.decode_with(ctx)?,
                            field1: d.decode_with(ctx)?,
                        }))
                    })
                }
            }

            assert_err::<TestCase<Foo>>("array length mismatch", &to_cbor(&AsDefinite(&FIXTURE)))
        }

        #[test]
        fn incomplete_indefinite() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            // An incomplete encoder, which skips the final break on indefinite arrays.
            impl<C> cbor::encode::Encode<C> for TestCase<&Foo> {
                fn encode<W: cbor::encode::Write>(
                    &self,
                    e: &mut cbor::Encoder<W>,
                    ctx: &mut C,
                ) -> Result<(), cbor::encode::Error<W::Error>> {
                    e.begin_array()?;
                    e.encode_with(self.0.field0, ctx)?;
                    e.encode_with(self.0.field1, ctx)?;
                    Ok(())
                }
            }

            let bytes = to_cbor(&TestCase(&FIXTURE));

            assert!(from_cbor::<AsDefinite<Foo>>(&bytes).is_none());
            assert!(from_cbor::<AsIndefinite<Foo>>(&bytes).is_none());
        }
    }

    mod heterogeneous_map_tests {
        use super::*;

        /// A decoder for `Foo` that interpret it as a map, and fails in case of a missing field.
        #[derive(Debug, PartialEq, Eq)]
        struct NoMissingFields<A>(A);
        impl<'d, C> cbor::decode::Decode<'d, C> for NoMissingFields<Foo> {
            fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
                let (field0, field1) = heterogeneous_map(
                    d,
                    (None::<u8>, None::<u8>),
                    |d| d.u8(),
                    |d, state, field| {
                        match field {
                            0 => state.0 = d.decode_with(ctx)?,
                            1 => state.1 = d.decode_with(ctx)?,
                            _ => return unexpected_field::<Foo, _>(field),
                        }
                        Ok(())
                    },
                )?;

                Ok(NoMissingFields(Foo {
                    field0: field0.ok_or_else(|| missing_field::<Foo, u8>(0))?,
                    field1: field1.ok_or_else(|| missing_field::<Foo, u8>(1))?,
                }))
            }
        }

        /// A decoder for `Foo` that interpret it as a map, but allows fields to be missing.
        #[derive(Debug, PartialEq, Eq)]
        struct WithDefaultValues<A>(A);
        impl<'d, C> cbor::decode::Decode<'d, C> for WithDefaultValues<Foo> {
            fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
                let (field0, field1) = heterogeneous_map(
                    d,
                    (14_u8, 42_u8),
                    |d| d.u8(),
                    |d, state, field| {
                        match field {
                            0 => state.0 = d.decode_with(ctx)?,
                            1 => state.1 = d.decode_with(ctx)?,
                            _ => return unexpected_field::<Foo, _>(field),
                        }
                        Ok(())
                    },
                )?;

                Ok(WithDefaultValues(Foo { field0, field1 }))
            }
        }

        #[test]
        fn no_optional_fields_no_missing_fields() {
            assert_ok(
                NoMissingFields(FIXTURE),
                &to_cbor(&AsIndefinite(AsMap(&FIXTURE))),
            );

            assert_ok(
                NoMissingFields(FIXTURE),
                &to_cbor(&AsDefinite(AsMap(&FIXTURE))),
            );
        }

        #[test]
        fn out_of_order_fields() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            // An invalid encoder, which adds an extra break in an definite map.
            impl<C> cbor::encode::Encode<C> for TestCase<&Foo> {
                fn encode<W: cbor::encode::Write>(
                    &self,
                    e: &mut cbor::Encoder<W>,
                    ctx: &mut C,
                ) -> Result<(), cbor::encode::Error<W::Error>> {
                    e.map(2)?;
                    e.encode_with(1_u8, ctx)?;
                    e.encode_with(self.0.field1, ctx)?;
                    e.encode_with(0_u8, ctx)?;
                    e.encode_with(self.0.field0, ctx)?;
                    Ok(())
                }
            }

            assert_ok(NoMissingFields(FIXTURE), &to_cbor(&TestCase(&FIXTURE)));
        }

        #[test]
        fn optional_fields_no_missing_fields() {
            assert_ok(
                WithDefaultValues(FIXTURE),
                &to_cbor(&AsIndefinite(AsMap(&FIXTURE))),
            );

            assert_ok(
                WithDefaultValues(FIXTURE),
                &to_cbor(&AsDefinite(AsMap(&FIXTURE))),
            );
        }

        #[test]
        fn one_field_missing() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            impl<C> cbor::encode::Encode<C> for TestCase<AsIndefinite<&Foo>> {
                fn encode<W: cbor::encode::Write>(
                    &self,
                    e: &mut cbor::Encoder<W>,
                    ctx: &mut C,
                ) -> Result<(), cbor::encode::Error<W::Error>> {
                    e.map(1)?;
                    e.encode_with(0_u8, ctx)?;
                    e.encode_with(self.0.0.field0, ctx)?;
                    Ok(())
                }
            }

            impl<C> cbor::encode::Encode<C> for TestCase<AsDefinite<&Foo>> {
                fn encode<W: cbor::encode::Write>(
                    &self,
                    e: &mut cbor::Encoder<W>,
                    ctx: &mut C,
                ) -> Result<(), cbor::encode::Error<W::Error>> {
                    e.begin_map()?;
                    e.encode_with(1_u8, ctx)?;
                    e.encode_with(self.0.0.field1, ctx)?;
                    e.end()?;
                    Ok(())
                }
            }

            assert_err::<NoMissingFields<Foo>>(
                "missing <u8> at field .1",
                &to_cbor(&TestCase(AsIndefinite(&FIXTURE))),
            );

            assert_ok(
                WithDefaultValues(FIXTURE),
                &to_cbor(&TestCase(AsIndefinite(&FIXTURE))),
            );

            assert_err::<NoMissingFields<Foo>>(
                "missing <u8> at field .0",
                &to_cbor(&TestCase(AsDefinite(&FIXTURE))),
            );

            assert_ok(
                WithDefaultValues(FIXTURE),
                &to_cbor(&TestCase(AsDefinite(&FIXTURE))),
            );
        }

        #[test]
        fn rogue_break() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            // An invalid encoder, which adds an extra break in an definite map.
            impl<C> cbor::encode::Encode<C> for TestCase<&Foo> {
                fn encode<W: cbor::encode::Write>(
                    &self,
                    e: &mut cbor::Encoder<W>,
                    ctx: &mut C,
                ) -> Result<(), cbor::encode::Error<W::Error>> {
                    e.map(2)?;
                    e.encode_with(0_u8, ctx)?;
                    e.encode_with(self.0.field0, ctx)?;
                    e.end()?;
                    Ok(())
                }
            }

            assert_err::<WithDefaultValues<Foo>>(
                "unexpected type break",
                &to_cbor(&TestCase(&FIXTURE)),
            );
        }

        #[test]
        fn unexpected_field_tag() {
            #[derive(Debug, PartialEq, Eq)]
            struct TestCase<A>(A);

            // An invalid encoder, which adds an extra break in an definite map.
            impl<C> cbor::encode::Encode<C> for TestCase<&Foo> {
                fn encode<W: cbor::encode::Write>(
                    &self,
                    e: &mut cbor::Encoder<W>,
                    ctx: &mut C,
                ) -> Result<(), cbor::encode::Error<W::Error>> {
                    e.map(2)?;
                    e.encode_with(0_u8, ctx)?;
                    e.encode_with(self.0.field0, ctx)?;
                    e.encode_with(14_u8, ctx)?;
                    e.encode_with(self.0.field0, ctx)?;
                    Ok(())
                }
            }

            assert_err::<WithDefaultValues<Foo>>(
                "unexpected field .14",
                &to_cbor(&TestCase(&FIXTURE)),
            );
        }
    }
}
