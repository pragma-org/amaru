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

use amaru_kernel::{HeaderHash, ORIGIN_HASH, Point, cbor};
use amaru_ouroboros_traits::is_header::IsHeader;
use serde::{Deserialize, Serialize};
use std::fmt::{Debug, Display, Formatter};

#[derive(PartialEq, Eq, Clone, Default, Serialize, Deserialize)]
pub enum Tip<H> {
    #[default]
    Genesis,
    Hdr(H),
}

impl<H> From<H> for Tip<H> {
    fn from(h: H) -> Self {
        Tip::Hdr(h)
    }
}

impl<H: IsHeader> Debug for Tip<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tip")
            .field("slot", &self.slot())
            .field("hash", &self.hash())
            .field("parent_hash", &self.parent())
            .finish()
    }
}

impl<H: IsHeader + Display> Display for Tip<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Tip::Genesis => f.write_str("Genesis"),
            Tip::Hdr(h) => Display::fmt(h, f),
        }
    }
}

impl<H: IsHeader> Tip<H> {
    /// Return the header for this Tip if the Tip doesn't represent the Genesis header.
    pub fn to_header(&self) -> Option<&H> {
        match self {
            Tip::Genesis => None,
            Tip::Hdr(h) => Some(h),
        }
    }

    pub fn block_height(&self) -> u64 {
        match self {
            Tip::Genesis => 0,
            Tip::Hdr(h) => h.block_height(),
        }
    }
}

impl<H: IsHeader, C> cbor::encode::Encode<C> for Tip<H>
where
    H: cbor::encode::Encode<C> + IsHeader,
{
    fn encode<W: cbor::encode::Write>(
        &self,
        e: &mut cbor::Encoder<W>,
        ctx: &mut C,
    ) -> Result<(), cbor::encode::Error<W::Error>> {
        match self {
            Tip::Genesis => e.encode(0).map(|_| ()),
            Tip::Hdr(header) => header.encode(e, ctx),
        }
    }
}

impl<H: IsHeader> Tip<H> {
    pub fn is_parent_of(&self, header: &Tip<H>) -> bool {
        match (header.parent(), self) {
            (None, Tip::Genesis) => true,
            (Some(p), Tip::Hdr(hdr)) => p == hdr.hash(),
            _ => false,
        }
    }
}
impl<H: IsHeader> IsHeader for Tip<H> {
    fn hash(&self) -> HeaderHash {
        match self {
            Tip::Genesis => ORIGIN_HASH,
            Tip::Hdr(header) => header.hash(),
        }
    }

    fn point(&self) -> Point {
        match self {
            Tip::Genesis => Point::Origin,
            Tip::Hdr(header) => header.point(),
        }
    }

    fn parent(&self) -> Option<HeaderHash> {
        match self {
            Tip::Genesis => None,
            Tip::Hdr(header) => header.parent(),
        }
    }

    fn block_height(&self) -> u64 {
        match self {
            Tip::Genesis => 0,
            Tip::Hdr(header) => header.block_height(),
        }
    }

    fn slot(&self) -> u64 {
        match self {
            Tip::Genesis => 0,
            Tip::Hdr(header) => header.slot(),
        }
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        match self {
            Tip::Genesis => vec![],
            Tip::Hdr(header) => header.extended_vrf_nonce_output(),
        }
    }
}

impl<H: IsHeader> From<Option<H>> for Tip<H> {
    fn from(tip: Option<H>) -> Tip<H> {
        match tip {
            Some(header) => Tip::from(header),
            None => Tip::Genesis,
        }
    }
}

/// This is equivalent to pallas_network::miniprotocols::chainsync::protocol::Tip
/// but does not incur the dependency on pallas_network.
///
/// This can also replace Tip<Header> in places where we only need the Point and block height,
/// without needing the full Header.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HeaderTip(Point, u64);

impl Display for HeaderTip {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}", self.1, self.0.hash())
    }
}

impl HeaderTip {
    pub fn new(point: Point, block_height: u64) -> Self {
        Self(point, block_height)
    }

    pub fn hash(&self) -> HeaderHash {
        self.0.hash()
    }

    pub fn point(&self) -> Point {
        self.0.clone()
    }

    pub fn block_height(&self) -> u64 {
        self.1
    }
}

pub trait AsHeaderTip {
    fn as_header_tip(&self) -> HeaderTip;
}

impl<H: IsHeader> AsHeaderTip for H {
    fn as_header_tip(&self) -> HeaderTip {
        HeaderTip::new(self.point(), self.block_height())
    }
}
