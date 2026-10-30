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

/// Return the number of bytes used by a value's CBOR encoding.
pub trait CborSizeOf {
    fn cbor_size_of(&self) -> usize;
}

impl CborSizeOf for u16 {
    fn cbor_size_of(&self) -> usize {
        match *self {
            0..=23 => 1,
            24..=255 => 2,
            _ => 3,
        }
    }
}

#[cfg(test)]
mod tests {
    use test_case::test_case;

    use super::CborSizeOf;

    #[test_case(17 => 1)]
    #[test_case(23 => 1)]
    #[test_case(24 => 2)]
    #[test_case(255 => 2)]
    #[test_case(256 => 3)]
    fn cbor_size_of_u16_matches_unsigned_integer_encoding(n: u16) -> usize {
        n.cbor_size_of()
    }
}
