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

use crate::Bytes;
use std::{array::TryFromSliceError, ops::Deref};

/// Turn any Bytes-like structure into a sized slice. Useful for crypto operation requiring
/// operands with specific bytes sizes. For example:
///
/// # ```
/// # let public_key: [u8; ed25519::PublicKey::SIZE] = into_sized_array(vkey, |error, expected| {
/// #     InvalidVKeyWitness::InvalidKeySize { error, expected }
/// # })?;
/// # ```
pub fn into_sized_array<const SIZE: usize, E, T>(
    bytes: T,
    into_error: impl Fn(TryFromSliceError, usize) -> E,
) -> Result<[u8; SIZE], E>
where
    T: Deref<Target = Bytes>,
{
    bytes
        .deref()
        .as_slice()
        .try_into()
        .map_err(|e| into_error(e, SIZE))
}
