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
use stacksafe::stacksafe;

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
///   - a non-minimal integer head becomes the minimal head for the same value,
///   - a bignum becomes the native head for the same value when the magnitude fits one.
///
/// This mirrors the specification described in `r2rationality/cardano-cbor-dataset`.
pub fn normalize_shape(bytes: &[u8]) -> anyhow::Result<Vec<u8>> {
    let mut d = cbor::Decoder::new(bytes);
    let mut out = Vec::with_capacity(bytes.len());
    rewrite(&mut d, &mut out)?;
    Ok(out)
}

/// Recursively rewrite a CBOR item and its children to normalise the shape of containers and strings.
///
/// Nesting depth is bounded only by what the sample carries, so the recursion grows the stack on demand rather
/// than overflowing on a deeply nested item.
#[stacksafe]
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
            if tag == cbor::IanaTag::PosBignum.tag() || tag == cbor::IanaTag::NegBignum.tag() {
                rewrite_bignum(tag, d, out)?;
            } else {
                cbor::Encoder::new(&mut *out).tag(tag).map_err(encode_error)?;
                rewrite(d, out)?;
            }
        }
        // Floats, booleans, null, undefined and simple values have no shape to normalise.
        Type::Bool
        | Type::Null
        | Type::Undefined
        | Type::F16
        | Type::F32
        | Type::F64
        | Type::Simple
        | Type::Break
        | Type::Unknown(_) => {
            let start = d.position();
            d.skip()?;
            let end = d.position();
            out.extend_from_slice(&d.input()[start..end]);
        }
    }
    Ok(())
}

/// Rewrite a bignum into the native head carrying the same value, when the magnitude fits one.
///
/// `#6.2(n)` denotes `n` and `#6.3(n)` denotes `-(n + 1)`, which is exactly the value a `uint` or a `nint` head
/// with the argument `n` denotes, so the rewrite is exact and loses nothing. A magnitude of more than eight
/// significant bytes has no native head and stays a bignum, with its leading zeroes dropped so that the same
/// number always normalises to the same bytes.
fn rewrite_bignum(tag: cbor::Tag, d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<(), cbor::decode::Error> {
    let magnitude = cbor::decode_bytes(d)?;
    let magnitude = &magnitude[magnitude.iter().take_while(|byte| **byte == 0).count()..];
    let mut encoder = cbor::Encoder::new(out);

    if magnitude.len() <= 8 {
        let mut argument = [0; 8];
        argument[8 - magnitude.len()..].copy_from_slice(magnitude);
        let argument = u64::from_be_bytes(argument);

        if tag == cbor::IanaTag::PosBignum.tag() {
            encoder.u64(argument).map_err(encode_error)?;
        } else {
            let value = cbor::data::Int::try_from(-(i128::from(argument) + 1))
                .map_err(|e| cbor::decode::Error::message(format!("negative bignum out of range: {e}")))?;
            encoder.int(value).map_err(encode_error)?;
        }
    } else {
        encoder.tag(tag).map_err(encode_error)?;
        encoder.bytes(magnitude).map_err(encode_error)?;
    }
    Ok(())
}

/// Normalise the items of a container. `per_item` is 1 for arrays and 2 for maps, so that a map
/// with a dangling key is reported rather than silently truncated.
#[stacksafe]
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

    if !items.len().is_multiple_of(per_item) {
        return Err(cbor::decode::Error::message("map with a dangling key"));
    }
    Ok(items)
}

fn encode_error(e: cbor::encode::Error<std::convert::Infallible>) -> cbor::decode::Error {
    cbor::decode::Error::message(format!("re-encoding while normalising: {e}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use amaru_kernel::utils::stack::{STACK_SIZE_512KIB, with_stack_size};

    use super::{
        super::{read_directory, read_file},
        normalize_shape,
    };

    #[test]
    fn test_normalize() {
        let cases = normalization_cases().expect("failed to read the normalization vectors");
        assert!(!cases.is_empty(), "no normalization vectors found in {}", normalization_dir().display());

        let mut failures = Vec::new();
        for (name, input, expected) in cases {
            match normalize_shape(&input) {
                Ok(actual) if actual == expected => (),
                Ok(actual) => failures.push(format!(
                    "{name}: normalize({}) = {}, expected {}",
                    hex::encode(&input),
                    hex::encode(&actual),
                    hex::encode(&expected)
                )),
                Err(e) => failures.push(format!("{name}: normalize({}) failed: {e}", hex::encode(&input))),
            }
        }

        assert!(failures.is_empty(), "{} normalization vector(s) failed:\n{}", failures.len(), failures.join("\n"));
    }

    /// Return the `(name, input, expected)` triple of every vector, ordered by name so runs are reproducible.
    fn normalization_cases() -> anyhow::Result<Vec<(String, Vec<u8>, Vec<u8>)>> {
        let dir = normalization_dir();
        let mut cases = Vec::new();
        for entry in read_directory(&dir)? {
            let path = entry.path();
            let file_name = entry.file_name().to_string_lossy().into_owned();
            let Some(name) = file_name.strip_suffix(".in.cbor") else {
                continue;
            };
            let expected_at = dir.join(format!("{name}.out.cbor"));
            assert!(expected_at.is_file(), "{file_name} has no matching {name}.out.cbor");
            cases.push((name.to_string(), read_file(&path)?, read_file(&expected_at)?));
        }
        cases.sort();
        Ok(cases)
    }

    /// A sample nested far deeper than any plain recursion would survive on a small stack, normalised in a thread
    /// with a 512 KiB stack so that a regression in the stack handling fails here rather than on some future corpus.
    #[test]
    fn test_normalize_deeply_nested() {
        const DEPTH: usize = 100_000;

        let mut nested = vec![0x00];
        for _ in 0..DEPTH {
            nested.insert(0, 0x9f);
            nested.push(0xff);
        }

        let normalized = with_stack_size(STACK_SIZE_512KIB, move || normalize_shape(&nested))
            .expect("normalising a deeply nested sample overflowed the stack")
            .expect("failed to normalise a deeply nested sample");

        let mut expected = vec![0x00];
        for _ in 0..DEPTH {
            expected.insert(0, 0x81);
        }
        assert_eq!(normalized, expected);
    }

    fn normalization_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/common/normalization")
    }
}
