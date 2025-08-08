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
