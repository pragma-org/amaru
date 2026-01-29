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

use crate::{BlockHeight, HeaderHash, Point, Slot, cbor};
use std::fmt;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct Tip(Point, BlockHeight);

impl Tip {
    pub fn origin() -> Self {
        Self(Point::Origin, BlockHeight::from(0))
    }

    pub fn new(point: Point, block_height: BlockHeight) -> Self {
        Self(point, block_height)
    }

    pub fn point(&self) -> Point {
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

impl fmt::Display for Tip {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}", self.1, self.0.hash())
    }
}

impl cbor::Encode<()> for Tip {
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        _ctx: &mut (),
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        e.array(2)?;
        e.encode(self.0)?;
        e.encode(self.1)?;
        Ok(())
    }
}

impl<'b> cbor::Decode<'b, ()> for Tip {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut ()) -> Result<Self, cbor::decode::Error> {
        let len = d.array()?;
        cbor::check_tagged_array_length(0, len, 2)?;
        let point = d.decode()?;
        let block_num = d.decode()?;
        Ok(Tip(point, block_num))
    }
}

#[cfg(any(test, feature = "test-utils"))]
pub use tests::*;

#[cfg(any(test, feature = "test-utils"))]
mod tests {
    use super::*;
    use crate::{any_block_height, any_point, prop_cbor_roundtrip};
    use proptest::prop_compose;

    prop_cbor_roundtrip!(Tip, any_tip());

    prop_compose! {
        pub fn any_tip()(point in any_point(), block_height in any_block_height()) -> Tip {
            Tip::new(point, block_height)
        }
    }
}
