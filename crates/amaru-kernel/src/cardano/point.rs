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

use crate::{BlockHeight, HeaderHash, NetworkPoint, ORIGIN_HASH, Slot};

/// In-memory chain point: slot, header hash, and block height.
///
/// This type has no CBOR instances. Persist or send a [`crate::NetworkPoint`] (slot + hash) or a
/// [`crate::NetworkTip`] (`[network_point, block_height]`) instead.
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

/// CBOR / compact serde of a header hash inside [`Point`]: raw bytes when the serializer is
/// not human-readable, hex string otherwise.
struct PointHashBytes<'a>(&'a HeaderHash);

impl serde::Serialize for PointHashBytes<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if serializer.is_human_readable() {
            serializer.serialize_str(&self.0.to_string())
        } else {
            serializer.serialize_bytes(self.0.as_ref())
        }
    }
}

impl serde::Serialize for Point {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeTuple;

        match *self {
            Point::Origin => serializer.serialize_tuple(0)?.end(),
            Point::Specific(slot, hash, height) => {
                let mut seq = serializer.serialize_tuple(3)?;
                seq.serialize_element(&slot.as_u64())?;
                seq.serialize_element(&PointHashBytes(&hash))?;
                seq.serialize_element(&u64::from(height))?;
                seq.end()
            }
        }
    }
}

impl<'de> serde::Deserialize<'de> for Point {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_seq(PointVisitor)
    }
}

struct PointVisitor;

impl<'de> serde::de::Visitor<'de> for PointVisitor {
    type Value = Point;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("an empty array (origin) or [slot, hash, height]")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let Some(slot) = seq.next_element::<u64>()? else {
            if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
                return Err(serde::de::Error::invalid_length(1, &self));
            }
            return Ok(Point::Origin);
        };

        let hash = seq.next_element::<PointHash>()?.ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;
        let height = seq.next_element::<u64>()?.ok_or_else(|| serde::de::Error::invalid_length(2, &self))?;
        if seq.next_element::<serde::de::IgnoredAny>()?.is_some() {
            return Err(serde::de::Error::invalid_length(4, &self));
        }

        Ok(Point::Specific(Slot::from(slot), hash.0, crate::BlockHeight::from(height)))
    }
}

struct PointHash(HeaderHash);

impl<'de> serde::Deserialize<'de> for PointHash {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_any(PointHashVisitor)
    }
}

struct PointHashVisitor;

impl<'de> serde::de::Visitor<'de> for PointHashVisitor {
    type Value = PointHash;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a 32-byte header hash as a hex string or a byte string")
    }

    fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
        HeaderHash::from_str(v).map(PointHash).map_err(serde::de::Error::custom)
    }

    fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Self::Value, E> {
        if v.len() != 32 {
            return Err(serde::de::Error::invalid_length(v.len(), &"32 bytes"));
        }
        Ok(PointHash(HeaderHash::from(v)))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: serde::de::SeqAccess<'de>,
    {
        let mut bytes = [0u8; 32];
        for (i, slot) in bytes.iter_mut().enumerate() {
            *slot = seq.next_element()?.ok_or_else(|| serde::de::Error::invalid_length(i, &self))?;
        }
        if seq.next_element::<u8>()?.is_some() {
            return Err(serde::de::Error::invalid_length(33, &self));
        }
        Ok(PointHash(HeaderHash::new(bytes)))
    }
}

impl schemars::JsonSchema for Point {
    fn schema_name() -> String {
        "Point".to_string()
    }

    fn json_schema(_gen: &mut schemars::r#gen::SchemaGenerator) -> schemars::schema::Schema {
        #[allow(clippy::expect_used)]
        serde_json::from_value(serde_json::json!({
            "description": "Origin is an empty array. Specific points are [slot, header hash, block height]. The hash is a hex string in JSON; CBOR transport uses a byte string.",
            "oneOf": [
                {
                    "type": "array",
                    "maxItems": 0
                },
                {
                    "type": "array",
                    "minItems": 3,
                    "maxItems": 3,
                    "prefixItems": [
                        { "type": "integer", "description": "slot" },
                        {
                            "type": "string",
                            "contentEncoding": "hex",
                            "minLength": 64,
                            "maxLength": 64,
                            "description": "header hash"
                        },
                        { "type": "integer", "description": "block height" }
                    ]
                }
            ]
        }))
        .expect("point json schema is valid")
    }

    fn is_referenceable() -> bool {
        false
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use proptest::prelude::*;

    use crate::{Point, Slot, any_block_height, any_header_hash};

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
            assert_eq!(point_json, "[42,\"fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1\",7]");
            assert_eq!(point, serde_json::from_str(&point_json).expect("failed to deserialize"));
            assert_eq!(serde_json::to_string(&Point::Origin).expect("origin"), "[]");
            assert_eq!(Point::Origin, serde_json::from_str("[]").expect("origin"));
        }

        #[test]
        fn cbor_specific_encodes_hash_as_byte_string() {
            let point = sample_specific(7);
            let mut buf = Vec::new();
            cbor4ii::serde::to_writer(&mut buf, &point).expect("encode");
            let decoded: Point = cbor4ii::serde::from_slice(&buf).expect("decode");
            assert_eq!(decoded, point);

            // CBOR major type 2, 32-byte header: 0x58 0x20. A hex *text* string would be 0x78 0x40.
            assert!(buf.windows(2).any(|w| w == [0x58, 0x20]), "hash must be a definite 32-byte CBOR byte string");
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
