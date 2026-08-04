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

use cryptoxide::blake2b::Blake2b;

use crate::Hash;

pub struct Hasher<const BITS: usize>(Blake2b);

impl<const BITS: usize> Hasher<BITS> {
    /// update the [`Hasher`] with the given inputs
    #[inline]
    pub fn input(&mut self, bytes: &[u8]) {
        use cryptoxide::digest::Digest as _;
        self.0.input(bytes);
    }
}

macro_rules! sized_hasher {
    ($size:literal) => {
        impl Hasher<$size> {
            /// create a new [`Hasher`]
            #[inline]
            pub fn new() -> Self {
                Self(Blake2b::new($size / 8))
            }

            /// convenient function to directly generate the hash
            /// of the given bytes without creating the intermediary
            /// types [`Hasher`] and calling [`Hasher::input`].
            #[inline]
            pub fn hash(bytes: &[u8]) -> Hash<{ $size / 8 }> {
                let mut hasher = Self::new();
                hasher.input(bytes);
                hasher.finalize()
            }

            #[inline]
            pub fn hash_tagged(bytes: &[u8], tag: u8) -> Hash<{ $size / 8 }> {
                let mut hasher = Self::new();
                hasher.input(&[tag]);
                hasher.input(bytes);
                hasher.finalize()
            }

            /// consume the [`Hasher`] and returns the computed digest
            pub fn finalize(mut self) -> Hash<{ $size / 8 }> {
                use cryptoxide::digest::Digest as _;
                let mut hash = [0; $size / 8];
                self.0.result(&mut hash);
                Hash::new(hash)
            }
        }

        impl Default for Hasher<$size> {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

sized_hasher!(224);
sized_hasher!(256);
