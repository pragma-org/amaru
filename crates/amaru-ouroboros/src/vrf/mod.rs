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

use std::{array::TryFromSliceError, ops::Deref};

use amaru_kernel::{
    Hash, Hasher, Slot,
    cardano::text_envelope::{FromTextEnvelope, TextEnvelopeError},
    cbor,
};
use thiserror::Error;
use vrf_dalek::{
    errors::VrfError,
    vrf03::{PublicKey03, SecretKey03, VrfProof03},
};
use zeroize::Zeroizing;

// ------------------------------------------------------------------- SecretKey

/// A VRF secret key.
pub struct SecretKey(SecretKey03);

impl SecretKey {
    /// Size of the VRF seed, in bytes.
    pub const SIZE: usize = 32;

    /// Bytes in a cardano-cli signing-key file: the seed followed by the verification key.
    const FILE_LEN: usize = Self::SIZE + PublicKey::SIZE;

    /// Raw signing seed.
    pub fn to_bytes(&self) -> [u8; Self::SIZE] {
        self.0.to_bytes()
    }
}

/// A CBOR byte string of 64 bytes, as cardano-cli writes it: the 32-byte seed followed by the verification key.
impl<'b, C> cbor::Decode<'b, C> for SecretKey {
    fn decode(d: &mut cbor::Decoder<'b>, _ctx: &mut C) -> Result<Self, cbor::decode::Error> {
        let raw = cbor::decode_bytes(d)?;
        let Ok(raw) = <&[u8; SecretKey::FILE_LEN]>::try_from(raw.as_ref()) else {
            return Err(cbor::decode::Error::message(SecretKeyError::InvalidSecretKeySize(raw.len())));
        };
        let mut seed = Zeroizing::new([0u8; SecretKey::SIZE]);
        seed.copy_from_slice(&raw[..SecretKey::SIZE]);
        let key = SecretKey::from(&*seed);
        if PublicKey::from(&key).as_ref() != &raw[SecretKey::SIZE..] {
            return Err(cbor::decode::Error::message(SecretKeyError::VerificationKeyMismatch));
        }
        Ok(key)
    }
}

impl FromTextEnvelope for SecretKey {
    const TYPES: &'static [&'static str] = &["VrfSigningKey_PraosVRF"];
    type Buffer = Zeroizing<Vec<u8>>;
    type Error = SecretKeyError;

    fn decode_cbor(_type: &'static str, decoder: &mut cbor::Decoder<'_>) -> Result<Self, SecretKeyError> {
        Ok(decoder.decode().map_err(TextEnvelopeError::Decode)?)
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

impl From<&[u8; Self::SIZE]> for SecretKey {
    fn from(slice: &[u8; Self::SIZE]) -> Self {
        SecretKey(SecretKey03::from_bytes(slice))
    }
}

impl TryFrom<&[u8]> for SecretKey {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self::from(<&[u8; Self::SIZE]>::try_from(slice)?))
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

impl Deref for PublicKey {
    type Target = [u8; PublicKey::SIZE];

    fn deref(&self) -> &Self::Target {
        self.0.as_bytes()
    }
}

impl From<&[u8; Self::SIZE]> for PublicKey {
    fn from(slice: &[u8; Self::SIZE]) -> Self {
        PublicKey(PublicKey03::from_bytes(slice))
    }
}

impl TryFrom<&[u8]> for PublicKey {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self::from(<&[u8; Self::SIZE]>::try_from(slice)?))
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
    pub fn new(absolute_slot: Slot, epoch_nonce: &Hash<32>) -> Self {
        let mut hasher = Hasher::<{ 8 * Self::SIZE }>::new();
        hasher.input(&u64::from(absolute_slot).to_be_bytes());
        hasher.input(epoch_nonce.as_ref());
        Input(hasher.finalize())
    }

    #[cfg(test)]
    /// Generate an arbitrary input challenge filled with random bytes.
    pub fn arbitrary() -> Self {
        use rand::{Rng, rng};
        let mut challenge = [0u8; Self::SIZE];
        rng().fill(&mut challenge);
        Input(challenge.into())
    }
}

impl AsRef<[u8]> for Input {
    fn as_ref(&self) -> &[u8] {
        self.0.as_ref()
    }
}

impl Deref for Input {
    type Target = Hash<32>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl From<&[u8; Self::SIZE]> for Input {
    fn from(slice: &[u8; Self::SIZE]) -> Self {
        Input(Hash::<{ Self::SIZE }>::from(*slice))
    }
}

impl TryFrom<&[u8]> for Input {
    type Error = TryFromSliceError;

    fn try_from(slice: &[u8]) -> Result<Self, Self::Error> {
        Ok(Input::from(<&[u8; Self::SIZE]>::try_from(slice)?))
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

    /// Verify a proof signature with a vrf public key. This will return a hash to compare with the original
    /// signature hash, but any non-error result is considered a successful verification without needing
    /// to do the extra comparison check.
    pub fn verify(&self, public_key: &PublicKey, input: &Input) -> Result<Hash<{ Self::HASH_SIZE }>, ProofVerifyError> {
        Ok(Hash::from(self.0.verify(&public_key.0, input.as_ref())?))
    }
}

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProofFromBytesError {
    #[error("Decompression from Edwards point failed.")]
    DecompressionFailed,
}

impl TryFrom<&[u8; Self::SIZE]> for Proof {
    type Error = ProofFromBytesError;

    #[expect(clippy::wildcard_enum_match_arm)]
    fn try_from(slice: &[u8; Self::SIZE]) -> Result<Self, Self::Error> {
        Ok(Proof(VrfProof03::from_bytes(slice).map_err(|e| match e {
            VrfError::DecompressionFailed => ProofFromBytesError::DecompressionFailed,
            _ => unreachable!("Other error than decompression failure found when deserialising proof: {e:?}"),
        })?))
    }
}

impl From<&Proof> for [u8; Proof::SIZE] {
    fn from(proof: &Proof) -> Self {
        proof.0.to_bytes()
    }
}

impl From<&Proof> for Hash<{ Proof::HASH_SIZE }> {
    fn from(proof: &Proof) -> Hash<{ Proof::HASH_SIZE }> {
        Hash::from(proof.0.proof_to_hash())
    }
}

// --------------------------------------------------------------- VrfDerivation

pub enum Derivation {
    Leader,
    Nonce,
}

impl Derivation {
    pub fn derive_tagged_vrf_output(self, block_vrf_output_bytes: &[u8]) -> Vec<u8> {
        let mut tagged_vrf: Vec<u8> = match self {
            Self::Leader => vec![0x4C_u8], /* "L" */
            Self::Nonce => vec![0x4E_u8],  /* "N" */
        };
        tagged_vrf.extend(block_vrf_output_bytes);
        Hasher::<256>::hash(&tagged_vrf).to_vec()
    }
}

// ---------------------------------------------------------------------- Errors

/// Failure to load a cardano-cli VRF signing key.
#[derive(Error, Debug)]
pub enum SecretKeyError {
    #[error(
        "VRF signing key must be {FILE_LEN} bytes (32-byte seed followed by its verification key), found {0}",
        FILE_LEN = SecretKey::FILE_LEN
    )]
    InvalidSecretKeySize(usize),
    #[error("VRF signing key verification key does not match the seed")]
    VerificationKeyMismatch,
    #[error("VRF key file: {0}")]
    Envelope(#[from] TextEnvelopeError),
}

/// error that can be returned if the verification of a [`VrfProof03`] fails
/// see [`VrfProof03::verify`]
#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[error("VRF proof verification failed: {:?}", .0)]
pub struct ProofVerifyError(
    #[from]
    #[source]
    #[serde(with = "serde_remote::VrfError")]
    VrfError,
);

mod serde_remote {
    #[derive(serde::Serialize, serde::Deserialize)]
    #[serde(remote = "super::VrfError")]
    pub enum VrfError {
        VerificationFailed,
        DecompressionFailed,
        PkSmallOrder,
        VrfOutputInvalid,
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{cardano::text_envelope, cbor};
    use proptest::prelude::*;
    use serde::Deserialize;

    use super::*;

    // Necessary to avoid defining a 'Debug' instance on SecretKey that would be leaky. It's only
    // needed for test, so appears here.
    struct WrappedSecretKey(SecretKey);
    impl std::fmt::Debug for WrappedSecretKey {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
            write!(f, "{}", hex::encode(self.0.0.to_bytes()))
        }
    }

    prop_compose! {
        fn any_secret_key()(bytes in proptest::array::uniform32(any::<u8>())) -> WrappedSecretKey {
            WrappedSecretKey(SecretKey::from(&bytes))
        }
    }

    prop_compose! {
        fn any_public_key()(bytes in proptest::array::uniform32(any::<u8>())) -> PublicKey {
            PublicKey::from(&bytes)
        }
    }

    prop_compose! {
        fn any_input()(bytes in proptest::array::uniform32(any::<u8>())) -> Input {
            Input::from(&bytes)
        }
    }

    prop_compose! {
        fn any_proof()(
            sk in any_secret_key(),
            input in any_input(),
        ) -> Proof {
            sk.0.prove(&input)
        }
    }

    proptest! {
        #[test]
        fn prop_verify_proof(
            WrappedSecretKey(sk) in any_secret_key(),
            input in any_input(),
        ) {
            let proof = sk.prove(&input);
            assert_eq!(
                proof.verify(&PublicKey::from(&sk), &input),
                Ok(Hash::<{ Proof::HASH_SIZE }>::from(&proof)),
            )
        }
    }

    proptest! {
        #[test]
        fn prop_verify_fail_on_invalid_key(
            WrappedSecretKey(sk) in any_secret_key(),
            input in any_input(),
            pk in any_public_key(),
        ) {
            let proof = sk.prove(&input);
            assert!(matches!(proof.verify(&pk, &input), Err(..)));
        }
    }

    proptest! {
        #[test]
        fn prop_verify_fail_on_invalid_proof(
            WrappedSecretKey(sk) in any_secret_key(),
            input in any_input(),
            proof in any_proof(),
        ) {
            assert!(matches!(proof.verify(&PublicKey::from(&sk), &input), Err(..)));
        }
    }

    #[test]
    fn golden_sk_to_pk() {
        let vrf_skey = SecretKey::try_from(
            hex::decode(
                "adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c\
                 90cd46a374",
            )
            .unwrap()
            .as_slice(),
        )
        .unwrap();

        let vrf_verification_key = PublicKey::try_from(
            hex::decode(
                "e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f\
                 9a7fda4381",
            )
            .unwrap()
            .as_slice(),
        )
        .unwrap();

        assert_eq!(vrf_verification_key, PublicKey::from(&vrf_skey),);
    }

    const VRF_SK_TYPE: &str = "VrfSigningKey_PraosVRF";
    const GOLDEN_SEED: &str = "adb9c97bec60189aa90d01d113e3ef405f03477d82a94f81da926c90cd46a374";
    const GOLDEN_VK: &str = "e0ff2371508ac339431b50af7d69cde0f120d952bb876806d3136f9a7fda4381";

    fn envelope(r#type: &str, cbor_hex: &str) -> String {
        format!(r#"{{"type":"{type}","description":"VRF Signing Key","cborHex":"{cbor_hex}"}}"#)
    }

    /// CBOR byte string of seed || verification key, as `cardano-cli conway node key-gen-VRF` writes it.
    fn golden_cbor_hex() -> String {
        format!("5840{GOLDEN_SEED}{GOLDEN_VK}")
    }

    #[test]
    fn deserializes_from_cardano_cli_vrf_skey() {
        let json = envelope(VRF_SK_TYPE, &golden_cbor_hex());
        let key: SecretKey = text_envelope::from_json(json.as_bytes()).unwrap();
        assert_eq!(hex::encode(key.to_bytes()), GOLDEN_SEED);
        assert_eq!(hex::encode(PublicKey::from(&key).as_ref()), GOLDEN_VK);
    }

    #[test]
    fn loads_from_vrf_skey_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vrf.skey");
        std::fs::write(&path, envelope(VRF_SK_TYPE, &golden_cbor_hex())).unwrap();
        let key: SecretKey = text_envelope::read(&path).unwrap();
        assert_eq!(hex::encode(key.to_bytes()), GOLDEN_SEED);
    }

    #[test]
    fn deserialize_rejects_wrong_type_and_truncated_cbor() {
        let wrong_type = envelope("VrfVerificationKey_PraosVRF", &golden_cbor_hex());
        let err = text_envelope::from_json::<SecretKey>(wrong_type.as_bytes()).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("unexpected text envelope type"), "{err}");

        // Definite byte string of 16 bytes, not seed || verification key.
        let short = envelope(VRF_SK_TYPE, &format!("5810{}", "00".repeat(16)));
        let err = text_envelope::from_json::<SecretKey>(short.as_bytes()).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("64 bytes"), "{err}");

        // The seed alone is not the file cardano-cli writes.
        let seed_only = envelope(VRF_SK_TYPE, &format!("5820{GOLDEN_SEED}"));
        let err = text_envelope::from_json::<SecretKey>(seed_only.as_bytes()).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("64 bytes"), "{err}");

        let mut mismatched = format!("{GOLDEN_SEED}{GOLDEN_VK}");
        mismatched.pop();
        mismatched.push('0');
        let mismatched = envelope(VRF_SK_TYPE, &format!("5840{mismatched}"));
        let err = text_envelope::from_json::<SecretKey>(mismatched.as_bytes()).map(|_| ()).unwrap_err().to_string();
        assert!(err.contains("verification key"), "{err}");
    }

    #[test]
    fn cardano_cli_vrf_skey_proves_against_its_vkey() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/forging/vrf.skey");
        let key: SecretKey = text_envelope::read(&path).unwrap();
        let vkey_bytes =
            envelope_byte_string(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data/forging/vrf.vkey"));
        assert_eq!(PublicKey::from(&key).as_ref(), vkey_bytes.as_slice());
        let input = Input::from(&[11u8; Input::SIZE]);
        let proof = key.prove(&input);
        proof.verify(&PublicKey::from(&key), &input).unwrap();
    }

    fn envelope_byte_string(path: &std::path::Path) -> Vec<u8> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Envelope {
            cbor_hex: String,
        }
        let envelope: Envelope = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let payload = hex::decode(envelope.cbor_hex).unwrap();
        let mut decoder = cbor::Decoder::new(&payload);
        cbor::decode_bytes(&mut decoder).unwrap().into_owned()
    }
}
