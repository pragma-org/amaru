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

use pallas_crypto::key::ed25519;
pub use pallas_primitives::conway::VKeyWitness;
use thiserror::Error;

use crate::{Bytes, utils::array::into_sized_array};

#[derive(Debug, Error)]
#[error("invalid signature for given key")]
pub struct InvalidEd25519Signature;

#[expect(clippy::expect_used, reason = "witness sizes are guaranteed by transaction decoding")]
pub fn verify_ed25519_signature(
    vkey: &Bytes,
    signature: &Bytes,
    message: &[u8],
) -> Result<(), InvalidEd25519Signature> {
    // Key and signature lengths are enforced when the transaction is decoded, so these sized
    // conversions cannot fail for a witness coming from a decoded transaction.
    let public_key = ed25519::PublicKey::from(
        into_sized_array(vkey, |error, _| error).expect("key size is guaranteed by transaction decoding"),
    );
    let signature = ed25519::Signature::from(
        into_sized_array(signature, |error, _| error).expect("signature size is guaranteed by transaction decoding"),
    );

    if !public_key.verify(message, &signature) { Err(InvalidEd25519Signature) } else { Ok(()) }
}
