//! Implementation of the VRF function as specified in the version 10 of the
//! [standard](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-10),
//! with the difference that it provides batch compatibility. The specification
//! of how one achieves batch-compatibility is described in this
//! [technical spec](https://iohk.io/en/research/library/papers/on-uc-secure-range-extension-and-batch-verification-for-ecvrf/).
//!
//! In a nutshell, instead of including the challenge and the response in VRF proof,
//! one includes the (two) announcements and the response. This allows for an equality check,
//! rather than a computation, of the announcements, which in turn allows for
//! batch verification.
use super::vrf10::SUITE_TEMP;

use curve25519_dalek::{
    constants::ED25519_BASEPOINT_POINT,
    edwards::{CompressedEdwardsY, EdwardsPoint},
    scalar::Scalar,
    traits::VartimeMultiscalarMul,
};

use super::constants::*;
use super::errors::VrfError;

use crate::vrf10::{PublicKey10, SecretKey10, VrfProof10};
#[cfg(feature = "batch_deterministic")]
use blake3;
use curve25519_dalek::traits::IsIdentity;
#[cfg(not(feature = "batch_deterministic"))]
use rand_core::{CryptoRng, OsRng, RngCore};
use sha2::{Digest, Sha512};
use std::iter;
use std::ops::Neg;

/// Byte size of the proof
pub const PROOF_SIZE: usize = 128;

/// VRF proof, which is formed by an `EdwardsPoint`, and two `Scalar`s
#[derive(Clone, Debug)]
pub struct VrfProof10BatchCompat {
    gamma: EdwardsPoint,
    u_point: EdwardsPoint,
    v_point: EdwardsPoint,
    response: Scalar,
}

impl VrfProof10BatchCompat {
    /// Hash to curve implementation, which is equivalent to that of the version 10 counterpart.
    fn hash_to_curve(public_key: &PublicKey10, alpha_string: &[u8]) -> EdwardsPoint {
        VrfProof10::hash_to_curve(public_key, alpha_string)
    }

    /// Nonce generation function, equivalent to the 10 specification.
    fn nonce_generation10(secret_extension: [u8; 32], compressed_h: CompressedEdwardsY) -> Scalar {
        VrfProof10::nonce_generation10(secret_extension, compressed_h)
    }

    /// Hash points function, equivalent to the 10 specification.
    fn compute_challenge(
        compressed_h: &CompressedEdwardsY,
        gamma: &EdwardsPoint,
        announcement_1: &EdwardsPoint,
        announcement_2: &EdwardsPoint,
    ) -> Scalar {
        VrfProof10::compute_challenge(compressed_h, gamma, announcement_1, announcement_2)
    }

    /// Generate a `VrfProof` from an array of bytes with the correct size. This function does not
    /// check the validity of the proof.
    pub fn from_bytes(bytes: &[u8; PROOF_SIZE]) -> Result<Self, VrfError> {
        let gamma = CompressedEdwardsY::from_slice(&bytes[..32])
            .decompress()
            .ok_or(VrfError::DecompressionFailed)?;

        let mut u_point_bytes = [0u8; 32];
        u_point_bytes[..32].copy_from_slice(&bytes[32..64]);
        let u_point = CompressedEdwardsY::from_slice(&u_point_bytes[..32])
            .decompress()
            .ok_or(VrfError::DecompressionFailed)?;

        let mut v_point_bytes = [0u8; 32];
        v_point_bytes[..32].copy_from_slice(&bytes[64..96]);
        let v_point = CompressedEdwardsY::from_slice(&v_point_bytes[..32])
            .decompress()
            .ok_or(VrfError::DecompressionFailed)?;

        let mut response_bytes = [0u8; 32];
        response_bytes.copy_from_slice(&bytes[96..]);
        let response =
            Scalar::from_canonical_bytes(response_bytes).ok_or(VrfError::DecompressionFailed)?;

        Ok(Self {
            gamma,
            u_point,
            v_point,
            response,
        })
    }

    /// Convert the proof into its byte representation.
    pub fn to_bytes(&self) -> [u8; PROOF_SIZE] {
        let mut proof = [0u8; PROOF_SIZE];
        proof[..32].copy_from_slice(self.gamma.compress().as_bytes());
        proof[32..64].copy_from_slice(self.u_point.compress().as_bytes());
        proof[64..96].copy_from_slice(self.v_point.compress().as_bytes());
        proof[96..].copy_from_slice(self.response.as_bytes());

        proof
    }

    /// `proof_to_hash` function, equivalent to the 10 specification.
    pub fn proof_to_hash(&self) -> [u8; OUTPUT_SIZE] {
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
            u_point: announcement_base,
            v_point: announcement_h,
            response,
        }
    }

    /// Verify VRF function, following the spec.
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

        let challenge =
            Self::compute_challenge(&compressed_h, &self.gamma, &self.u_point, &self.v_point);

        let U = EdwardsPoint::vartime_double_scalar_mul_basepoint(
            &challenge.neg(),
            &decompressed_pk,
            &self.response,
        );
        let V = EdwardsPoint::vartime_multiscalar_mul(
            iter::once(self.response).chain(iter::once(challenge.neg())),
            iter::once(h).chain(iter::once(self.gamma)),
        );

        if U == self.u_point && V == self.v_point {
            Ok(self.proof_to_hash())
        } else {
            Err(VrfError::VerificationFailed)
        }
    }
}

/// A structure that represents a batch verifier. It saves each proof and corresponding information
/// independently.
#[cfg(feature = "batch_deterministic")]
#[derive(Clone)]
pub struct BatchVerifier {
    proof_scalars: Vec<(Scalar, Scalar)>,
    pks: Vec<EdwardsPoint>,
    us: Vec<EdwardsPoint>,
    hs: Vec<EdwardsPoint>,
    gammas: Vec<EdwardsPoint>,
    vs: Vec<EdwardsPoint>,
    hasher: blake3::Hasher,
}

#[cfg(feature = "batch_deterministic")]
impl BatchVerifier {
    /// Initialise a BatchVerifier.
    pub fn new(size_batch: usize) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"suite_s");

        Self {
            proof_scalars: Vec::with_capacity(size_batch),
            pks: Vec::with_capacity(size_batch),
            us: Vec::with_capacity(size_batch),
            hs: Vec::with_capacity(size_batch),
            gammas: Vec::with_capacity(size_batch),
            vs: Vec::with_capacity(size_batch),
            hasher,
        }
    }

    /// Insert a new proof into the batch.
    pub fn insert(&mut self, item: BatchItem) -> Result<(), VrfError> {
        if item.output != item.proof.proof_to_hash() {
            return Err(VrfError::VrfOutputInvalid);
        }
        let decompressed_pk = item
            .key
            .0
            .decompress()
            .ok_or(VrfError::DecompressionFailed)?;

        if decompressed_pk.is_small_order() {
            return Err(VrfError::PkSmallOrder);
        }

        let h = VrfProof10BatchCompat::hash_to_curve(&item.key, &item.msg);

        self.proof_scalars.push((
            VrfProof10BatchCompat::compute_challenge(
                &h.compress(),
                &item.proof.gamma,
                &item.proof.u_point,
                &item.proof.v_point,
            ),
            item.proof.response,
        ));

        self.pks.push(decompressed_pk);
        self.us.push(item.proof.u_point);
        self.hs.push(h);
        self.gammas.push(item.proof.gamma);
        self.vs.push(item.proof.v_point);

        self.hasher.update(h.compress().as_bytes());
        self.hasher.update(&item.proof.to_bytes()[..]);

        Ok(())
    }
    /// Verify VRF function, following the spec.
    pub fn verify(mut self) -> Result<(), VrfError> {
        let vec_size = self.proof_scalars.len();
        let mut B_coeff = Scalar::zero();
        let mut lchalls = Vec::with_capacity(vec_size);
        let mut rchalls = Vec::with_capacity(vec_size);
        let mut ls = Vec::with_capacity(vec_size);
        let mut rs = Vec::with_capacity(vec_size);
        let mut rresponse = Vec::with_capacity(vec_size);

        for (index, (challenge, response)) in self.proof_scalars.iter().enumerate() {
            self.hasher.update(&index.to_le_bytes());
            self.hasher.update(&[0x00]);

            let mut l_hasher = self.hasher.clone();
            l_hasher.update(&[0x4c]);
            let mut temp_slice = [0u8; 32];
            temp_slice[..16].copy_from_slice(&l_hasher.finalize().as_bytes()[..16]);
            let l_i = Scalar::from_bits(temp_slice);

            let mut r_hasher = self.hasher.clone();
            r_hasher.update(&[0x52]);
            let mut temp_slice = [0u8; 32];
            temp_slice[..16].copy_from_slice(&r_hasher.finalize().as_bytes()[..16]);
            let r_i = Scalar::from_bits(temp_slice);

            B_coeff += l_i * response;

            lchalls.push(l_i * challenge);
            ls.push(l_i);
            rresponse.push(r_i * response.neg());
            rchalls.push(r_i * challenge);
            rs.push(r_i);
        }
        use iter::once;
        let result = EdwardsPoint::vartime_multiscalar_mul(
            once(&B_coeff.neg())
                .chain(lchalls.iter())
                .chain(ls.iter())
                .chain(rresponse.iter())
                .chain(rchalls.iter())
                .chain(rs.iter()),
            once(&ED25519_BASEPOINT_POINT)
                .chain(self.pks.iter())
                .chain(self.us.iter())
                .chain(self.hs.iter())
                .chain(self.gammas.iter())
                .chain(self.vs.iter()),
        );

        if result.is_identity() {
            return Ok(());
        }

        Err(VrfError::VerificationFailed)
    }
}

#[cfg(not(feature = "batch_deterministic"))]
fn gen_u128<R: RngCore + CryptoRng>(mut rng: R) -> u128 {
    let mut bytes = [0u8; 16];
    rng.fill_bytes(&mut bytes[..]);
    u128::from_le_bytes(bytes)
}

/// A structure that represents a non-deterministic batch verifier. It saves each proof and
/// corresponding information independently.
#[cfg(not(feature = "batch_deterministic"))]
#[derive(Clone)]
pub struct BatchVerifier {
    proof_scalars: Vec<(Scalar, Scalar)>,
    pks: Vec<EdwardsPoint>,
    us: Vec<EdwardsPoint>,
    hs: Vec<EdwardsPoint>,
    gammas: Vec<EdwardsPoint>,
    vs: Vec<EdwardsPoint>,
}

#[cfg(not(feature = "batch_deterministic"))]
impl BatchVerifier {
    /// Initialise a BatchVerifier.
    pub fn new(size_batch: usize) -> Self {
        Self {
            proof_scalars: Vec::with_capacity(size_batch),
            pks: Vec::with_capacity(size_batch),
            us: Vec::with_capacity(size_batch),
            hs: Vec::with_capacity(size_batch),
            gammas: Vec::with_capacity(size_batch),
            vs: Vec::with_capacity(size_batch),
        }
    }

    /// Insert a new proof into the batch.
    pub fn insert(&mut self, item: BatchItem) -> Result<(), VrfError> {
        if item.output != item.proof.proof_to_hash() {
            return Err(VrfError::VrfOutputInvalid);
        }
        let decompressed_pk = item
            .key
            .0
            .decompress()
            .ok_or(VrfError::DecompressionFailed)?;

        if decompressed_pk.is_small_order() {
            return Err(VrfError::PkSmallOrder);
        }

        let h = VrfProof10BatchCompat::hash_to_curve(&item.key, &item.msg);

        self.proof_scalars.push((
            VrfProof10BatchCompat::compute_challenge(
                &h.compress(),
                &item.proof.gamma,
                &item.proof.u_point,
                &item.proof.v_point,
            ),
            item.proof.response,
        ));

        self.pks.push(decompressed_pk);
        self.us.push(item.proof.u_point);
        self.hs.push(h);
        self.gammas.push(item.proof.gamma);
        self.vs.push(item.proof.v_point);

        Ok(())
    }
    /// Verify VRF function, following the spec.
    pub fn verify(self) -> Result<(), VrfError> {
        let vec_size = self.proof_scalars.len();
        let mut B_coeff = Scalar::zero();
        let mut lchalls = Vec::with_capacity(vec_size);
        let mut rchalls = Vec::with_capacity(vec_size);
        let mut ls = Vec::with_capacity(vec_size);
        let mut rs = Vec::with_capacity(vec_size);
        let mut rresponse = Vec::with_capacity(vec_size);

        for (challenge, response) in self.proof_scalars.iter() {
            let l_i = Scalar::from(gen_u128(OsRng));
            let r_i = Scalar::from(gen_u128(OsRng));

            B_coeff += l_i * response;

            lchalls.push(l_i * challenge);
            ls.push(l_i);
            rresponse.push(r_i * response.neg());
            rchalls.push(r_i * challenge);
            rs.push(r_i);
        }
        use iter::once;
        let result = EdwardsPoint::vartime_multiscalar_mul(
            once(&B_coeff.neg())
                .chain(lchalls.iter())
                .chain(ls.iter())
                .chain(rresponse.iter())
                .chain(rchalls.iter())
                .chain(rs.iter()),
            once(&ED25519_BASEPOINT_POINT)
                .chain(self.pks.iter())
                .chain(self.us.iter())
                .chain(self.hs.iter())
                .chain(self.gammas.iter())
                .chain(self.vs.iter()),
        );

        if result.is_identity() {
            return Ok(());
        }

        Err(VrfError::VerificationFailed)
    }
}

/// Structure representing a single batch element. Contains a proof, a public key, the message
/// signed and the VRF output.
pub struct BatchItem {
    /// VRF proof of correct computation
    pub proof: VrfProof10BatchCompat,
    /// Key associated with the proof
    pub key: PublicKey10,
    /// Message (or sometimes referred to as `alpha`)
    pub msg: Vec<u8>,
    /// Pseudo-randomness generated by the VRF
    pub output: [u8; OUTPUT_SIZE],
}

#[cfg(test)]
mod test {
    use super::*;
    use rand_chacha::ChaCha20Rng;
    use rand_core::{RngCore, SeedableRng};

    //////////////////////////////////////////////////////////////////////////////////////////////////
    // VRF test vector from standard                                                                //
    //////////////////////////////////////////////////////////////////////////////////////////////////
    fn test_vectors() -> Vec<Vec<&'static str>> {
        vec![
            vec![
                "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "8657106690b5526245a92b003bb079ccd1a92130477671f6fc01ad16f26f723faef27c725be964c6a9bf4c45ca8e35df258c1878b838f37d9975523f090340715016572f71466c646c119443455d6cb9b952f07d060ec8286d678615d55f954f241fc442e6569e96c462f62b8079b9ed83ff2ee21c90c7c398802fdeebea4001",
                "90cf1df3b703cce59e2a35b925d411164068269d7b2d29f3301c03dd757876ff66b71dda49d2de59d03450451af026798e8f81cd2e333de5cdf4f3e140fdd8ae",
                "",
            ],
            vec![
                "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                "f3141cd382dc42909d19ec5110469e4feae18300e94f304590abdced48aed5931dcb0a4821a2c48bf53548228b7f170962988f6d12f5439f31987ef41f034ab3fd03c0bf498c752161bae4719105a074630a2aa5f200ff7b3995f7bfb1513423ab7b1ea44a256e811892e13eeae7c9f6ea8992557453eac11c4d5476b1f35a08",
                "eb4440665d3891d668e7e0fcaf587f1b4bd7fbfe99d0eb2211ccec90496310eb5e33821bc613efb94db5e5b54c70a848a0bef4553a41befc57663b56373a5031",
                "72",
            ],
            vec![
                "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
                "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
                "9bc0f79119cc5604bf02d23b4caede71393cedfbb191434dd016d30177ccbf802bae73e15a64042fcebf062abe7e432b2eca6744f3e8265bc38e009cd577ecd588cba1cb0d4f9b649d9a86026b69de076724a93a65c349c988954f0961c5d506d08a6c3e3665ff5a4cab13a643bef812e284c6b2ee063a2cb4f456794723ad0a",
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

            let proof = VrfProof10BatchCompat::generate(&pk, &sk, &alpha_string);
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

    #[test]
    fn batch_verification() {
        let nr_proofs = 10;
        let mut alpha = vec![0u8; 32];
        let mut rng = ChaCha20Rng::from_seed([0u8; 32]);

        let mut batch_verifier = BatchVerifier::new(nr_proofs);

        // We generate `nr_proofs` valid proofs.
        for _ in 0..nr_proofs {
            rng.fill_bytes(&mut alpha);
            let secret_key = SecretKey10::generate(&mut rng);
            let public_key = PublicKey10::from(&secret_key);

            let vrf_proof = VrfProof10BatchCompat::generate(&public_key, &secret_key, &alpha);

            batch_verifier
                .insert(BatchItem {
                    output: vrf_proof.proof_to_hash(),
                    proof: vrf_proof,
                    key: public_key,
                    msg: alpha.clone(),
                })
                .expect("Should not fail");
        }
        assert_eq!(batch_verifier.proof_scalars.len(), nr_proofs);

        // Now we perform batch_verification
        assert!(batch_verifier.verify().is_ok());

        // If we have a single invalid proof, the batch verification will fail.
        let invalid_sk = SecretKey10::generate(&mut rng);
        let invalid_pk = PublicKey10::from(&invalid_sk);

        let secret_key = SecretKey10::generate(&mut rng);
        let public_key = PublicKey10::from(&secret_key);
        let vrf_proof = VrfProof10BatchCompat::generate(&public_key, &secret_key, &alpha);

        let mut invalid_batch = BatchVerifier::new(nr_proofs);

        invalid_batch
            .insert(BatchItem {
                output: vrf_proof.proof_to_hash(),
                proof: vrf_proof,
                key: invalid_pk,
                msg: alpha.clone(),
            })
            .expect("Should not fail");

        for _ in 0..nr_proofs {
            rng.fill_bytes(&mut alpha);
            let secret_key = SecretKey10::generate(&mut rng);
            let public_key = PublicKey10::from(&secret_key);

            let vrf_proof = VrfProof10BatchCompat::generate(&public_key, &secret_key, &alpha);

            invalid_batch
                .insert(BatchItem {
                    output: vrf_proof.proof_to_hash(),
                    proof: vrf_proof,
                    key: public_key,
                    msg: alpha.clone(),
                })
                .expect("Should not fail");
        }
        assert_eq!(invalid_batch.proof_scalars.len(), nr_proofs + 1);
        // Now we perform batch_verification
        assert!(invalid_batch.verify().is_err());
    }
}
