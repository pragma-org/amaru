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

//! Wire and external form of a chain point: slot and header hash, without block height.
//!
//! Ouroboros mini-protocols and some external interfaces (archive names, snapshot keys, peer
//! snapshots, CLI arguments) identify a block by `slot.hash` only. Convert to [`Point`] as soon as
//! the block height is known.

use std::{
    fmt::{self, Debug, Display},
    str::FromStr,
};

use crate::{BlockHeight, Hash, HeaderHash, ORIGIN_HASH, Point, Slot, cbor, size::HEADER};

#[derive(Default, Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub enum NetworkPoint {
    #[default]
    Origin,
    Specific(Slot, HeaderHash),
}

impl NetworkPoint {
    pub fn slot_or_default(&self) -> Slot {
        match self {
            NetworkPoint::Origin => Slot::from(0),
            NetworkPoint::Specific(slot, _) => *slot,
        }
    }

    pub fn hash(&self) -> HeaderHash {
        match self {
            // By convention, the hash of `Genesis` is all 0s.
            NetworkPoint::Origin => ORIGIN_HASH,
            NetworkPoint::Specific(_, header_hash) => *header_hash,
        }
    }

    /// Attach a block height to produce a full [`Point`].
    ///
    /// [`NetworkPoint::Origin`] stays origin; the height is ignored.
    pub fn with_height(self, height: BlockHeight) -> Point {
        match self {
            NetworkPoint::Origin => Point::Origin,
            NetworkPoint::Specific(slot, hash) => Point::Specific(slot, hash, height),
        }
    }
}

impl Debug for NetworkPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkPoint::Origin => write!(f, "Origin"),
            NetworkPoint::Specific(slot, _hash) => write!(f, "Specific({slot}, {})", self.hash()),
        }
    }
}

impl Display for NetworkPoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NetworkPoint::Origin => write!(f, "origin"),
            NetworkPoint::Specific(slot, hash) => write!(f, "{slot}.{hash}"),
        }
    }
}

impl From<&NetworkPoint> for HeaderHash {
    fn from(point: &NetworkPoint) -> Self {
        point.hash()
    }
}

impl From<Point> for NetworkPoint {
    fn from(point: Point) -> Self {
        NetworkPoint::from(&point)
    }
}

impl From<&Point> for NetworkPoint {
    fn from(point: &Point) -> Self {
        match *point {
            Point::Origin => NetworkPoint::Origin,
            Point::Specific(slot, hash, _) => NetworkPoint::Specific(slot, hash),
        }
    }
}

/// Parse a network point from a string.
///
/// Expects `origin` or `<slot>.<hash>`, where `<slot>` is a number and `<hash>` is a hex-encoded
/// 32-byte hash.
impl TryFrom<&str> for NetworkPoint {
    type Error = String;

    fn try_from(raw_str: &str) -> Result<Self, Self::Error> {
        if raw_str == "origin" {
            return Ok(NetworkPoint::Origin);
        }

        let mut split = raw_str.split('.');

        let slot = split
            .next()
            .ok_or("missing slot number before '.'")
            .and_then(|s| s.parse::<u64>().map_err(|_| "failed to parse point's slot as a non-negative integer"))?;

        let block_header_hash = split
            .next()
            .ok_or("missing block header hash after '.'".to_string())
            .and_then(|s| s.parse::<HeaderHash>().map_err(|e| format!("failed to parse block header hash: {}", e)))?;

        Ok(NetworkPoint::Specific(Slot::from(slot), block_header_hash))
    }
}

impl FromStr for NetworkPoint {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

impl cbor::encode::Encode<()> for NetworkPoint {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::encode::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            NetworkPoint::Origin => e.array(0)?,
            NetworkPoint::Specific(slot, hash) => e.array(2)?.encode(slot)?.encode(hash)?,
        };

        Ok(())
    }
}

impl<'b> cbor::decode::Decode<'b, ()> for NetworkPoint {
    fn decode(d: &mut cbor::decode::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let size = d.array()?;

        match size {
            Some(0) => Ok(NetworkPoint::Origin),
            Some(2) => {
                let slot = d.decode()?;
                let hash = cbor::decode_bytes(d)?;
                if hash.len() != HEADER {
                    return Err(cbor::decode::Error::message("header hash must be 32 bytes"));
                }
                Ok(NetworkPoint::Specific(slot, Hash::from(&hash[..])))
            }
            _ => Err(cbor::decode::Error::message("can't decode NetworkPoint from array of size")),
        }
    }
}

impl serde::Serialize for NetworkPoint {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        //TODO RK: Consider using a more compact representation for JSON, e.g. an object with `slot` and `hash` fields.
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for NetworkPoint {
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

    use crate::{NetworkPoint, Slot, any_header_hash, prop_cbor_roundtrip};

    prop_cbor_roundtrip!(NetworkPoint, any_network_point());

    prop_compose! {
        fn any_slot()(n in 0u64..=1000) -> Slot {
            Slot::from(n)
        }
    }

    prop_compose! {
        pub fn any_specific_network_point()(slot in any_slot(), header_hash in any_header_hash()) -> NetworkPoint {
            NetworkPoint::Specific(slot, header_hash)
        }
    }

    pub fn any_network_point() -> impl Strategy<Value = NetworkPoint> {
        prop_oneof![
            1 => Just(NetworkPoint::Origin),
            3 => any_specific_network_point(),
        ]
    }

    #[cfg(test)]
    mod internal {
        use test_case::test_case;

        use super::*;
        use crate::Hash;

        #[test_case(NetworkPoint::Origin => "Origin")]
        #[test_case(
            NetworkPoint::Specific(
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
        fn better_debug_network_point(point: NetworkPoint) -> String {
            format!("{point:?}")
        }

        #[test_case(
            NetworkPoint::Origin => "origin";
           "origin"
        )]
        #[test_case(
            NetworkPoint::Specific(
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
        fn better_display_network_point(point: NetworkPoint) -> String {
            format!("{point}")
        }

        #[test]
        fn test_parse_network_point() {
            let error = NetworkPoint::try_from("42.0123456789abcdef").unwrap_err();
            assert_eq!(error, "failed to parse block header hash: Invalid string length");
        }

        #[test]
        fn json() {
            let point_str = "42.fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1";
            let point = NetworkPoint::try_from(point_str).expect("failed to parse from string");
            let point_json = serde_json::to_string(&point).expect("failed to serialize");
            assert_eq!(format!("\"{point_str}\""), point_json);
            assert_eq!(point, serde_json::from_str(&point_json).expect("failed to deserialize"));
        }

        #[test]
        fn test_parse_real_network_point() {
            let point =
                NetworkPoint::try_from("70070379.d6fe6439aed8bddc10eec22c1575bf0648e4a76125387d9e985e9a3f8342870d")
                    .unwrap();
            match point {
                NetworkPoint::Specific(slot, _hash) => {
                    assert_eq!(70070379, slot.as_u64());
                }
                _ => panic!("expected a specific network point"),
            }
        }
    }
}
