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

//! Shape normalisation for comparing CBOR encodings that differ only in form.

use amaru_kernel::cbor::{self, data::Type};

/// Rewrite a CBOR item so that its shape is canonical while its content is untouched.
///
/// The CDDL admits both the definite and the indefinite form for every container and string, so
/// which one an implementation emits is not part of the format. Normalising both sides before
/// comparing forgives that choice while keeping every difference in *what* was read: a dropped map
/// entry, a reordered one, a truncated list and a changed integer all survive normalisation.
///
/// Normalised:
///   - indefinite-length array or map becomes definite, same items in the same order,
///   - chunked byte or text string becomes a single definite string,
///   - a non-minimal integer head becomes the minimal head for the same value.
///
/// Deliberately **not** normalised:
///   - map key order, which is observable in Cardano,
///   - duplicate map keys, which are content rather than form,
///   - tags, including tag numbers nothing here recognises,
///   - floats, and the value of any integer across the whole `u64` range.
///
/// This mirrors the specification proposed upstream in `r2rationality/cardano-cbor-dataset`; once
/// that ships a pre-normalised reference tree, the reference side of the comparison can use it
/// directly and only amaru's own output needs normalising here.
pub fn normalize_shape(bytes: &[u8]) -> Vec<u8> {
    let mut d = cbor::Decoder::new(bytes);
    let mut out = Vec::with_capacity(bytes.len());
    rewrite(&mut d, &mut out).expect("normalising a valid CBOR item should not fail");
    out
}

fn rewrite(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<(), cbor::decode::Error> {
    match d.datatype()? {
        Type::Array | Type::ArrayIndef => {
            let items = children(d, 1)?;
            cbor::Encoder::new(&mut *out).array(items.len() as u64).map_err(encode_error)?;
            for item in items {
                out.extend_from_slice(&item);
            }
        }
        Type::Map | Type::MapIndef => {
            let items = children(d, 2)?;
            cbor::Encoder::new(&mut *out).map((items.len() / 2) as u64).map_err(encode_error)?;
            for item in items {
                out.extend_from_slice(&item);
            }
        }
        Type::Bytes | Type::BytesIndef => {
            let value = cbor::decode_bytes(d)?;
            cbor::Encoder::new(&mut *out).bytes(&value).map_err(encode_error)?;
        }
        Type::String | Type::StringIndef => {
            let value = cbor::decode_string(d)?;
            cbor::Encoder::new(&mut *out).str(&value).map_err(encode_error)?;
        }
        Type::U8 | Type::U16 | Type::U32 | Type::U64 | Type::I8 | Type::I16 | Type::I32 | Type::I64 | Type::Int => {
            let value = d.int()?;
            cbor::Encoder::new(&mut *out).int(value).map_err(encode_error)?;
        }
        Type::Tag => {
            let tag = d.tag()?;
            cbor::Encoder::new(&mut *out).tag(tag).map_err(encode_error)?;
            rewrite(d, out)?;
        }
        // Floats, booleans, null, undefined and simple values have no shape to normalise.
        _ => {
            let start = d.position();
            d.skip()?;
            let end = d.position();
            out.extend_from_slice(&d.input()[start..end]);
        }
    }
    Ok(())
}

/// Normalise the items of a container. `per_item` is 1 for arrays and 2 for maps, so that a map
/// with a dangling key is reported rather than silently truncated.
fn children(d: &mut cbor::Decoder<'_>, per_item: usize) -> Result<Vec<Vec<u8>>, cbor::decode::Error> {
    let len = if per_item == 1 { d.array()? } else { d.map()? };
    let expected = len.map(|len| len as usize * per_item);

    let mut items: Vec<Vec<u8>> = Vec::new();
    while expected.is_none_or(|expected| items.len() < expected) {
        if cbor::decode_break(d, len)? {
            break;
        }
        let mut item = Vec::new();
        rewrite(d, &mut item)?;
        items.push(item);
    }

    if items.len() % per_item != 0 {
        return Err(cbor::decode::Error::message("map with a dangling key"));
    }
    Ok(items)
}

fn encode_error(e: cbor::encode::Error<std::convert::Infallible>) -> cbor::decode::Error {
    cbor::decode::Error::message(format!("re-encoding while normalising: {e}"))
}
