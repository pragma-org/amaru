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

//! Forgive the order of the labels in a transaction metadata map.
//!
//! The ledger holds those labels in a `Map`, so it reads them in key order like amaru does.
//! However the Haskell node keeps the whole auxiliary data in a `MemoBytes` and replays
//! those bytes rather than writing the map again. amaru choses to _not_ memoise them.
//!
//! This modules sorts the labels on every rules that contains metadata in order to check re-encoded
//! data vs expected data.

use amaru_kernel::cbor::{self, data::Type};

/// Position of the auxiliary data among the fields of a `transaction`.
const AUXILIARY_DATA_IN_TRANSACTION: u64 = 3;

/// Position of the auxiliary data among the fields of a `block`.
pub const AUXILIARY_DATA_IN_BLOCK: u64 = 3;

/// Key of the metadata labels inside Conway auxiliary data.
const METADATA_IN_AUXILIARY_DATA: u64 = 0;

/// Whether a rule's samples carry a metadata map whose label order the node only preserves by replaying bytes.
pub fn forgives_metadata_order(rule: &str) -> bool {
    matches!(
        rule,
        "metadata" | "auxiliary_data" | "auxiliary_data_array" | "auxiliary_data_map" | "transaction" | "block"
    )
}

/// Rewrite a sample so that every metadata map carries its labels in ascending order, leaving every other
/// byte as it was.
///
/// The sample must already be shape-normalised, so that every container it walks is definite-length.
pub fn sort_metadata_labels(rule: &str, bytes: &[u8]) -> Result<Vec<u8>, cbor::decode::Error> {
    let mut d = cbor::Decoder::new(bytes);
    let mut out = Vec::with_capacity(bytes.len());

    match rule {
        "metadata" => labels(&mut d, &mut out)?,

        "auxiliary_data" | "auxiliary_data_array" | "auxiliary_data_map" => auxiliary_data(&mut d, &mut out)?,

        "transaction" => {
            for index in 0..copy_array_head(&mut d, &mut out)? {
                if index == AUXILIARY_DATA_IN_TRANSACTION {
                    auxiliary_data(&mut d, &mut out)?;
                } else {
                    copy_item(&mut d, &mut out)?;
                }
            }
        }

        "block" => {
            for index in 0..copy_array_head(&mut d, &mut out)? {
                if index == AUXILIARY_DATA_IN_BLOCK {
                    // The auxiliary data of a block is a map from transaction index to one of the above.
                    for _ in 0..copy_map_head(&mut d, &mut out)? {
                        copy_item(&mut d, &mut out)?;
                        auxiliary_data(&mut d, &mut out)?;
                    }
                } else {
                    copy_item(&mut d, &mut out)?;
                }
            }
        }

        _ => {
            return Err(cbor::decode::Error::message(format!("rule `{rule}` carries no metadata")));
        }
    }

    Ok(out)
}

/// Copy auxiliary data in any of the three era forms it reaches the wire in, sorting the labels it holds.
///
/// Shelley wrote the labels on their own, Allegra wrapped them in an array alongside the scripts, and Conway
/// tags a map in which the labels are one field.
fn auxiliary_data(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<(), cbor::decode::Error> {
    let datatype = d.datatype()?;

    if datatype == Type::Map {
        return labels(d, out);
    }

    if datatype == Type::Array {
        for index in 0..copy_array_head(d, out)? {
            if index == 0 {
                labels(d, out)?;
            } else {
                copy_item(d, out)?;
            }
        }
        return Ok(());
    }

    if datatype == Type::Tag {
        let start = d.position();
        d.tag()?;
        copy_from(d, out, start);

        for _ in 0..copy_map_head(d, out)? {
            let key = d.u64()?;
            cbor::Encoder::new(&mut *out).u64(key).map_err(encode_error)?;

            if key == METADATA_IN_AUXILIARY_DATA {
                labels(d, out)?;
            } else {
                copy_item(d, out)?;
            }
        }
        return Ok(());
    }

    // A transaction without auxiliary data writes `null` in its place.
    copy_item(d, out)
}

/// Copy a metadata map with its labels in ascending order. A label's value is copied as it stands.
fn labels(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<(), cbor::decode::Error> {
    let start = d.position();
    let len = d.map()?.ok_or_else(|| indefinite_container("map", start))?;
    copy_from(d, out, start);

    let mut entries = Vec::with_capacity(len as usize);
    for _ in 0..len {
        let key = span(d)?;
        let value = span(d)?;
        entries.push((key, value));
    }

    entries.sort_by(|(left, _), (right, _)| d.input()[left.clone()].cmp(&d.input()[right.clone()]));

    for (key, value) in entries {
        out.extend_from_slice(&d.input()[key]);
        out.extend_from_slice(&d.input()[value]);
    }

    Ok(())
}

/// The range the next item occupies in the input.
fn span(d: &mut cbor::Decoder<'_>) -> Result<std::ops::Range<usize>, cbor::decode::Error> {
    let start = d.position();
    d.skip()?;
    Ok(start..d.position())
}

/// Copy the head of a definite-length array, returning the number of items that follow it.
fn copy_array_head(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<u64, cbor::decode::Error> {
    let start = d.position();
    let len = d.array()?.ok_or_else(|| indefinite_container("array", start))?;
    copy_from(d, out, start);
    Ok(len)
}

/// Copy the head of a definite-length map, returning the number of entries that follow it.
fn copy_map_head(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<u64, cbor::decode::Error> {
    let start = d.position();
    let len = d.map()?.ok_or_else(|| indefinite_container("map", start))?;
    copy_from(d, out, start);
    Ok(len)
}

/// Copy the next item, whatever it is, byte for byte.
fn copy_item(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<(), cbor::decode::Error> {
    let start = d.position();
    d.skip()?;
    copy_from(d, out, start);
    Ok(())
}

/// Copy the input between `start` and where the decoder now stands.
fn copy_from(d: &cbor::Decoder<'_>, out: &mut Vec<u8>, start: usize) {
    out.extend_from_slice(&d.input()[start..d.position()]);
}

fn indefinite_container(what: &str, position: usize) -> cbor::decode::Error {
    cbor::decode::Error::message(format!("indefinite-length {what} in a shape-normalised sample")).at(position)
}

fn encode_error(e: cbor::encode::Error<std::convert::Infallible>) -> cbor::decode::Error {
    cbor::decode::Error::message(format!("re-encoding while sorting metadata labels: {e}"))
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::sort_metadata_labels;

    /// Labels reach ascending order whichever order they arrived in. `1bffffffffffffffff` is the largest
    /// label there is, so a sample carrying it first is the one that tells the two apart.
    #[test_case("a21bffffffffffffffff010002", "a200021bffffffffffffffff01"; "the largest label arrives first")]
    #[test_case("a200021bffffffffffffffff01", "a200021bffffffffffffffff01"; "already in order")]
    fn sorts_the_labels_of_a_metadata_map(input: &str, expected: &str) {
        let actual = sort_metadata_labels("metadata", &hex::decode(input).unwrap()).unwrap();

        assert_eq!(hex::encode(&actual), expected);
    }

    /// A map nested inside a label's value is content, not form: the ledger holds those pairs in a list and
    /// writes them in the order it read them.
    #[test]
    fn a_map_nested_in_a_value_keeps_its_order() {
        let input = "a100a26262630162616100";

        let actual = sort_metadata_labels("metadata", &hex::decode(input).unwrap()).unwrap();

        assert_eq!(hex::encode(&actual), input);
    }

    /// Conway auxiliary data holds the labels under key 0 of a tagged map; the other fields are untouched.
    #[test]
    fn sorts_the_labels_inside_conway_auxiliary_data() {
        let input = "d90103a200a21bffffffffffffffff0100020280";
        let expected = "d90103a200a200021bffffffffffffffff010280";

        let actual = sort_metadata_labels("auxiliary_data", &hex::decode(input).unwrap()).unwrap();

        assert_eq!(hex::encode(&actual), expected);
    }

    /// Allegra auxiliary data is an array of the labels and the scripts.
    #[test]
    fn sorts_the_labels_inside_allegra_auxiliary_data() {
        let actual =
            sort_metadata_labels("auxiliary_data_array", &hex::decode("82a21bffffffffffffffff01000280").unwrap())
                .unwrap();

        assert_eq!(hex::encode(&actual), "82a200021bffffffffffffffff0180");
    }

    /// A transaction carries its auxiliary data last, and writes `null` when it has none.
    #[test_case("8400010082a21bffffffffffffffff01000280", "8400010082a200021bffffffffffffffff0180"; "with auxiliary data")]
    #[test_case("84000100f6", "84000100f6"; "without auxiliary data")]
    fn sorts_the_labels_inside_a_transaction(input: &str, expected: &str) {
        let actual = sort_metadata_labels("transaction", &hex::decode(input).unwrap()).unwrap();

        assert_eq!(hex::encode(&actual), expected);
    }

    /// A block holds its auxiliary data in a map keyed by transaction index.
    #[test]
    fn sorts_the_labels_inside_a_block() {
        let actual =
            sort_metadata_labels("block", &hex::decode("85008080a10082a21bffffffffffffffff0100028080").unwrap())
                .unwrap();

        assert_eq!(hex::encode(&actual), "85008080a10082a200021bffffffffffffffff018080");
    }
}
