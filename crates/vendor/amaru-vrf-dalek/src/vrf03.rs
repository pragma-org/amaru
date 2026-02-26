//! VRF implementation following
//! [version 03](https://datatracker.ietf.org/doc/html/draft-irtf-cfrg-vrf-03)
//! of the draft.
use curve25519_dalek_fork::{
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

/// Secret key, which is formed by `SEED_SIZE` bytes.
pub struct SecretKey03([u8; SEED_SIZE]);

impl SecretKey03 {
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
        SecretKey03(*bytes)
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

impl Drop for SecretKey03 {
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
pub struct PublicKey03(CompressedEdwardsY);

impl Debug for PublicKey03 {
    fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
        write!(f, "PublicKey({:?}))", self.0)
    }
}

impl PublicKey03 {
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
        PublicKey03(CompressedEdwardsY::from_slice(bytes))
    }
}

impl<'a> From<&'a SecretKey03> for PublicKey03 {
    /// Derive a public key from a `SecretKey03`.
    fn from(sk: &SecretKey03) -> PublicKey03 {
        let (scalar, _) = sk.extend();
        let point = scalar * ED25519_BASEPOINT_POINT;
        PublicKey03(point.compress())
    }
}

/// VRF proof, which is formed by an `EdwardsPoint`, and two `Scalar`s
#[derive(Clone, Debug)]
pub struct VrfProof03 {
    gamma: EdwardsPoint,
    challenge: Scalar,
    response: Scalar,
}

impl VrfProof03 {
    /// Hash to curve function, following the 03 specification.
    // Note that in order to be compatible with the implementation over libsodium, we rely on using
    // a fork of curve25519-dalek.
    fn hash_to_curve(public_key: &PublicKey03, alpha_string: &[u8]) -> EdwardsPoint {
        let mut hash_input = Vec::with_capacity(2 + PUBLIC_KEY_SIZE + alpha_string.len());
        hash_input.extend_from_slice(SUITE);
        hash_input.extend_from_slice(ONE);
        hash_input.extend_from_slice(public_key.as_bytes());
        hash_input.extend_from_slice(alpha_string);
        EdwardsPoint::hash_from_bytes::<Sha512>(&hash_input)
    }

    /// Nonce generation function, following the 03 specification.
    fn nonce_generation03(secret_extension: [u8; 32], compressed_h: CompressedEdwardsY) -> Scalar {
        let mut nonce_gen_input = [0u8; 64];
        let h_bytes = compressed_h.to_bytes();

        nonce_gen_input[..32].copy_from_slice(&secret_extension);
        nonce_gen_input[32..].copy_from_slice(&h_bytes);

        Scalar::hash_from_bytes::<Sha512>(&nonce_gen_input)
    }

    /// Hash points function, following the 03 specification.
    fn compute_challenge(
        compressed_h: &CompressedEdwardsY,
        gamma: &EdwardsPoint,
        announcement_1: &EdwardsPoint,
        announcement_2: &EdwardsPoint,
    ) -> Scalar {
        // we use a scalar of 16 bytes (instead of 32), but store it in 32 bits, as that is what
        // `Scalar::from_bits()` expects.
        let mut scalar_bytes = [0u8; 32];
        let mut challenge_hash = Sha512::new();
        challenge_hash.update(SUITE);
        challenge_hash.update(TWO);
        challenge_hash.update(compressed_h.to_bytes());
        challenge_hash.update(gamma.compress().as_bytes());
        challenge_hash.update(announcement_1.compress().as_bytes());
        challenge_hash.update(announcement_2.compress().as_bytes());

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
        let response = Scalar::from_bytes_mod_order(response_bytes);

        Ok(Self {
            gamma,
            challenge,
            response,
        })
    }

    /// Convert the proof into its byte representation. As specified in the 03 specification, the
    /// challenge can be represented using only 16 bytes, and therefore use only the first 16
    /// bytes of the `Scalar`.
    pub fn to_bytes(&self) -> [u8; PROOF_SIZE] {
        let mut proof = [0u8; PROOF_SIZE];
        proof[..32].copy_from_slice(self.gamma.compress().as_bytes());
        proof[32..48].copy_from_slice(&self.challenge.to_bytes()[..16]);
        proof[48..].copy_from_slice(self.response.as_bytes());

        proof
    }

    /// `proof_to_hash` function, following the 03 specification. This computes the output of the VRF
    /// function. In particular, this function computes
    /// SHA512(SUITE || THREE || Gamma)
    pub fn proof_to_hash(&self) -> [u8; OUTPUT_SIZE] {
        let mut output = [0u8; OUTPUT_SIZE];
        let gamma_cofac = self.gamma.mul_by_cofactor();
        let mut hash = Sha512::new();
        hash.update(SUITE);
        hash.update(THREE);
        hash.update(gamma_cofac.compress().as_bytes());

        output.copy_from_slice(hash.finalize().as_slice());
        output
    }

    /// Generate a new VRF proof following the 03 standard. It proceeds as follows:
    /// - Extend the secret key, into a `secret_scalar` and the `secret_extension`
    /// - Evaluate `hash_to_curve` over PK || alpha_string to get `H`
    /// - Compute `Gamma = secret_scalar *  H`
    /// - Generate a proof of discrete logarithm equality between `PK` and `Gamma` with
    ///   bases `generator` and `H` respectively.
    pub fn generate(
        public_key: &PublicKey03,
        secret_key: &SecretKey03,
        alpha_string: &[u8],
    ) -> Self {
        let (secret_scalar, secret_extension) = secret_key.extend();

        let h = Self::hash_to_curve(public_key, alpha_string);
        let compressed_h = h.compress();
        let gamma = secret_scalar * h;

        // Now we generate the nonce
        let k = Self::nonce_generation03(secret_extension, compressed_h);

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

    /// Verify VRF function, following the 03 specification.
    pub fn verify(
        &self,
        public_key: &PublicKey03,
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
    // VRF test vector from                                                                         //
    //  https://github.com/input-output-hk/libsodium/blob/draft-irtf-cfrg-vrf-03/test/default/vrf.c //
    //////////////////////////////////////////////////////////////////////////////////////////////////
    fn test_vectors() -> Vec<Vec<&'static str>> {
        vec![
            vec![
                "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a",
                "b6b4699f87d56126c9117a7da55bd0085246f4c56dbc95d20172612e9d38e8d7ca65e573a126ed88d4e30a46f80a666854d675cf3ba81de0de043c3774f061560f55edc256a787afe701677c0f602900",
                "5b49b554d05c0cd5a5325376b3387de59d924fd1e13ded44648ab33c21349a603f25b84ec5ed887995b33da5e3bfcb87cd2f64521c4c62cf825cffabbe5d31cc",
                "",
            ],
            vec![
                "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb",
                "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c",
                "ae5b66bdf04b4c010bfe32b2fc126ead2107b697634f6f7337b9bff8785ee111200095ece87dde4dbe87343f6df3b107d91798c8a7eb1245d3bb9c5aafb093358c13e6ae1111a55717e895fd15f99f07",
                "94f4487e1b2fec954309ef1289ecb2e15043a2461ecc7b2ae7d4470607ef82eb1cfa97d84991fe4a7bfdfd715606bc27e2967a6c557cfb5875879b671740b7d8",
                "72",
            ],
            vec![
                "c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7",
                "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025",
                "dfa2cba34b611cc8c833a6ea83b8eb1bb5e2ef2dd1b0c481bc42ff36ae7847f6ab52b976cfd5def172fa412defde270c8b8bdfbaae1c7ece17d9833b1bcf31064fff78ef493f820055b561ece45e1009",
                "2031837f582cd17a9af9e0c7ef5a6540e3453ed894b62c293686ca3c1e319dde9d0aa489a4b59a9594fc2328bc3deff3c8a0929a369a72b1180a596e016b5ded",
                "af82",
            ],
            vec![
                "2072dbe36636cccdd91036924c2b8809ce498fb44b0f49e5893d43cdd3abef5c",
                "8bddc92da92c0cdec13143a6270ec9d403040cdd4594a4010c48fdd0cfdc1259",
                "eec40e8cfa4ea3260ef2c77c459feae19b73af587c478c360276a7960e1442fbea0027f65efe26639eaf6ab51ea54639993d903f9933c99683ce3df4103ae6cc66621c222dfcea3ff4e592055d336b03",
                "98c8113fba551c032cf78fd23a49d9213b6603ce6258be2060d22d3cb006d0d75c54304d9bff507b5fea5ea33c1273c5528550a74c4e4ab765e4793d4137d534",
                "7f6819",
            ],
            vec![
                "0e7246b4f1f398c7888780bad3db1ea19a8c9d2ce80a09c526598ee87041c3ea",
                "6fe054540159c1cccacafdc00b357be745e6a6f5d88cdd212232d59a62fe700c",
                "677f422313b3ec0763a7b0f177a41cdf487b0964ccf8335e324a1ac145c791673417f633fdd82d8b6f9016c95ad3f23c809a0dc87a97777b503218d697327c0de9ae970fdcb64ee52aa2e4d4659e3301",
                "3e1fc65dce1e962b0c250dd839aeda46d595105e748fcb5e0c2af7cb4b88b65d2786ca6beb1e6f9e87fc51271463744e6e22fe039bd4dbc7fb1976450b570937",
                "ef97d7cd",
            ],
            vec![
                "503f366351f2ca1b64c9c9cab9fc99d740fe24bce9a429a01c3c59caac5656ab",
                "23c3d11e8add6774703cb37fd640d5dbf7271632717a7b6d93ef81c56e7cfea7",
                "48c8dd1ee07d9d1187217cbaa0e75bbd86dd9fa6f68b6c50f44282403dc29bbda31db087f868625f85a778eab202c350771b172860176dfeb504cd7fed5c485e9bbc79612153f05e00b3c312e0a4c001",
                "1277372ed92744fef3190e1334d64dc78f562a0d8698ebbbd67732a671fe86f32f6546c41d4544a95106306ce3f73b7ac28d9cc0ac0b16b38d1350b3f45137be",
                "26cfa42eec",
            ],
            vec![
                "2212c3b41c5d90743ebb3e42a359a416a0d24b4864731cd9e02a36b2a725c4fc",
                "2df1e001debe5d406e955dd69ed206da9257157bd8c412c7d3b5189736e0928e",
                "d24a9fd404c0758091ebcb6d6141e1df02a960ea5b0d23b138ab7dd6b2d567ce824e9f270a0817e4cb55a58cfaefe4d132a4ae6c22ae597cffbeced42b5e969e1993918eb94d5e77fc404b7299ffbe0a",
                "92c55e8b46ad6cbbfdb322cd0d39b68c3dbb31dc7ea9e5961ce36588a8d9f5b6dbfbdda613512710ae686be3aba9110837f5428ca1ee37dd8c3d0e6eccd7819e",
                "b03279f76bc6",
            ],
            vec![
                "a33e559a1ead4eb2f4de5a98905cd8fa44b9ce42e235e28d81c28e1450ccfcb2",
                "020fcae5434ad29c3799ee6b9309d4038bc781762e624233ab42f5507d4e09a0",
                "7c57edb597647582c5b4ddcc2ef651dccc3f8fb11bb5cf87102a5fb355fef5357449878cb7b651f0896e6947f23e796ea23d369e1a9d45cde8d2dd40923ff6948de7bb0315b07b998e98a09b3204d30f",
                "a88816cf0a6abe8552042435a79db443062d47c2f3269d997ab14e4976fc97f95e78c72608e62feb8ef56c7d0eef91f084796c12da9827c435978a34d78e3342",
                "c5385c291d7b83",
            ],
            vec![
                "99d48f3ac4fbade725674ac13e981ddddb74fb535cedec75687a2d403ea7852e",
                "7f543c693080da044167108d0f1b8c079c4e6356928f7d47badcee555bc19539",
                "0cb633ba9575cfdeaa3dd87d324c6c77292cfea615fdcf6874a373fc8998d9b284199f7543c85f00264f2eb4b4e01f6e785ea4ea5a2172a335dbbb7ee7208311eb473e2a8c0566ee19bab3c133564500",
                "2c48092b21f6957275e21910bd66edc617d7413fc9fcdbcca54ec5926405bb59624eb9c0646c226a256cfde21bd62b1640992b6b6a843bd5d6947d51f820968e",
                "5ac2fd05d52fbcb6",
            ],
            vec![
                "ce8cacafbb0825002c7f539a2d55c1308ca0f1f2537382a86c2c74abce4be16a",
                "a5f47393326bb59de797f0680d8e56c32a6da0867cb80b4e383f89afe86b0205",
                "2481014efdc5dec43014e448dc0103b1f41578df95faef569918f6a77fca23463c91be079511c6cedf30196b3688d344c037adf7838d9c46929e5927471a09a5161d8163bfe7f3a2566791c58604ef0e",
                "868210b075ece9fbef2fe779837081230d0bddb537a1f10af5feda999013987e96b99185e325172c9944937d70c8f897d421434598b1dcb08ba99bb60475cd86",
                "5e08fdc76779542412",
            ],
            vec![
                "7256902cd26e106124a53dba2906dce6181f17b55d7f3b4891bfb6169681aafc",
                "bb95ebcdf29d371827bfc5e2fb7466aa813ae1c117f558c18b0bbdcffcbea1ce",
                "1bb1d1f1e9fa14448575c8d5d4ac838413b69a9ed50250374830c0e50abd2c0cae92110af5c54a54cfb1799da202f1cd897cdc13e98ae0688a8768aca5b6165903eb45f1c8463f1de78be6fe6a236102",
                "50740f4735f825e97368c99462bf5a1f13296276522342b90dc230712a98e8e98b0325f6439c1e230ef26ed7a854c2eb28da5e8b7cb568b74686eb9088d3cac6",
                "1091393f7f5ee6cfd5ac",
            ],
            vec![
                "191e7138092584004b9e7b04e480ed5c860c9c21cb78a4e9019d9f51586ba5aa",
                "0234d5d93e124e7840eae01eb5647d80f5aec4e4985111fd2d747fca127dbe37",
                "dfa0efe6dd74958d79d4c550368672f47b5a1f87ed8608eaa11fef245539843852fb1607a0910e722b34da22b37573fa996b7695351b52a488b36703576c805bfbe3c930b5ed48b35dfb88b586f04904",
                "20b23f16c140db2977d4ae12c04c9785eb4e6e3e104bcb67aff890780605cc2b85c5dc37be484030196d0f8d840ead741f5c20606643a2bd578af438cb3049c8",
                "a16870cb1b34815a653ab8",
            ],
            vec![
                "f55b39a17b8a3bf71fd700c8619807aa362388ec507f48f8d9a69c791c3ac272",
                "af2c6d80ed1167c50b88d347ff2eafc85b4dbe444b70dbd2daaec0c0c9eb0454",
                "daa1aa4ad1648d115136f8592769c29922f70544e162b15f4826a8d5fb58d12f006630a34181e45c2e9b05d7610b44bdfa3a1e678ac5fddb929244cb729f1ac46f71d619eac81a60231b5aab33984101",
                "843bd43eb87fd27855f6b3ffe012daa9c48d49cb8306daea1a7243950c6c7ce469d3d3d151d8ec859c1a5507995609a689deac04610cfa83ef26833c815a45e1",
                "87e7d71f6d040ea68d499599",
            ],
            vec![
                "8e7e7aea299543e3e7aa508a298deb09d8ccf43106bfe85b00bd67ccb3518e25",
                "82a7f4b9c2768be9dab5296c5faf028bb62289afe261677d6a6f99296429abeb",
                "3381880481c7dec448ac57e2bd3bb1773c0f472267d61e749c0d5bb8b03270885913435a3573bcf796c78da96ea008a2f5a7be2def5d62a019cf6793497d74f0cc7c026f8a74a202ff75f550ec918405",
                "e2cc92d3502f20e640fe788ceb2f4432b2bab97411e6509cb0be800a708d76ee0cea2f27dd35d9cae88a802ed971743ac6d5863591bcbe772713722c11eee63d",
                "cdf85caff85005bae2a6822d07",
            ],
            vec![
                "7ce63963c9882aff9191f07faab453c76700cc226514e1a69c79f916e64c1a8c",
                "17b90da3841cc87983ac3022adb26d31e5261844f692020f2ac9a46e31e04265",
                "0f19ec31d42b466b4cd22f0a22483cf86d7c8980513ff5e5abfd76ee97c1c1bd5179f0dc17920bdeaab077ebbd3544155591c04720ddfce05d9a45261b94ddeb6179c00b5b961078362c11c651452807",
                "63d0e432e68068ae8a3244e6741c173ab2040d8ce64bd9179fa45c53b0b0ea2b20772c5576242621d84578ae907c77ccdad1795ee5bf23a02ad2b5b62c374bdc",
                "732cb48511e267afaa63e11f4c9b",
            ],
            vec![
                "29462076624f06128b734b7ccffde61a93caec478dc4443e860ace98f6ce9a9e",
                "2fb75ba523d7cdcc78a987d7f79d0ffefa552bd1b335e152a3bbefe53ff59a3b",
                "51dc03f686da4d442bf7aa971185f1cfc8c70887469afac097842bc2a4401b0cd86917b487aa88372f0c1ffa27f3e4f1e460f88b8bd2fb22c084387c608e0d1e50c1b934553abaee4148015d15c3e00a",
                "fe242ca7a17c03d9dcf59e5636ac1d68f8b5b74def3364a8c338c23a6b3749d18e916c962d2543b589cffb8ded3437e9d03339c26d2f5dab09a4bdcf825372e4",
                "34b86c82800b22d6b0c4e1b4d7f9c8",
            ],
            vec![
                "5d5aa7ec999a492e0dff784aac9af33b0e26408be6103d9dfb75c04a0ad974f7",
                "e2b1148d7fdd9610add3cd6592419044bcc1b6da4284b186381bffe3172135cc",
                "9404e77231c053e6c8f9e5ad07c96046374141b25a3d247b2ec8666842679a7fa08f5fb515a315fae0d26f638e4225c53329cdc18b294522105efe0b32daaecaeed84a8417c38091ee975fea972a4303",
                "f4160b3caee4c554f588921db90265299584c01e6856d9bdaa70ba83df4f90b118bfdf7de59c6c809e0c45e9cb513e57ac399da6bac0b6e8b58c58864181487e",
                "8734d67b402fa990cad4b657232b53f9",
            ],
            vec![
                "4146ed0469b04977161b4c5886e952dce4a18c48a6ae96dc27b7dc02365b3958",
                "7756e4cd1ffef6bcb8d53875a578519fa4d7877f1550cc1a0b6595b26bd24325",
                "79db88c3357ca6a206b7261251b1a24c9a624e3950ee91d50c8cf978444591fac9e5bb10cb8bd3057fb17cb738c5f1acda0bd3e54cb59819d3868c569f889663f99a31d00a6187a8e20c7dcc5a37e90f",
                "7c10b9e09a307e8a3c80695a4e794df5c76057db5656e1b4c19eb72d0865453382f61526fc37382adde70c627482eecd96b2b445adc83338d374eadf25f810b9",
                "3c7e90bec4ae377da12ec7eb7afa7a40e4",
            ],
            vec![
                "fc23e96b69efc327ddc9d6b6461b96649d9f1a1a1b0ea177dec19ea36eee41a3",
                "e7101e167c898f6cf592a5c6b8e6f59f759801acdd211708d4733ef32064d235",
                "29a78f2ec180f76de175b19a17a3001c033a57d5797a2223585133c1230c12ed3a41951e6813e336ee4be1e251981f6faa6e035dd0bb05d791f4ecff26b1b014973fb6d8004de20533922a7b530cb10d",
                "00b8c8212b67360f2c6182736a68a449982e9f3d898bbc5ab16f5b9cc5b800081b90613aea27314d071030477217d2e87ba25127dfaf5f7e0ac48e2073591430",
                "4c6cccfe75c640e667ec86fe7480be95e6d7",
            ],
            vec![
                "ae4aed359b684b1a8dcc79f7814ae22b6abd6fd4e1acefcf38c21e6fce4b0c23",
                "98c4bd2215c499c416927ea5ae6b81a169f745a3c76b4ba09e8d48b6899dcf0e",
                "949e40d8b1fcf0a994a5c00b63303fc01f1b3ef85f6e3c76d1ec35bd03133e125b42dee4d0f511770d2487cf45bd8076f19822674d2ac26d355e7334db3619c15193ca8983c0fde4d851aafc78c07c0d",
                "db49edf179917672f7e500980c119dba0895bdfb81a21030dfb28ba94f7c8b430b5b2d5b702716b1465f705980c6d3aac116fa1e684ddd4ae119498b385c14b2",
                "1e93418995a29124b88cb37bc1a51d430512e8",
            ],
            vec![
                "837a8cf36ccf652cba6536493cf5c6bb7dae7c41b49e0b80c7554cb233541af0",
                "7c8b273a693020a296a6b08742195b936986021f038925a04df5416bbb20c3fd",
                "7e0ff23297f14fd449e9362e41d1f4ed8792fda457a68458149c0e4126fc99290585564f20176db2332d5e42bb71d0ebf56670995f7623030e739399e1c54ce37acf0644bfb945ca310d7b0abff5940b",
                "694cbab0a2fe5c4270269e75d6117376d8f49137789582247b50018731ea151a5fae53e8571f8518a782056a2401744336add37f5a40b7a63788a987c9a542da",
                "9e8e69a8646ae9afd40d31cc9ebea1a53820e58f",
            ],
            vec![
                "3693a6381b5aeffb7bac59edae7a4c8ab26a647842a4bab3e1371818e995ea23",
                "4292d4142e6c622b32cf883e6580a559723d74458fe1a0ba81407b3a575f3ab6",
                "8486300f0b95c436625316c8d1c018df4999ae54d08f4a83613292a95cd7ed3f1341781c708d9ab18dfe09ebd33be5a166c4f288986b734289e5892e2c75df2966e9c3ec676e1e4ec7a3605f79d4590b",
                "2b75850a74912ba2bfe0045ed9f4aa494da9d3900b79e86fe35c6efa924aca3753f64ab68b00e2d7bf2ea787cb251e75a1a7c820b6cbdd92faa33e90efb572a0",
                "4a18cfa03d1ca01136d6ecf66cb5e809fee9ddbfb5",
            ],
            vec![
                "f9c50f053dd57762ecf9bc0dfe2d514227e6cd6ccaca1ff1eabaed30c78490e6",
                "05c8dae0326df273fc47d89e20da78e5fac3671f694ce5a9e15821936bc7dc63",
                "179e8ff9adfdc5ac416a4a1be1603e0bc0a1e1d0525061aa3135b7b7bd00f50a532d37d6cde20752e71ffef270e9f52fc3790b1a4765d99fb8f2d80ece0e71583e32bbc2961a121668c51f9b74bde903",
                "91e2a355d929783bba063c73e2283da93a2434cb1cb1ddf473132b10be907c9f8bbc0a718a22fa91b15230d27469907aad9ff8dd3d70c574f94363e4a8269d7b",
                "1da534df1f14d150bf3f6777376a03900eeea2bd2c8b",
            ],
            vec![
                "498b903922241dbcafba563de392462d85a52e61a689e310ccc9661ade33cd05",
                "5f29a1d81c1f9da7ad060fae11626a6bdf92ae45bc7c60749200a22a60689545",
                "00161502d55f50760bcf14f1f7e4571342258234d7896d8aa7cbfdf7b6cf30f5a1b4420b9ccec3e34ecc4a579a131bb3cb5f0a93595b3cb70f942f502c31901cb915c2e67b7891c4101471fe8a003605",
                "c188760b742838e68a13764722430f0a257a4106f16bada982432a3208af644839e3a96966057d705b2abdb884e6778b6fc5eba3c5d0109a1f33948a807a353d",
                "eba29b15d478ed3d5af71caa5ac192af0d48b52308d2fb",
            ],
            vec![
                "1700cd633ffd17e10ca9a7f97e3b2f67022460ff2c2b717c32f53c7f3bfc2eca",
                "540cc06ad0fedd229f894cc40907398e4ffe034f35443548870a15c5b95f7470",
                "b3c2d59e8d7171b26e3db2f75fca7ea3b84b736583a3a1901fa1fa3ec5fe622d5971dfe72df99616d4dbc8baa9bdc72316b5a62a1a563e17dc3e31827f19b6aa224b433bc318bf2db2dfd5781afac006",
                "249037751503d315f3e81cef5dafd23de94fd843b011d0772713bcaa68551c84e09fdb285191c3f75f9a2c496bae11ca4125b2a1e0ad63be5a8ad04d8daf5385",
                "92cb5d538bc863afdb96b9a44ba00ba8ac656c3e5feea358",
            ],
            vec![
                "8195fd1ef129923610deb85f79e821502dacb44792be15e6480308d4604b8620",
                "526c274fc2962dcc365334023176d11089a327f7d60c420cf3dfc807fe1744a4",
                "a270c181ccf93989927c6724c12c8716bf5f235fe8e9b8cb33fdb9240800baf1bec3173a432c7ab9455f9463775dccb59caa75e313debd11313f9db70c74d01021b11e0c57d5896926c375cd6bbaf307",
                "ab9c4ca36ccdd7f55ecb2e4c91f0c64e1a1807aab4c1cb6d13fc7e7c75b18489a07bc97d30c5cdf388247edcf8cc90ed644c08bf18e57abd93998fb54bbb3947",
                "b5f6af3d6d9b06cd2619d287708a7a0d932bc702b7c24cb90c",
            ],
            vec![
                "46217e4402ded3cec9ccf7d78d8263ef4ab61a0d23910bdff028433dc6d736b0",
                "095784abf7977822f73115a3fc48325f0fde1428c7fa082109d6aefd4a1aa647",
                "ca1ceaf4555c6696234df1b397bb2886ac0d3394aa35e748351b0275896b65a11245154ca2dc31a7005b8717205d9128d94a96bb6d6999fcff7151dab2f576e9c4a1189e3ae5d5b9f5e6c9b8a90b4900",
                "5aee58976aecf422ab4e1574af8575104434591c8d607c749abf9e807b3d9fe4e89634b27950bcb91c3a332a74f9363b6faeb39c5026afbe4a130af68e2a9036",
                "1d5390b72c5e8cb54bff239baf6351a098a86ea9e0e7188e9e94",
            ],
            vec![
                "307dfc17f2db83c6678ec05015bd399fa50ac3d8b8e95b7fbfb9f8f92dcb723e",
                "eb8d10e8711f75248dd187d233445d8f6901be05921dac4df587a6578d0017e0",
                "24764a4b875b540cb7fc454af5b35cc4c5b8f783de50a3415baf6044d5114884f28056de9098aec21e5816cc97349c7cba7135804b9b7a813b81c0afb3dada4a758df97a8ece7336f46607f7f263b408",
                "6ecb3ef1a7a130f5d8b37ff4c62442422180896e2feb5eb2014945433d36628b4e2a9addc689cd465dd11f16be80df6f7cf6ae03861fb550837b6c60acfb47cb",
                "6bb52c5cc8e2fc79dfe2fecd252a67317d52bd6526dbbc04050f5b",
            ],
            vec![
                "7f73a42e2227955695a52f8315f1fbfb198ac53598e0bc7ba90b667f6baf6690",
                "d1330bedccd81fd0973571575b1cc739b10940c3ba0c1031d8cea7ede0598299",
                "2004be378f4b31f5402d6be3a5a5eac93381d8799fe0c4d6913819925e780ddfb09cd2a934c0214df6c56b405329bbbc81a85d007c5d0a3cde2182203af85e6356ff5bf4d7ed6fb9529f3bdb396ca707",
                "1ed33bb204000413110ea830cbb42345e9340aad92690a78998230926f5e793cc606cdc6eb1c5cc7f90e06b2c89e110dcf7f0b73c525641463817a54614153cd",
                "348dd0c347758196bd77408d62d9c561d2d572773b7e2e3bb8c9b689",
            ],
            vec![
                "3f7695ff8e0524eb1a7bac9b2ceaba64232dca022a1cd3ce753290d15fe9eb12",
                "5ca0ed2b774adf3bae5e3e7da2f8ec877d9f063cc3d7050a6c49dfbbe2641dec",
                "02180c447320b66012420971b70b448d11fead6d6e334c398f4daf01ccd92bfbcc4a8730a296ab33241f72da3c3a1fd53f1206a2b9f27ff6a5d9b8860fd955c39f55f9293ab58d1a2c18d555d2686101",
                "deb23fdc1267fa447fb087796544ce02b0580df8f1927450bed0df134ddc3548075ed48ffd72ae2a9ea65f79429cfbe2e15b625cb239ad0ec3910003765a8eb3",
                "fc9f719740f900ee2809f6fdcf31bb6f096f0af133c604a27aaf85379c",
            ],
            vec![
                "ac21b0303ed8b5123ae3bf13f2c627dc9b9c2e04cf1c732364cfe472a5f145c2",
                "5b14fde6db37a5302f0150a73f6ada5a05a810a24c656482cbdeb51020e92fe7",
                "31cdab620bdf8231abcee32d615af34649e69b132286cc59f950f2355284d953a6b7ea975a2fd89af443a15c076399f390c94a6aa3f5dbf8e1576780a7e16fa48c3a095972ac3ab1e1aa076d9246ac09",
                "152dca76912d4a38606f46d90d4b878b3e60b9cef8740ef322290f18a67ad48e716f412e7013d5e1ee4c4aaf5e7f86d158249bcd067f5e62b62c25b7166b9429",
                "230dd4c855c133c5b3c24a72af9bbbc48205984ea4f2045feaac17fe1af9",
            ],
        ]
    }

    #[test]
    fn check_test_vectors() {
        for vector in test_vectors().iter() {
            let mut seed = [0u8; 32];
            seed.copy_from_slice(&hex::decode(vector[0]).unwrap());
            let sk = SecretKey03::from_bytes(&seed);
            let pk = PublicKey03::from(&sk);
            assert_eq!(pk.to_bytes()[..], hex::decode(vector[1]).unwrap());

            let alpha_string = hex::decode(vector[4]).unwrap();

            let proof = VrfProof03::generate(&pk, &sk, &alpha_string);
            assert_eq!(proof.to_bytes()[..], hex::decode(vector[2]).unwrap());

            let output = proof.verify(&pk, &alpha_string).unwrap();
            assert_eq!(output[..], hex::decode(vector[3]).unwrap());
        }
    }
}
