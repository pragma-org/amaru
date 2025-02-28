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

use amaru_kernel::{cbor, Hash, Hasher, Header, Point};

pub mod fake;

pub const HASH_SIZE: usize = 32;

/// Interface to a header for the purpose of chain selection.
pub trait IsHeader: cbor::Encode<()> + for<'d> cbor::Decode<'d, ()> {
    /// Hash of the header
    ///
    /// This is used to identify the header in the chain selection.
    /// Header hash is expected to be unique for each header, eg.
    /// $h \neq h' \logeq hhash() \new h'.hash()$.
    fn hash(&self) -> Hash<HASH_SIZE> {
        Hasher::<{ HASH_SIZE * 8 }>::hash_cbor(self)
    }

    /// Point to this header
    fn point(&self) -> Point {
        Point::Specific(self.slot(), self.hash().to_vec())
    }

    /// Parent hash of the header
    /// Not all headers have a parent, eg. genesis block.
    fn parent(&self) -> Option<Hash<HASH_SIZE>>;

    /// Block height of the header w.r.t genesis block
    fn block_height(&self) -> u64;

    /// Slot number of the header
    fn slot(&self) -> u64;

    /// The range-extended tagged nonce vrf output
    // TODO: Return type here should be a Hash<32>, but we cannot make this happen without either:
    // 1. Making this return a Result
    // 2. Use a panic
    // 3. Fix Pallas' leader_vrf_output to return a Hash<32> instead of a Vec.
    fn extended_vrf_nonce_output(&self) -> Vec<u8>;
}

/// Concrete Conway-era compatible `Header` implementation.
///
/// There's no difference in headers' structure between Babbage
/// and Conway era. The idea is that we only keep concrete the header from
/// the latest era, and convert other headers on the fly when needed.
impl IsHeader for Header {
    fn parent(&self) -> Option<Hash<HASH_SIZE>> {
        self.header_body.prev_hash
    }

    fn block_height(&self) -> u64 {
        self.header_body.block_number
    }

    fn slot(&self) -> u64 {
        self.header_body.slot
    }

    fn extended_vrf_nonce_output(&self) -> Vec<u8> {
        self.header_body.nonce_vrf_output()
    }
}
