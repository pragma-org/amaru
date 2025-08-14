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

use amaru_kernel::{cbor, Point, HEADER_HASH_SIZE};
use amaru_ouroboros_traits::is_header::IsHeader;
use pallas_crypto::hash::Hash;
use std::fmt::{Debug, Formatter};

#[derive(PartialEq, Clone)]
pub enum Tip<H> {
    Genesis,
    Hdr(H),
}

impl<H: IsHeader> Debug for Tip<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Tip")
            .field("slot", &self.slot())
            .field("hash", &self.hash())
            .finish()
    }
}

impl<H> Tip<H> {
    /// Return the header for this Tip if the Tip doesn't represent the Genesis header.
    pub fn to_header(&self) -> Option<&H> {
        match self {
            Tip::Genesis => None,
            Tip::Hdr(h) => Some(h),
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
    fn hash(&self) -> Hash<HEADER_HASH_SIZE> {
        match self {
            Tip::Genesis => Hash::from([0; HEADER_HASH_SIZE]),
            Tip::Hdr(header) => header.hash(),
        }
    }

    fn point(&self) -> Point {
        match self {
            Tip::Genesis => Point::Origin,
            Tip::Hdr(header) => header.point(),
        }
    }

    fn parent(&self) -> Option<Hash<HEADER_HASH_SIZE>> {
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
            Some(header) => Tip::Hdr(header),
            None => Tip::Genesis,
        }
    }
}
