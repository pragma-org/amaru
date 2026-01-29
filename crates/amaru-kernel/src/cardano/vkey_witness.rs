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

use crate::{Bytes, utils::array::into_sized_array};
use pallas_crypto::key::ed25519;
use std::array::TryFromSliceError;
use thiserror::Error;

pub use pallas_primitives::conway::VKeyWitness;

#[derive(Debug, Error)]
pub enum InvalidEd25519Signature {
    #[error("invalid signature size: {error:?}")]
    InvalidSignatureSize {
        error: TryFromSliceError,
        expected: usize,
    },
    #[error("invalid verification key size: {error:?}")]
    InvalidKeySize {
        error: TryFromSliceError,
        expected: usize,
    },
    #[error("invalid signature for given key")]
    InvalidSignature,
}

pub fn verify_ed25519_signature(
    vkey: &Bytes,
    signature: &Bytes,
    message: &[u8],
) -> Result<(), InvalidEd25519Signature> {
    // TODO: vkey should come as sized bytes out of the serialization.
    // To be fixed upstream in Pallas.
    let public_key = ed25519::PublicKey::from(into_sized_array(vkey, |error, expected| {
        InvalidEd25519Signature::InvalidKeySize { error, expected }
    })?);

    // TODO: signature should come as sized bytes out of the serialization.
    // To be fixed upstream in Pallas.
    let signature = ed25519::Signature::from(into_sized_array(signature, |error, expected| {
        InvalidEd25519Signature::InvalidSignatureSize { error, expected }
    })?);

    if !public_key.verify(message, &signature) {
        Err(InvalidEd25519Signature::InvalidSignature)
    } else {
        Ok(())
    }
}
