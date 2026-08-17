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

//! Wire form of a chain tip: a [`NetworkPoint`] plus block height.
//!
//! Ouroboros mini-protocols encode a tip as `[network_point, block_height]`. Convert to
//! [`Point`] as soon as the value is in memory.

use std::{
    fmt::{self, Debug, Display},
    str::FromStr,
};

use crate::{BlockHeight, HeaderHash, NetworkPoint, Point, Slot, cbor};

/// Wire and external form of a chain tip: a network point together with its block height.
///
/// [`NetworkPoint::Origin`] is stored with height 0.
#[derive(Clone, Copy, Eq, PartialEq, PartialOrd, Ord, Hash)]
pub struct NetworkTip(NetworkPoint, BlockHeight);

impl NetworkTip {
    /// Construct a [`NetworkTip`]. [`NetworkPoint::Origin`] always stores height 0.
    pub fn new(point: NetworkPoint, block_height: BlockHeight) -> Self {
        match point {
            NetworkPoint::Origin => Self(NetworkPoint::Origin, BlockHeight::from(0)),
            NetworkPoint::Specific(_, _) => Self(point, block_height),
        }
    }

    pub fn origin() -> Self {
        Self::new(NetworkPoint::Origin, BlockHeight::from(0))
    }

    pub fn point(&self) -> NetworkPoint {
        self.0
    }

    pub fn slot(&self) -> Slot {
        self.0.slot_or_default()
    }

    pub fn hash(&self) -> HeaderHash {
        self.0.hash()
    }

    pub fn block_height(&self) -> BlockHeight {
        self.1
    }
}

impl Default for NetworkTip {
    fn default() -> Self {
        Self::origin()
    }
}

impl Debug for NetworkTip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NetworkTip({:?}, {})", self.0, self.1)
    }
}

impl Display for NetworkTip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        Display::fmt(&Point::from(*self), f)
    }
}

impl From<Point> for NetworkTip {
    fn from(point: Point) -> Self {
        NetworkTip::from(&point)
    }
}

impl From<&Point> for NetworkTip {
    fn from(point: &Point) -> Self {
        NetworkTip::new(NetworkPoint::from(point), point.block_height())
    }
}

impl From<NetworkTip> for Point {
    fn from(tip: NetworkTip) -> Self {
        Point::from(&tip)
    }
}

impl From<&NetworkTip> for Point {
    fn from(tip: &NetworkTip) -> Self {
        tip.0.with_height(tip.1)
    }
}

impl TryFrom<&str> for NetworkTip {
    type Error = String;

    fn try_from(raw_str: &str) -> Result<Self, Self::Error> {
        Point::try_from(raw_str).map(NetworkTip::from)
    }
}

impl FromStr for NetworkTip {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::try_from(s)
    }
}

/// CBOR encoding matches the Ouroboros tip wire form: `[network_point, block_height]`.
impl cbor::encode::Encode<()> for NetworkTip {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::encode::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode(self.0)?;
        e.encode(self.1)?;
        Ok(())
    }
}

impl<'b> cbor::decode::Decode<'b, ()> for NetworkTip {
    fn decode(d: &mut cbor::decode::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        cbor::check_tagged_array_length(0, len, 2)?;
        let network_point = d.decode::<NetworkPoint>()?;
        let block_height = d.decode::<BlockHeight>()?;
        Ok(NetworkTip::new(network_point, block_height))
    }
}

impl serde::Serialize for NetworkTip {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for NetworkTip {
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

    use crate::{NetworkTip, any_block_height, any_network_point, prop_cbor_roundtrip};

    prop_cbor_roundtrip!(NetworkTip, any_network_tip());

    prop_compose! {
        pub fn any_network_tip()(
            point in any_network_point(),
            block_height in any_block_height(),
        ) -> NetworkTip {
            NetworkTip::new(point, block_height)
        }
    }

    #[cfg(test)]
    mod internal {
        use test_case::test_case;

        use super::*;
        use crate::{BlockHeight, Hash, NetworkPoint, Point, Slot};

        const SAMPLE_HASH: [u8; 32] = [
            254, 252, 156, 3, 124, 63, 156, 139, 79, 183, 138, 155, 15, 19, 123, 94, 208, 128, 60, 61, 70, 189, 45, 14,
            64, 197, 159, 169, 12, 160, 2, 193,
        ];

        fn sample_specific(height: u64) -> NetworkTip {
            NetworkTip::new(NetworkPoint::Specific(Slot::from(42), Hash::new(SAMPLE_HASH)), BlockHeight::from(height))
        }

        #[test_case(NetworkTip::origin() => "NetworkTip(Origin, 0)")]
        #[test_case(
            sample_specific(7) => "NetworkTip(Specific(42, fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1), 7)";
            "specific"
        )]
        fn better_debug_network_tip(tip: NetworkTip) -> String {
            format!("{tip:?}")
        }

        #[test_case(NetworkTip::origin() => "origin"; "origin")]
        #[test_case(
            sample_specific(7) => "42.fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1(7)";
            "specific"
        )]
        fn better_display_network_tip(tip: NetworkTip) -> String {
            format!("{tip}")
        }

        #[test]
        fn origin_normalizes_height() {
            let tip = NetworkTip::new(NetworkPoint::Origin, BlockHeight::from(99));
            assert_eq!(tip, NetworkTip::origin());
            assert_eq!(tip.block_height(), BlockHeight::from(0));
        }

        #[test]
        fn from_point_roundtrip() {
            let point = Point::Specific(Slot::from(42), Hash::new(SAMPLE_HASH), BlockHeight::from(7));
            assert_eq!(point, Point::from(NetworkTip::from(point)));
        }

        #[test]
        fn json() {
            let tip_str = "42.fefc9c037c3f9c8b4fb78a9b0f137b5ed0803c3d46bd2d0e40c59fa90ca002c1(7)";
            let tip = NetworkTip::try_from(tip_str).expect("failed to parse from string");
            let tip_json = serde_json::to_string(&tip).expect("failed to serialize");
            assert_eq!(format!("\"{tip_str}\""), tip_json);
            assert_eq!(tip, serde_json::from_str(&tip_json).expect("failed to deserialize"));
        }
    }
}
