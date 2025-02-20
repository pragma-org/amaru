// Copyright 2024 PRAGMA
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

//! Utility functions for working with Verifiable Random Functions (a.k.a. VRF), according to:
//!
//! <https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-03>

pub use pallas_primitives::babbage::{derive_tagged_vrf_output, VrfDerivation as Derivation};
use std::array::TryFromSliceError;

use crate::{Hash, Hasher};
use thiserror::Error;
use vrf_dalek::{
    errors::VrfError,
    vrf03::{PublicKey03, SecretKey03, VrfProof03},
};

// ------------------------------------------------------------------- SecretKey

/// A VRF secret key.
pub struct SecretKey(SecretKey03);

impl SecretKey {
    /// Size of a VRF secret key, in bytes.
    pub const SIZE: usize = 32;
}

impl From<&[u8; Self::SIZE]> for SecretKey {
    fn from(slice: &[u8; Self::SIZE]) -> Self {
        SecretKey(SecretKey03::from_bytes(slice))
    }
}

impl SecretKey {
    /// Sign a challenge message value with a vrf secret key and produce a proof signature
    pub fn prove(&self, input: &Input) -> Proof {
        let pk = PublicKey03::from(&self.0);
        let proof = VrfProof03::generate(&pk, &self.0, input.as_ref());
        Proof(proof)
    }
}

// ------------------------------------------------------------------- PublicKey

/// A VRF public key.
#[derive(Debug, PartialEq)]
pub struct PublicKey(PublicKey03);

impl PublicKey {
    /// Size of a VRF public key, in bytes.
    pub const SIZE: usize = 32;

    /// Size of a VRF public key hash digest (Blake2b-256), in bytes.
    pub const HASH_SIZE: usize = 32;
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl TryFrom<&[u8]> for PublicKey {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(PublicKey(PublicKey03::from_bytes(slice.try_into()?)))
    }
}

impl From<&[u8; Self::SIZE]> for PublicKey {
    fn from(slice: &[u8; Self::SIZE]) -> Self {
        PublicKey(PublicKey03::from_bytes(slice))
    }
}

impl From<[u8; Self::SIZE]> for PublicKey {
    fn from(slice: [u8; Self::SIZE]) -> Self {
        Self::from(&slice)
    }
}

impl From<&SecretKey> for PublicKey {
    fn from(secret_key: &SecretKey) -> Self {
        PublicKey(PublicKey03::from(&secret_key.0))
    }
}

// ----------------------------------------------------------------------- Input

#[derive(Debug, PartialEq)]
pub struct Input(Hash<32>);

impl Input {
    /// Size of a VRF input challenge, in bytes
    pub const SIZE: usize = 32;

    /// Create a new input challenge from an absolute slot number and an epoch entropy (a.k.a η0)
    pub fn new(absolute_slot: u64, epoch_nonce: &Hash<32>) -> Self {
        let mut hasher = Hasher::<{ 8 * Self::SIZE }>::new();
        hasher.input(&absolute_slot.to_be_bytes());
        hasher.input(epoch_nonce.as_ref());
        Input(hasher.finalize())
    }

    #[cfg(test)]
    /// Generate an arbitrary input challenge filled with random bytes.
    pub fn arbitrary() -> Self {
        use rand::{thread_rng, Rng};
        let mut challenge = [0u8; Self::SIZE];
        thread_rng().fill(&mut challenge);
        Input(challenge.into())
    }
}

impl AsRef<[u8]> for Input {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

// ----------------------------------------------------------------------- Proof

/// A VRF proof formed by an Edward point and two scalars.
#[derive(Debug)]
pub struct Proof(VrfProof03);

impl Proof {
    /// Size of a VRF proof, in bytes.
    pub const SIZE: usize = 80;

    /// Size of a VRF proof hash digest (SHA512), in bytes.
    pub const HASH_SIZE: usize = 64;
}

#[derive(Error, Debug, PartialEq)]
pub enum ProofFromBytesError {
    #[error("Decompression from Edwards point failed.")]
    DecompressionFailed,
}

#[allow(clippy::expect_used)]
impl TryFrom<&[u8; Self::SIZE]> for Proof {
    type Error = ProofFromBytesError;

    fn try_from(slice: &[u8; Self::SIZE]) -> Result<Self, Self::Error> {
        Ok(Proof(VrfProof03::from_bytes(slice).map_err(
            |e| match e {
                VrfError::DecompressionFailed => ProofFromBytesError::DecompressionFailed,
                _ => unreachable!(
                    "Other error than decompression failure found when deserialising proof: {e:?}"
                ),
            },
        )?))
    }
}

impl Proof {
    /// Return the created proof signature
    pub fn signature(&self) -> [u8; Self::SIZE] {
        self.0.to_bytes()
    }

    /// Verify a proof signature with a vrf public key. This will return a hash to compare with the original
    /// signature hash, but any non-error result is considered a successful verification without needing
    /// to do the extra comparison check.
    pub fn verify(
        &self,
        public_key: &PublicKey,
        input: &Input,
    ) -> Result<Hash<{ Self::HASH_SIZE }>, ProofVerifyError> {
        Ok(Hash::from(self.0.verify(&public_key.0, input.as_ref())?))
    }
}

impl From<&Proof> for Hash<{ Proof::HASH_SIZE }> {
    fn from(proof: &Proof) -> Hash<{ Proof::HASH_SIZE }> {
        Hash::from(proof.0.proof_to_hash())
    }
}

// ---------------------------------------------------------------------- Errors

/// error that can be returned if the verification of a [`VrfProof`] fails
/// see [`VrfProof::verify`]
#[derive(Error, Debug, PartialEq)]
#[error("VRF proof verification failed: {:?}", .0)]
pub struct ProofVerifyError(
    #[from]
    #[source]
    VrfError,
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vrf_prove_and_verify() {
        let raw_vrf_skey: Vec<u8> = hex::decode(
            "adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a3\
             74e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda\
             4381",
        )
        .unwrap();

        let raw_vrf_vkey: Vec<u8> = hex::decode(
            "e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda43\
             81",
        )
        .unwrap();

        let vrf_skey = SecretKey::from(&raw_vrf_skey[..SecretKey::SIZE].try_into().unwrap());
        let vrf_vkey = PublicKey::from(
            &raw_vrf_vkey[..PublicKey::SIZE].try_into().unwrap() as &[u8; PublicKey::SIZE]
        );
        let calculated_vrf_vkey = PublicKey::from(&vrf_skey);

        assert_eq!(vrf_vkey.as_ref(), calculated_vrf_vkey.as_ref());

        // random challenge to sign with vrf_skey
        let input = Input::arbitrary();

        // create a proof signature and hash of the seed
        let proof = vrf_skey.prove(&input);
        let proof_hash = Hash::<{ Proof::HASH_SIZE }>::from(&proof);

        // verify the proof signature with the public vrf public key
        let verified_hash = proof.verify(&vrf_vkey, &input).unwrap();
        assert_eq!(proof_hash, verified_hash);
    }
}
