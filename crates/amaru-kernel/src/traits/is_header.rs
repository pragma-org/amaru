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

use crate::{BlockHeight, HeaderHash, Point, Slot, Tip, cbor};

/// Interface to a header for the purpose of chain selection.
pub trait IsHeader: cbor::Encode<()> + Sized {
    /// Hash of the header
    ///
    /// This is used to identify the header in the chain selection.
    /// Header hash is expected to be unique for each header, eg.
    /// $h \neq h' \logeq hhash() \new h'.hash()$.
    fn hash(&self) -> HeaderHash;

    /// Point to this header
    fn point(&self) -> Point {
        Point::Specific(self.slot(), self.hash())
    }

    /// Parent hash of the header
    /// Not all headers have a parent, eg. genesis block.
    fn parent(&self) -> Option<HeaderHash>;

    /// Block height of the header w.r.t genesis block
    fn block_height(&self) -> BlockHeight;

    /// Slot number of the header
    fn slot(&self) -> Slot;

    /// The raw vrf output from the header, which can then be derived for nonce or leader VRF
    /// computations.
    fn vrf_output(&self) -> &[u8];

    /// Return the header tip
    fn tip(&self) -> Tip {
        Tip::new(self.point(), self.block_height())
    }
}
