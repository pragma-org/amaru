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

use crate::{Slot, cbor};
use pallas_crypto::hash::Hash;
use std::fmt::{self, Debug, Display};

pub const HEADER_HASH_SIZE: usize = 32;

/// Type alias for a header hash to improve readability
pub type HeaderHash = Hash<HEADER_HASH_SIZE>;

#[derive(
    Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize, Default,
)]
pub enum Point {
    #[default]
    Origin,
    Specific(Slot, HeaderHash),
}

impl Point {
    pub fn slot_or_default(&self) -> Slot {
        match self {
            Point::Origin => Slot::from(0),
            Point::Specific(slot, _) => *slot,
        }
    }

    pub fn hash(&self) -> HeaderHash {
        match self {
            // By convention, the hash of `Genesis` is all 0s.
            Point::Origin => Hash::from([0; HEADER_HASH_SIZE]),
            Point::Specific(_, header_hash) => *header_hash,
        }
    }
}

impl Debug for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Point::Origin => write!(f, "Origin"),
            Point::Specific(slot, _hash) => write!(f, "Specific({slot}, {})", self.hash()),
        }
    }
}

impl Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.slot_or_default(), self.hash())
    }
}

impl From<&Point> for HeaderHash {
    fn from(point: &Point) -> Self {
        point.hash()
    }
}

/// Utility function to parse a point from a string.
///
/// Expects the input to be of the form '<point>.<hash>', where `<point>` is a number and `<hash>`
/// is a hex-encoded 32 bytes hash.
/// The first argument is the string to parse, the `bail` function is user to
/// produce the error type `E` in case of failure to parse.
impl TryFrom<&str> for Point {
    type Error = String;

    fn try_from(raw_str: &str) -> Result<Self, Self::Error> {
        let mut split = raw_str.split('.');

        let slot = split
            .next()
            .ok_or("missing slot number before '.'")
            .and_then(|s| {
                s.parse::<u64>()
                    .map_err(|_| "failed to parse point's slot as a non-negative integer")
            })?;

        let block_header_hash = split
            .next()
            .ok_or("missing block header hash after '.'".to_string())
            .and_then(|s| {
                s.parse::<HeaderHash>()
                    .map_err(|e| format!("failed to parse block header hash: {}", e))
            })?;

        Ok(Point::Specific(Slot::from(slot), block_header_hash))
    }
}

impl cbor::encode::Encode<()> for Point {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::encode::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Point::Origin => e.array(0)?,
            Point::Specific(slot, hash) => e.array(2)?.encode(slot)?.encode(hash)?,
        };

        Ok(())
    }
}

impl<'b> cbor::decode::Decode<'b, ()> for Point {
    fn decode(
        d: &mut cbor::decode::Decoder<'b>,
        _ctx: &mut (),
    ) -> Result<Self, cbor::decode::Error> {
        let size = d.array()?;

        match size {
            Some(0) => Ok(Point::Origin),
            Some(2) => {
                let slot = d.decode()?;
                let hash = d.bytes()?;
                if hash.len() != HEADER_HASH_SIZE {
                    return Err(cbor::decode::Error::message("header hash must be 32 bytes"));
                }
                Ok(Point::Specific(slot, HeaderHash::from(hash)))
            }
            _ => Err(cbor::decode::Error::message(
                "can't decode Point from array of size",
            )),
        }
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub mod tests {
    use super::Point;
    use crate::tests::random_bytes;
    use crate::{HEADER_HASH_SIZE, HeaderHash};
    use amaru_slot_arithmetic::Slot;
    use pallas_crypto::hash::Hash;
    use proptest::prelude::*;

    /// Generate a random Hash that could be the hash of a `H: IsHeader` value.
    pub fn random_hash() -> HeaderHash {
        Hash::from(random_bytes(HEADER_HASH_SIZE).as_slice())
    }

    prop_compose! {
        pub fn any_point()(
            slot in any::<u64>(),
            bytes in proptest::array::uniform32(any::<u8>()),
        ) -> Point {
            Point::Specific(Slot::from(slot), Hash::new(bytes))
        }
    }

    #[cfg(test)]
    mod internal {
        use super::*;
        use test_case::test_case;

        #[test_case(Point::Origin => "Origin")]
        #[test_case(
            Point::Specific(
                Slot::from(42),
                Hash::new([
                  254, 252, 156,   3, 124,  63, 156, 139,
                   79, 183, 138, 155,  15,  19, 123,  94,
                  208, 128,  60,  61,  70, 189,  45,  14,
                   64, 197, 159, 169,  12, 160,   2, 193
                ])
            ) => "Specific(42, fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1)";
            "specific"
        )]
        fn better_debug_point(point: Point) -> String {
            format!("{point:?}")
        }

        #[test_case(
            Point::Origin => "0.0000000000000000000000000000000000000000000000000000000000000000";
           "origin"
        )]
        #[test_case(
            Point::Specific(
                Slot::from(42),
                Hash::new([
                  254, 252, 156,   3, 124,  63, 156, 139,
                   79, 183, 138, 155,  15,  19, 123,  94,
                  208, 128,  60,  61,  70, 189,  45,  14,
                   64, 197, 159, 169,  12, 160,   2, 193
                ])
            ) => "42.fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1";
            "specific"
        )]
        fn better_display_point(point: Point) -> String {
            format!("{point}")
        }

        #[test]
        fn test_parse_point() {
            let error = Point::try_from("42.0123456789abcdef").unwrap_err();
            assert_eq!(
                error,
                "failed to parse block header hash: Invalid string length"
            );
        }

        #[test]
        fn test_parse_real_point() {
            let point = Point::try_from(
                "70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d",
            )
            .unwrap();
            match point {
                Point::Specific(slot, _hash) => {
                    assert_eq!(70070379, slot.as_u64());
                }
                _ => panic!("expected a specific point"),
            }
        }
    }
}
