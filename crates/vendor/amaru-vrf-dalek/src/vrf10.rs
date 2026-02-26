//! Implementation of version 10 of the [standard draft](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-10).
use curve25519_dalek::{
    constants::ED25519_BASEPOINT_POINT,
    edwards::{CompressedEdwardsY, EdwardsPoint},
    scalar::Scalar,
    traits::VartimeMultiscalarMul,
};

use super::constants::*;
use super::errors::VrfError;

use rand_core::{CryptoRng, RngCore};
use sha2::{Digest, Sha512};
use std::fmt::Debug;
use std::ops::Neg;
use std::{iter, ptr};

/// Byte size of the proof
pub const PROOF_SIZE: usize = 80;
/// Temporary SUITE identifier, as TAI uses 0x03
pub const SUITE_TEMP: &[u8] = &[0x03];

/// Secret key, which is formed by `SEED_SIZE` bytes.
pub struct SecretKey10([u8; SEED_SIZE]);

impl SecretKey10 {
    /// View `SecretKey` as byte array
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }

    /// Convert a `SecretKey` into its byte representation
    pub fn to_bytes(&self) -> [u8; SEED_SIZE] {
        self.0
    }

    /// Convert a `SecretKey` from a byte array
    pub fn from_bytes(bytes: &[u8; SEED_SIZE]) -> Self {
        SecretKey10(*bytes)
    }

    /// Given a cryptographically secure random number generator `csrng`, this function returns
    /// a random `SecretKey`
    pub fn generate<R>(csrng: &mut R) -> Self
    where
        R: CryptoRng + RngCore,
    {
        let mut seed = [0u8; SEED_SIZE];

        csrng.fill_bytes(&mut seed);
        Self::from_bytes(&seed)
    }

    /// Given a `SecretKey`, the `extend` function hashes the secret bytes to an output of 64 bytes,
    /// and then uses the first 32 bytes to generate a secret
    /// scalar. The function returns the secret scalar and the remaining 32 bytes
    /// (named the `SecretKey` extension).
    pub fn extend(&self) -> (Scalar, [u8; 32]) {
        let mut h: Sha512 = Sha512::new();
        let mut extended = [0u8; 64];
        let mut secret_key_bytes = [0u8; 32];
        let mut extension = [0u8; 32];

        h.update(self.as_bytes());
        extended.copy_from_slice(&h.finalize().as_slice()[..64]);

        secret_key_bytes.copy_from_slice(&extended[..32]);
        extension.copy_from_slice(&extended[32..]);

        secret_key_bytes[0] &= 248;
        secret_key_bytes[31] &= 127;
        secret_key_bytes[31] |= 64;

        (Scalar::from_bits(secret_key_bytes), extension)
    }
}

impl Drop for SecretKey10 {
    #[inline(never)]
    fn drop(&mut self) {
        unsafe {
            let ptr = self.0.as_mut_ptr();
            for i in 0..SEED_SIZE {
                ptr::write_volatile(ptr.add(i), 0u8);
            }
        }
    }
}

/// VRF Public key, which is formed by an Edwards point (in compressed form).
#[derive(Copy, Clone, Default, Eq, PartialEq)]
pub struct PublicKey10(pub(crate) CompressedEdwardsY);

impl Debug for PublicKey10 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        write!(f, "PublicKey({:?}))", self.0)
    }
}

impl PublicKey10 {
    /// View the `PublicKey` as bytes
    pub fn as_bytes(&self) -> &[u8; PUBLIC_KEY_SIZE] {
        self.0.as_bytes()
    }

    /// Convert a `PublicKey` into its byte representation.
    pub fn to_bytes(self) -> [u8; PUBLIC_KEY_SIZE] {
        self.0.to_bytes()
    }

    /// Generate a `PublicKey` from an array of `PUBLIC_KEY_SIZE` bytes.
    pub fn from_bytes(bytes: &[u8; PUBLIC_KEY_SIZE]) -> Self {
        PublicKey10(CompressedEdwardsY::from_slice(bytes))
    }
}

impl<'a> From<&'a SecretKey10> for PublicKey10 {
    /// Derive a public key from a `SecretKey`.
    fn from(sk: &SecretKey10) -> PublicKey10 {
        let (scalar, _) = sk.extend();
        let point = scalar * ED25519_BASEPOINT_POINT;
        PublicKey10(point.compress())
    }
}

/// VRF proof, which is formed by an `EdwardsPoint`, and two `Scalar`s
#[derive(Clone, Debug)]
pub struct VrfProof10 {
    gamma: EdwardsPoint,
    challenge: Scalar,
    response: Scalar,
}

impl VrfProof10 {
    /// Computing the `hash_to_curve` using try and increment. In order to make the
    /// function always terminate, we bound  the number of tries to 64. If the try
    /// 64 fails, which happens with probability around 1/2^64, we compute the
    /// Elligator mapping. This diverges from the standard: the latter describes
    /// the function with an infinite loop. To avoid infinite loops or possibly
    /// non-terminating functions, we adopt this modification.
    /// This function is temporary, until `curve25519` provides a `hash_to_curve` as
    /// presented in the standard. Depends on
    /// [this pr](https://github.com/dalek-cryptography/curve25519-dalek/pull/377). The
    /// goal of providing this temporary function, is to ensure that the rest of the
    /// implementation is valid with respect to the standard, by using their test-vectors.
    pub(crate) fn hash_to_curve(public_key: &PublicKey10, alpha_string: &[u8]) -> EdwardsPoint {
        let mut counter = 0u8;
        let mut hash_input = Vec::with_capacity(4 + PUBLIC_KEY_SIZE + alpha_string.len());
        hash_input.extend_from_slice(SUITE_TEMP);
        hash_input.extend_from_slice(ONE);
        hash_input.extend_from_slice(public_key.as_bytes());
        hash_input.extend_from_slice(alpha_string);
        hash_input.extend_from_slice(&counter.to_be_bytes());
        hash_input.extend_from_slice(ZERO);

        while counter < 64 {
            hash_input[2 + PUBLIC_KEY_SIZE + alpha_string.len()] = counter.to_be_bytes()[0];
            if let Some(result) =
                CompressedEdwardsY::from_slice(&Sha512::digest(&hash_input)[..32]).decompress()
            {
                return result.mul_by_cofactor();
            };

            counter += 1;
        }

        EdwardsPoint::hash_from_bytes::<Sha512>(&hash_input)
    }

    /// Nonce generation function, following the 10 specification.
    pub(crate) fn nonce_generation10(
        secret_extension: [u8; 32],
        compressed_h: CompressedEdwardsY,
    ) -> Scalar {
        let mut nonce_gen_input = [0u8; 64];
        let h_bytes = compressed_h.to_bytes();

        nonce_gen_input[..32].copy_from_slice(&secret_extension);
        nonce_gen_input[32..].copy_from_slice(&h_bytes);

        Scalar::hash_from_bytes::<Sha512>(&nonce_gen_input)
    }

    /// Hash points function, following the 10 specification.
    pub(crate) fn compute_challenge(
        compressed_h: &CompressedEdwardsY,
        gamma: &EdwardsPoint,
        announcement_1: &EdwardsPoint,
        announcement_2: &EdwardsPoint,
    ) -> Scalar {
        // we use a scalar of 16 bytes (instead of 32), but store it in 32 bits, as that is what
        // `Scalar::from_bits()` expects.
        let mut scalar_bytes = [0u8; 32];
        let mut challenge_hash = Sha512::new();
        challenge_hash.update(SUITE_TEMP);
        challenge_hash.update(TWO);
        challenge_hash.update(compressed_h.to_bytes());
        challenge_hash.update(gamma.compress().as_bytes());
        challenge_hash.update(announcement_1.compress().as_bytes());
        challenge_hash.update(announcement_2.compress().as_bytes());
        challenge_hash.update(ZERO);

        scalar_bytes[..16].copy_from_slice(&challenge_hash.finalize().as_slice()[..16]);

        Scalar::from_bits(scalar_bytes)
    }

    /// Generate a `VrfProof` from an array of bytes with the correct size. This function does not
    /// check the validity of the proof.
    pub fn from_bytes(bytes: &[u8; PROOF_SIZE]) -> Result<Self, VrfError> {
        let gamma = CompressedEdwardsY::from_slice(&bytes[..32])
            .decompress()
            .ok_or(VrfError::DecompressionFailed)?;

        let mut challenge_bytes = [0u8; 32];
        challenge_bytes[..16].copy_from_slice(&bytes[32..48]);
        let challenge = Scalar::from_bits(challenge_bytes);

        let mut response_bytes = [0u8; 32];
        response_bytes.copy_from_slice(&bytes[48..]);
        let response =
            Scalar::from_canonical_bytes(response_bytes).ok_or(VrfError::DecompressionFailed)?;

        Ok(Self {
            gamma,
            challenge,
            response,
        })
    }

    /// Convert the proof into its byte representation. As specified in the 10 specification, the
    /// challenge can be represented using only 16 bytes, and therefore use only the first 16
    /// bytes of the `Scalar`.
    pub fn to_bytes(&self) -> [u8; PROOF_SIZE] {
        let mut proof = [0u8; PROOF_SIZE];
        proof[..32].copy_from_slice(self.gamma.compress().as_bytes());
        proof[32..48].copy_from_slice(&self.challenge.to_bytes()[..16]);
        proof[48..].copy_from_slice(self.response.as_bytes());

        proof
    }

    /// `proof_to_hash` function, following the 10 specification. This computes the output of the VRF
    /// function. In particular, this function computes
    /// SHA512(SUITE || THREE || Gamma || ZERO)
    fn proof_to_hash(&self) -> [u8; OUTPUT_SIZE] {
        let mut output = [0u8; OUTPUT_SIZE];
        let gamma_cofac = self.gamma.mul_by_cofactor();
        let mut hash = Sha512::new();
        hash.update(SUITE_TEMP);
        hash.update(THREE);
        hash.update(gamma_cofac.compress().as_bytes());
        hash.update(ZERO);

        output.copy_from_slice(hash.finalize().as_slice());
        output
    }

    /// Generate a new VRF proof following the standard. It proceeds as follows:
    /// - Extend the secret key, into a `secret_scalar` and the `secret_extension`
    /// - Evaluate `hash_to_curve` over PK || alpha_string to get `H`
    /// - Compute `Gamma = secret_scalar *  H`
    /// - Generate a proof of discrete logarithm equality between `PK` and `Gamma` with
    ///   bases `generator` and `H` respectively.
    pub fn generate(
        public_key: &PublicKey10,
        secret_key: &SecretKey10,
        alpha_string: &[u8],
    ) -> Self {
        let (secret_scalar, secret_extension) = secret_key.extend();

        let h = Self::hash_to_curve(public_key, alpha_string);
        let compressed_h = h.compress();
        let gamma = secret_scalar * h;

        // Now we generate the nonce
        let k = Self::nonce_generation10(secret_extension, compressed_h);

        let announcement_base = k * ED25519_BASEPOINT_POINT;
        let announcement_h = k * h;

        // Now we compute the challenge
        let challenge =
            Self::compute_challenge(&compressed_h, &gamma, &announcement_base, &announcement_h);

        // And finally the response of the sigma protocol
        let response = k + challenge * secret_scalar;
        Self {
            gamma,
            challenge,
            response,
        }
    }

    /// Verify VRF function, following the specification.
    pub fn verify(
        &self,
        public_key: &PublicKey10,
        alpha_string: &[u8],
    ) -> Result<[u8; OUTPUT_SIZE], VrfError> {
        let h = Self::hash_to_curve(public_key, alpha_string);
        let compressed_h = h.compress();

        let decompressed_pk = public_key
            .0
            .decompress()
            .ok_or(VrfError::DecompressionFailed)?;

        if decompressed_pk.is_small_order() {
            return Err(VrfError::PkSmallOrder);
        }

        let U = EdwardsPoint::vartime_double_scalar_mul_basepoint(
            &self.challenge.neg(),
            &decompressed_pk,
            &self.response,
        );
        let V = EdwardsPoint::vartime_multiscalar_mul(
            iter::once(self.response).chain(iter::once(self.challenge.neg())),
            iter::once(h).chain(iter::once(self.gamma)),
        );

        // Now we compute the challenge
        let challenge = Self::compute_challenge(&compressed_h, &self.gamma, &U, &V);

        if challenge.to_bytes()[..16] == self.challenge.to_bytes()[..16] {
            Ok(self.proof_to_hash())
        } else {
            Err(VrfError::VerificationFailed)
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    //////////////////////////////////////////////////////////////////////////////////////////////////
    // VRF test vector from standard                                                                //
    //////////////////////////////////////////////////////////////////////////////////////////////////
    fn test_vectors() -> Vec<Vec<&'static str>> {
        vec![
            vec![
                "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "8657106690b5526245a92b003bb079ccd1a92130477671f6fc01ad16f26f723f5e8bd1839b414219e8626d393787a192241fc442e6569e96c462f62b8079b9ed83ff2ee21c90c7c398802fdeebea4001",
                "90cf1df3b703cce59e2a35b925d411164068269d7b2d29f3301c03dd757876ff66b71dda49d2de59d03450451af026798e8f81cd2e333de5cdf4f3e140fdd8ae",
                "",
            ],
            vec![
                "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                "f3141cd382dc42909d19ec5110469e4feae18300e94f304590abdced48aed593f7eaf3eb2f1a968cba3f6e23b386aeeaab7b1ea44a256e811892e13eeae7c9f6ea8992557453eac11c4d5476b1f35a08",
                "eb4440665d3891d668e7e0fcaf587f1b4bd7fbfe99d0eb2211ccec90496310eb5e33821bc613efb94db5e5b54c70a848a0bef4553a41befc57663b56373a5031",
                "72",
            ],
            vec![
                "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
                "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
                "9bc0f79119cc5604bf02d23b4caede71393cedfbb191434dd016d30177ccbf80e29dc513c01c3a980e0e545bcd848222d08a6c3e3665ff5a4cab13a643bef812e284c6b2ee063a2cb4f456794723ad0a",
                "645427e5d00c62a23fb703732fa5d892940935942101e456ecca7bb217c61c452118fec1219202a0edcf038bb6373241578be7217ba85a2687f7a0310b2df19f",
                "af82",
            ],
        ]
    }

    #[test]
    fn check_test_vectors() {
        for (index, vector) in test_vectors().iter().enumerate() {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&hex::decode(vector[0]).unwrap());
            let sk = SecretKey10::from_bytes(&seed);
            let pk = PublicKey10::from(&sk);
            assert_eq!(
                pk.to_bytes()[..],
                hex::decode(vector[1]).unwrap(),
                "PK comparison failed at iteration {}",
                index
            );

            let alpha_string = hex::decode(vector[4]).unwrap();

            let proof = VrfProof10::generate(&pk, &sk, &alpha_string);
            assert_eq!(
                proof.to_bytes()[..],
                hex::decode(vector[2]).unwrap(),
                "Proof comparison failed at iteration {}",
                index
            );

            let output = proof.verify(&pk, &alpha_string).unwrap();
            assert_eq!(
                output[..],
                hex::decode(vector[3]).unwrap(),
                "Output comparison failed at iteration {}",
                index
            );
        }
    }
}
