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

use std::{
    fmt::{self, Debug, Display},
    str::FromStr,
};

use crate::{BlockHeight, HeaderHash, NetworkPoint, ORIGIN_HASH, Slot, cbor};

#[derive(Default, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum Point {
    #[default]
    Origin,
    Specific(Slot, HeaderHash, BlockHeight),
}

impl Point {
    /// Construct a [`Point`] from a network point and a block height.
    ///
    /// [`NetworkPoint::Origin`] yields [`Point::Origin`]; the height is ignored.
    pub fn new(point: NetworkPoint, block_height: BlockHeight) -> Self {
        point.with_height(block_height)
    }

    pub fn slot_or_default(&self) -> Slot {
        match self {
            Point::Origin => Slot::from(0),
            Point::Specific(slot, _, _) => *slot,
        }
    }

    pub fn slot(&self) -> Slot {
        self.slot_or_default()
    }

    pub fn hash(&self) -> HeaderHash {
        match self {
            // By convention, the hash of `Genesis` is all 0s.
            Point::Origin => ORIGIN_HASH,
            Point::Specific(_, header_hash, _) => *header_hash,
        }
    }

    pub fn block_height(&self) -> BlockHeight {
        match self {
            Point::Origin => BlockHeight::from(0),
            Point::Specific(_, _, block_height) => *block_height,
        }
    }

    pub fn to_network_point(&self) -> NetworkPoint {
        NetworkPoint::from(self)
    }
}

impl Debug for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Point::Origin => write!(f, "Origin"),
            Point::Specific(slot, _hash, height) => write!(f, "Specific({slot}, {}, {height})", self.hash()),
        }
    }
}

impl Display for Point {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Point::Origin => write!(f, "origin"),
            Point::Specific(slot, hash, height) => write!(f, "{slot}.{hash}({height})"),
        }
    }
}

impl From<&Point> for HeaderHash {
    fn from(point: &Point) -> Self {
        point.hash()
    }
}

/// Parse a point from a string.
///
/// Expects `origin` or `<slot>.<hash>(<height>)`, with no space before the parentheses.
impl TryFrom<&str> for Point {
    type Error = String;

    fn try_from(raw_str: &str) -> Result<Self, Self::Error> {
        if raw_str == "origin" {
            return Ok(Point::Origin);
        }

        let (prefix, height_part) = raw_str.split_once('(').ok_or("missing '(' for block height")?;
        let height_str = height_part.strip_suffix(')').ok_or("missing ')' after block height")?;
        let height =
            height_str.parse::<u64>().map_err(|_| "failed to parse point's block height as a non-negative integer")?;

        let (slot_str, hash_str) = prefix.split_once('.').ok_or("missing slot number before '.'")?;
        let slot = slot_str.parse::<u64>().map_err(|_| "failed to parse point's slot as a non-negative integer")?;
        let block_header_hash =
            hash_str.parse::<HeaderHash>().map_err(|e| format!("failed to parse block header hash: {}", e))?;

        Ok(Point::Specific(Slot::from(slot), block_header_hash, BlockHeight::from(height)))
    }
}

impl FromStr for Point {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

/// CBOR encoding matches the Ouroboros tip wire form: `[network_point, block_height]`.
impl cbor::encode::Encode<()> for Point {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::encode::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode(self.to_network_point())?;
        e.encode(self.block_height())?;
        Ok(())
    }
}

impl<'b> cbor::decode::Decode<'b, ()> for Point {
    fn decode(d: &mut cbor::decode::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        cbor::check_tagged_array_length(0, len, 2)?;
        let network_point = d.decode::<NetworkPoint>()?;
        let block_height = d.decode::<BlockHeight>()?;
        Ok(network_point.with_height(block_height))
    }
}

impl serde::Serialize for Point {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for Point {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::from_str(<&str>::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{Point, Slot, any_block_height, any_header_hash, prop_cbor_roundtrip};

    prop_cbor_roundtrip!(Point, any_point());

    prop_compose! {
        fn any_slot()(n in 0u64..=1000) -> Slot {
            Slot::from(n)
        }
    }

    prop_compose! {
        pub fn any_specific_point()(
            slot in any_slot(),
            header_hash in any_header_hash(),
            block_height in any_block_height(),
        ) -> Point {
            Point::Specific(slot, header_hash, block_height)
        }
    }

    pub fn any_point() -> impl Strategy<Value = Point> {
        prop_oneof![
            1 => Just(Point::Origin),
            3 => any_specific_point(),
        ]
    }

    #[cfg(test)]
    mod internal {
        use test_case::test_case;

        use super::*;
        use crate::{BlockHeight, Hash};

        const SAMPLE_HASH: [u8; 32] = [
            254, 252, 156, 3, 124, 63, 156, 139, 79, 183, 138, 155, 15, 19, 123, 94, 208, 128, 60, 61, 70, 189, 45, 14,
            64, 197, 159, 169, 12, 160, 2, 193,
        ];

        fn sample_specific(height: u64) -> Point {
            Point::Specific(Slot::from(42), Hash::new(SAMPLE_HASH), BlockHeight::from(height))
        }

        #[test_case(Point::Origin => "Origin")]
        #[test_case(
            sample_specific(7) => "Specific(42, fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1, 7)";
            "specific"
        )]
        fn better_debug_point(point: Point) -> String {
            format!("{point:?}")
        }

        #[test_case(
            Point::Origin => "origin";
           "origin"
        )]
        #[test_case(
            sample_specific(7) => "42.fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1(7)";
            "specific"
        )]
        fn better_display_point(point: Point) -> String {
            format!("{point}")
        }

        #[test]
        fn test_parse_point() {
            let error = Point::try_from("42.0123456789abcdef").unwrap_err();
            assert_eq!(error, "missing '(' for block height");
        }

        #[test]
        fn json() {
            let point_str = "42.fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1(7)";
            let point = Point::try_from(point_str).expect("failed to parse from string");
            let point_json = serde_json::to_string(&point).expect("failed to serialize");
            assert_eq!(format!("\"{point_str}\""), point_json);
            assert_eq!(point, serde_json::from_str(&point_json).expect("failed to deserialize"));
        }

        #[test]
        fn test_parse_real_point() {
            let point =
                Point::try_from("70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d(123)")
                    .unwrap();
            match point {
                Point::Specific(slot, _hash, height) => {
                    assert_eq!(70070379, slot.as_u64());
                    assert_eq!(123, height.as_u64());
                }
                _ => panic!("expected a specific point"),
            }
        }

        #[test]
        fn ord_is_slot_then_hash_then_height() {
            let hash_lo = Hash::new([1; 32]);
            let hash_hi = Hash::new([2; 32]);
            let origin = Point::Origin;
            let slot1_lo_h10 = Point::Specific(Slot::from(1), hash_lo, BlockHeight::from(10));
            let slot1_lo_h11 = Point::Specific(Slot::from(1), hash_lo, BlockHeight::from(11));
            let slot1_hi_h1 = Point::Specific(Slot::from(1), hash_hi, BlockHeight::from(1));
            let slot2_lo_h1 = Point::Specific(Slot::from(2), hash_lo, BlockHeight::from(1));

            assert!(origin < slot1_lo_h10);
            assert!(slot1_lo_h10 < slot1_hi_h1, "same slot: hash is the second key");
            assert!(slot1_hi_h1 < slot2_lo_h1, "slot is the first key");
            assert!(slot1_lo_h10 < slot1_lo_h11, "same slot and hash: height is the third key");
        }
    }
}
