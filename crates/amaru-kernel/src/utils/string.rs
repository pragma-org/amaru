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

use crate::{Bytes, cbor, from_cbor};
use itertools::Itertools;
use std::fmt::{Debug, Display};

pub fn encode_bech32(hrp: &str, payload: &[u8]) -> Result<String, Box<dyn std::error::Error>> {
    let hrp = bech32::Hrp::parse(hrp)?;
    Ok(bech32::encode::<bech32::Bech32>(hrp, payload)?)
}

pub fn display_collection<T>(collection: impl IntoIterator<Item = T>) -> String
where
    T: std::fmt::Display,
{
    collection
        .into_iter()
        .collect::<Vec<_>>()
        .list_to_string(", ")
}

/// Extension trait to convert a list of displayable items into a single string.
/// For example, `vec![1, 2, 3].list_to_string(", ")` will produce the string `1, 2, 3`.
pub trait ListToString {
    fn list_to_string(&self, separator: &str) -> String;
}

impl<I, H> ListToString for I
where
    for<'a> &'a I: IntoIterator<Item = &'a H>,
    H: Display,
{
    fn list_to_string(&self, separator: &str) -> String {
        self.into_iter().join(separator)
    }
}

/// Extension trait to convert a list of lists of displayable items into a single string.
/// For example, `vec![vec![1, 2], vec![3]].lists_to_string(", ", " | ")` will produce
/// the string `"[1, 2] | [3]"`.
pub trait ListsToString {
    fn lists_to_string(&self, intra_separator: &str, inter_separator: &str) -> String;
}

impl<H, I, J> ListsToString for J
where
    for<'a> &'a I: IntoIterator<Item = &'a H>,
    for<'a> &'a J: IntoIterator<Item = &'a I>,
    H: Display,
{
    fn lists_to_string(&self, intra_separator: &str, inter_separator: &str) -> String {
        self.into_iter()
            .map(|l| format!("[{}]", l.list_to_string(intra_separator)))
            .collect::<Vec<_>>()
            .list_to_string(inter_separator)
    }
}

/// Extension trait to convert a list of debug-printable items into a single string.
/// For example, `vec![Some(1), None, Some(3)].list_debug(", ")` will produce
/// the string `Some(1), None, Some(3)`
pub trait ListDebug {
    fn list_debug(&self, separator: &str) -> String;
}

impl<I, H> ListDebug for I
where
    for<'a> &'a I: IntoIterator<Item = &'a H>,
    H: Debug,
{
    fn list_debug(&self, separator: &str) -> String {
        self.into_iter().map(|h| format!("{h:?}")).join(separator)
    }
}

/// Extension trait to convert a list of lists of debuggable items into a single string.
/// For example, `vec![vec![Some(1), Some(2)], vec![Some(3)]].lists_debug(", ", " | ")` will produce
/// the string `"[Some(1), Some(2)] | [Some(3)]"`.
pub trait ListsDebug {
    fn lists_debug(&self, intra_separator: &str, inter_separator: &str) -> String;
}

impl<H, I, J> ListsDebug for J
where
    for<'a> &'a I: IntoIterator<Item = &'a H>,
    for<'a> &'a J: IntoIterator<Item = &'a I>,
    H: Debug,
{
    fn lists_debug(&self, intra_separator: &str, inter_separator: &str) -> String {
        self.into_iter()
            .map(|l| format!("[{}]", l.list_debug(intra_separator)))
            .collect::<Vec<_>>()
            .list_to_string(inter_separator)
    }
}

/// An implementation of 'TryFrom' from hex-encoded strings, for any type that can be decoded from
/// CBOR. Yields the original bytes and the deserialized value.
pub fn blanket_try_from_hex_bytes<T, I: for<'d> cbor::Decode<'d, ()>>(
    s: &str,
    new: impl Fn(Bytes, I) -> T,
) -> Result<T, String> {
    let original_bytes = Bytes::from(hex::decode(s.as_bytes()).map_err(|e| e.to_string())?);

    let value =
        from_cbor(&original_bytes).ok_or_else(|| "failed to decode from CBOR".to_string())?;

    Ok(new(original_bytes, value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_a_string_from_a_list() {
        let actual = vec![1, 2, 3].list_to_string(", ");
        assert_eq!(actual, "1, 2, 3");
    }

    #[test]
    fn make_a_debug_string_from_a_list() {
        let actual = vec![Some(1), None, Some(3)].list_debug(", ");
        assert_eq!(actual, "Some(1), None, Some(3)");
    }

    #[test]
    fn make_a_string_from_a_list_of_lists() {
        let actual = vec![vec![1, 2], vec![3]].lists_to_string(", ", " | ");
        assert_eq!(actual, "[1, 2] | [3]");
    }
}
