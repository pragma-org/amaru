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

use blake2::{
    Blake2b,
    digest::{
        Digest as _,
        consts::{U28, U32},
    },
};

use crate::Hash;

enum Blake2bHasher {
    Bits224(Blake2b<U28>),
    Bits256(Blake2b<U32>),
}

pub struct Hasher<const BITS: usize>(Blake2bHasher);

impl<const BITS: usize> Hasher<BITS> {
    /// update the [`Hasher`] with the given inputs
    #[inline]
    pub fn input(&mut self, bytes: &[u8]) {
        match &mut self.0 {
            Blake2bHasher::Bits224(hasher) => hasher.update(bytes),
            Blake2bHasher::Bits256(hasher) => hasher.update(bytes),
        }
    }
}

macro_rules! sized_hasher {
    ($size:literal, $variant:ident) => {
        impl Hasher<$size> {
            /// create a new [`Hasher`]
            #[inline]
            pub fn new() -> Self {
                Self(Blake2bHasher::$variant(Blake2b::new()))
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
            pub fn finalize(self) -> Hash<{ $size / 8 }> {
                let Blake2bHasher::$variant(hasher) = self.0 else {
                    unreachable!("hasher output size must match its BLAKE2b state")
                };

                // BLAKE2b is configured for exactly this digest size, so the conversion cannot fail.
                Hash::try_from(hasher.finalize().as_slice())
                    .unwrap_or_else(|_| unreachable!("BLAKE2b digest must be {} bytes", $size / 8))
            }
        }

        impl Default for Hasher<$size> {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

sized_hasher!(224, Bits224);
sized_hasher!(256, Bits256);
