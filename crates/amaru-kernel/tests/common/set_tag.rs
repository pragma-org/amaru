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

//! Forgive the `#6.258` set tag on the fields of a transaction witness set.
//!
//! Below protocol version 12 the tag is optional on those fields: the Haskell decoder accepts a bare array
//! just as it accepts a tagged one, and both denote the same witnesses. The node reproduces whichever form
//! it read, because it keeps a witness set in a `MemoBytes`; amaru rebuilds from the decoded value and
//! always writes the tagged form. Dropping the tag from both sides before comparing forgives that choice
//! and nothing else.
//!
//! The scope is deliberately narrow. Only the values of a witness set map lose their tag, so a set
//! elsewhere in a transaction — the inputs, the certificates, the required signers — is still compared
//! with its tag in place. The tag becomes mandatory at protocol version 12, and the unit tests in
//! `amaru-kernel` pin that amaru writes it, so forgiving it here does not let a missing tag through
//! unnoticed.

use amaru_kernel::cbor::{self, data::Type};

/// Position of the witness set among the fields of a `transaction`.
const WITNESS_SET_IN_TRANSACTION: u64 = 1;

/// Position of the witness sets among the fields of a `block`.
const WITNESS_SETS_IN_BLOCK: u64 = 2;

/// Whether a rule's samples carry a witness set whose set tags are optional.
pub fn forgives_optional_set_tags(rule: &str) -> bool {
    matches!(rule, "transaction_witness_set" | "transaction" | "block")
}

/// Rewrite a sample so that no field of a witness set carries the `#6.258` tag, leaving every other byte
/// as it was.
///
/// The sample must already be shape-normalised, so that every container it walks is definite-length.
pub fn drop_optional_set_tags(rule: &str, bytes: &[u8]) -> Result<Vec<u8>, cbor::decode::Error> {
    let mut d = cbor::Decoder::new(bytes);
    let mut out = Vec::with_capacity(bytes.len());

    match rule {
        "transaction_witness_set" => witness_set(&mut d, &mut out)?,

        "transaction" => {
            for index in 0..copy_array_head(&mut d, &mut out)? {
                if index == WITNESS_SET_IN_TRANSACTION {
                    witness_set(&mut d, &mut out)?;
                } else {
                    copy_item(&mut d, &mut out)?;
                }
            }
        }

        "block" => {
            for index in 0..copy_array_head(&mut d, &mut out)? {
                if index == WITNESS_SETS_IN_BLOCK {
                    for _ in 0..copy_array_head(&mut d, &mut out)? {
                        witness_set(&mut d, &mut out)?;
                    }
                } else {
                    copy_item(&mut d, &mut out)?;
                }
            }
        }

        _ => {
            return Err(cbor::decode::Error::message(format!("rule `{rule}` carries no witness set")));
        }
    }

    Ok(out)
}

/// Copy a witness set, dropping the `#6.258` tag from the value of every field that carries one.
fn witness_set(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<(), cbor::decode::Error> {
    let start = d.position();
    let len = d.map()?.ok_or_else(|| indefinite_container("map", start))?;
    copy_from(d, out, start);

    for _ in 0..len {
        copy_item(d, out)?;

        if d.datatype()? == Type::Tag {
            let start = d.position();
            if d.tag()? != cbor::TAG_SET_258 {
                copy_from(d, out, start);
            }
        }

        copy_item(d, out)?;
    }

    Ok(())
}

/// Copy the head of a definite-length array, returning the number of items that follow it.
fn copy_array_head(d: &mut cbor::Decoder<'_>, out: &mut Vec<u8>) -> Result<u64, cbor::decode::Error> {
    let start = d.position();
    let len = d.array()?.ok_or_else(|| indefinite_container("array", start))?;
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

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::drop_optional_set_tags;

    /// A `vkeywitness`: a 32-byte verification key and the 64-byte signature over it.
    fn witness() -> String {
        format!("825820{}5840{}", "00".repeat(32), "00".repeat(64))
    }

    /// Both forms of a witness set field reduce to the same bytes, and nothing else moves.
    #[test_case("a100d9010281", "a10081"; "a tagged field loses its tag")]
    #[test_case("a10081", "a10081"; "an untagged field is left alone")]
    fn a_witness_set_field_loses_its_tag(input: &str, expected: &str) {
        let input = hex::decode(format!("{input}{}", witness())).unwrap();
        let expected = hex::decode(format!("{expected}{}", witness())).unwrap();

        let actual = drop_optional_set_tags("transaction_witness_set", &input).unwrap();

        assert_eq!(hex::encode(&actual), hex::encode(&expected));
    }

    /// A tag that is not the set tag is content, and a witness set that carries one keeps it.
    #[test]
    fn another_tag_survives() {
        let input = hex::decode("a104d81e820102").unwrap();

        let actual = drop_optional_set_tags("transaction_witness_set", &input).unwrap();

        assert_eq!(hex::encode(&actual), "a104d81e820102");
    }

    /// The sets a transaction body carries are not witness set fields, so their tags stay.
    #[test]
    fn a_set_in_the_transaction_body_keeps_its_tag() {
        let body = "a10082d9010280d9010280";
        let input = hex::decode(format!("84{body}a100d9010281{}f5f6", witness())).unwrap();
        let expected = hex::decode(format!("84{body}a10081{}f5f6", witness())).unwrap();

        let actual = drop_optional_set_tags("transaction", &input).unwrap();

        assert_eq!(hex::encode(&actual), hex::encode(&expected));
    }
}
