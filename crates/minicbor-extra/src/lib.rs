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

use minicbor as cbor;
use std::convert::Infallible;

pub use decode::*;
pub mod decode;

#[allow(clippy::unwrap_used)]
/// Encode any serialisable value `T` into bytes.
pub fn to_cbor<T: cbor::Encode<()>>(value: &T) -> Vec<u8> {
    let mut buffer = Vec::new();
    let result: Result<(), cbor::encode::Error<Infallible>> = cbor::encode(value, &mut buffer);
    result.unwrap(); // Infallible
    buffer
}

/// Decode raw bytes into a structured type `T`, assuming no context.
pub fn from_cbor<T: for<'d> cbor::Decode<'d, ()>>(bytes: &[u8]) -> Option<T> {
    cbor::decode(bytes).ok()
}

/// Decode a CBOR input, ensuring that there are no bytes leftovers once decoded. This is handy to
/// test standalone decoders and ensures that they entirely consume their inputs.
pub fn from_cbor_no_leftovers<T: for<'d> cbor::Decode<'d, ()>>(
    bytes: &[u8],
) -> Result<T, cbor::decode::Error> {
    cbor::decode(bytes).map(|NoLeftovers(inner)| inner)
}

#[repr(transparent)]
struct NoLeftovers<A>(A);

impl<'a, C, A: cbor::Decode<'a, C>> cbor::decode::Decode<'a, C> for NoLeftovers<A> {
    fn decode(d: &mut cbor::Decoder<'a>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let inner = d.decode_with(ctx)?;

        if !d.datatype().is_err_and(|e| e.is_end_of_input()) {
            return Err(cbor::decode::Error::message(format!(
                "leftovers bytes after decoding after position {}",
                d.position()
            )));
        }

        Ok(NoLeftovers(inner))
    }
}

#[cfg(test)]
mod tests {
    use crate::{cbor, from_cbor, from_cbor_no_leftovers, to_cbor};
    use foo::Foo;

    #[test]
    fn from_cbor_no_leftovers_catches_trailing_breaks() {
        #[derive(Debug, PartialEq, Eq)]
        struct TestCase<A>(A);

        // Incomplete decoder that ignores the trailing break caracter.
        impl<'d, C> cbor::decode::Decode<'d, C> for TestCase<Foo> {
            fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
                d.array()?;
                Ok(TestCase(Foo {
                    field0: d.decode_with(ctx)?,
                    field1: d.decode_with(ctx)?,
                }))
            }
        }

        let original_foo = Foo {
            field0: 14,
            field1: 42,
        };

        let bytes = to_cbor(&AsIndefinite(&original_foo));

        assert_eq!(Some(TestCase(original_foo)), from_cbor(&bytes));
        assert!(from_cbor_no_leftovers::<TestCase<Foo>>(&bytes).is_err())
    }

    pub(crate) struct AsIndefinite<A>(pub(crate) A);

    pub(crate) struct AsDefinite<A>(pub(crate) A);

    pub(crate) struct AsMap<A>(pub(crate) A);

    pub(crate) mod foo {
        use super::{AsDefinite, AsIndefinite, AsMap};
        use minicbor as cbor;

        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) struct Foo {
            pub(crate) field0: u8,
            pub(crate) field1: u8,
        }

        impl<C> cbor::encode::Encode<C> for AsIndefinite<&Foo> {
            fn encode<W: cbor::encode::Write>(
                &self,
                e: &mut cbor::Encoder<W>,
                ctx: &mut C,
            ) -> Result<(), cbor::encode::Error<W::Error>> {
                e.begin_array()?;
                e.encode_with(self.0.field0, ctx)?;
                e.encode_with(self.0.field1, ctx)?;
                e.end()?;
                Ok(())
            }
        }

        impl<C> cbor::encode::Encode<C> for AsIndefinite<AsMap<&Foo>> {
            fn encode<W: cbor::encode::Write>(
                &self,
                e: &mut cbor::Encoder<W>,
                ctx: &mut C,
            ) -> Result<(), cbor::encode::Error<W::Error>> {
                e.begin_map()?;
                e.encode_with(0_u8, ctx)?;
                e.encode_with(self.0 .0.field0, ctx)?;
                e.encode_with(1_u8, ctx)?;
                e.encode_with(self.0 .0.field1, ctx)?;
                e.end()?;
                Ok(())
            }
        }

        impl<C> cbor::encode::Encode<C> for AsDefinite<&Foo> {
            fn encode<W: cbor::encode::Write>(
                &self,
                e: &mut cbor::Encoder<W>,
                ctx: &mut C,
            ) -> Result<(), cbor::encode::Error<W::Error>> {
                e.array(2)?;
                e.encode_with(self.0.field0, ctx)?;
                e.encode_with(self.0.field1, ctx)?;
                Ok(())
            }
        }

        impl<C> cbor::encode::Encode<C> for AsDefinite<AsMap<&Foo>> {
            fn encode<W: cbor::encode::Write>(
                &self,
                e: &mut cbor::Encoder<W>,
                ctx: &mut C,
            ) -> Result<(), cbor::encode::Error<W::Error>> {
                e.map(2)?;
                e.encode_with(0_u8, ctx)?;
                e.encode_with(self.0 .0.field0, ctx)?;
                e.encode_with(1_u8, ctx)?;
                e.encode_with(self.0 .0.field1, ctx)?;
                Ok(())
            }
        }

        impl<'d, C> cbor::decode::Decode<'d, C> for AsDefinite<Foo> {
            fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
                let len = d.array()?;
                let foo = Foo {
                    field0: d.decode_with(ctx)?,
                    field1: d.decode_with(ctx)?,
                };
                match len {
                    Some(2) => Ok(AsDefinite(foo)),
                    _ => Err(cbor::decode::Error::message(
                        "invalid or missing definite array length",
                    )),
                }
            }
        }

        impl<'d, C> cbor::decode::Decode<'d, C> for AsIndefinite<Foo> {
            fn decode(d: &mut cbor::Decoder<'d>, ctx: &mut C) -> Result<Self, cbor::decode::Error> {
                let len = d.array()?;
                let foo = Foo {
                    field0: d.decode_with(ctx)?,
                    field1: d.decode_with(ctx)?,
                };
                match len {
                    None if d.datatype()? == cbor::data::Type::Break => {
                        d.skip()?;
                        Ok(AsIndefinite(foo))
                    }
                    _ => Err(cbor::decode::Error::message(
                        "missing indefinite array break",
                    )),
                }
            }
        }
    }
}
