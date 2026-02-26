//! This module implements `ECVRF-ED25519-SHA512-Elligator2`, as specified in IETF draft. The
//! library provides both,
//! [version 10](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-10),
//! and [version 03](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-03).
//! However, the goal of this library is to be compatible with the VRF implementation over
//! [libsodium](https://github.com/input-output-hk/libsodium). In particular, the differences
//! that completely modify the output of the VRF function are the following:
//! - Computation of the Elligator2 function performs a `bit` modification where it shouldn't,
//!   resulting in a completely different VRF output. [Here](https://github.com/input-output-hk/libsodium/blob/draft-irtf-cfrg-vrf-03/src/libsodium/crypto_vrf/ietfdraft03/convert.c#L84)
//!   we clear the sign bit, when it should be cleared only [here](https://github.com/input-output-hk/libsodium/blob/draft-irtf-cfrg-vrf-03/src/libsodium/crypto_core/ed25519/ref10/ed25519_ref10.c#L2527).
//!   This does not reduce the security of the scheme, but makes it incompatible with other
//!   implementations.
//! - The latest ietf draft no longer includes the suite_string as an input to the `hash_to_curve`
//!   function. Furthermore, it concatenates a zero byte when computing the `proof_to_hash`
//!   function. These changes can be easily seen in the [diff between version 6 and 7](https://www.ietf.org/rfcdiff?difftype=--hwdiff&url2=draft-irtf-cfrg-vrf-07.txt).
//! - The Elligator2 function of the latest [hash-to-curve draft](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-hash-to-curve-11)
//!   is different to that specified in the VRF standard.
//!
//! To provide compatibility with libsodium's implementation,
//! we rely on a fork of dalek-cryptography, to counter the non-compatible `bit` modification.
//!
//! By default, we always expose as the generic VRF, the latest version. However, all VRF functions
//! are exposed in the API.
#![allow(non_snake_case)]
use crate::vrf10::{PublicKey10, SecretKey10, VrfProof10};

/// VRF secret key
pub type SecretKey = SecretKey10;
/// VRF public key
pub type PublicKey = PublicKey10;
/// VRF proof
pub type VrfProof = VrfProof10;

#[cfg(test)]
mod test {
    use super::*;
    use rand_chacha::ChaCha20Rng;
    use rand_core::SeedableRng;

    #[test]
    fn verify_vrf() {
        let alpha_string = [0u8; 23];
        let secret_key = SecretKey::generate(&mut ChaCha20Rng::from_seed([0u8; 32]));
        let public_key = PublicKey::from(&secret_key);

        let vrf_proof = VrfProof::generate(&public_key, &secret_key, &alpha_string);

        assert!(vrf_proof.verify(&public_key, &alpha_string).is_ok());

        let false_sk = SecretKey::generate(&mut ChaCha20Rng::from_seed([1u8; 32]));
        let false_key = PublicKey::from(&false_sk);
        assert!(vrf_proof.verify(&false_key, &alpha_string).is_err())
    }

    #[test]
    fn proof_serialisation() {
        let alpha_string = [0u8; 23];
        let secret_key = SecretKey::generate(&mut ChaCha20Rng::from_seed([0u8; 32]));
        let public_key = PublicKey::from(&secret_key);

        let vrf_proof = VrfProof::generate(&public_key, &secret_key, &alpha_string);
        let serialised_proof = vrf_proof.to_bytes();

        let deserialised_proof = VrfProof::from_bytes(&serialised_proof);
        assert!(deserialised_proof.is_ok());

        assert!(deserialised_proof
            .unwrap()
            .verify(&public_key, &alpha_string)
            .is_ok());
    }

    #[test]
    fn keypair_serialisation() {
        let secret_key = SecretKey::generate(&mut ChaCha20Rng::from_seed([0u8; 32]));
        let public_key = PublicKey::from(&secret_key);

        let serialised_sk = secret_key.to_bytes();
        let deserialised_sk = SecretKey::from_bytes(&serialised_sk);
        let pk_from_ser = PublicKey::from(&deserialised_sk);
        assert_eq!(public_key, pk_from_ser);

        let serialised_pk = public_key.to_bytes();
        let deserialised_pk = PublicKey::from_bytes(&serialised_pk);
        assert_eq!(public_key, deserialised_pk);
    }
}
