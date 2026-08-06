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

use thiserror::Error;

use crate::{Ed25519Signature, VKey, cbor, ed25519};

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize, cbor::Encode, cbor::Decode)]
pub struct VKeyWitness {
    #[n(0)]
    pub vkey: VKey,

    #[n(1)]
    pub signature: Ed25519Signature,
}

#[derive(Debug, Error)]
#[error("invalid signature for given key")]
pub struct InvalidEd25519Signature;

#[expect(clippy::expect_used, reason = "witness sizes are guaranteed by transaction decoding")]
pub fn verify_ed25519_signature(
    vkey: &VKey,
    signature: &Ed25519Signature,
    message: &[u8],
) -> Result<(), InvalidEd25519Signature> {
    // Key and signature lengths are enforced when the transaction is decoded, so these sized
    // conversions cannot fail for a witness coming from a decoded transaction.
    let public_key =
        ed25519::VerifyingKey::try_from(vkey.as_slice()).expect("key size is guaranteed by transaction decoding");

    let signature = ed25519::Signature::try_from(signature.as_slice())
        .expect("signature size is guaranteed by transaction decoding");

    public_key.verify_strict(message, &signature).map_err(|_| InvalidEd25519Signature)
}
