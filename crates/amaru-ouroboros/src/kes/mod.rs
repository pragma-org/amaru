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

use kes_summed_ed25519::{
    self as kes,
    kes::{Sum6Kes, Sum6KesSig},
    traits::{KesSig, KesSk},
};
use std::{array::TryFromSliceError, ops::Deref};
use thiserror::Error;

// ------------------------------------------------------------------- SecretKey

/// KES secret key
pub struct SecretKey<'a>(Sum6Kes<'a>);

impl SecretKey<'_> {
    /// Create a new KES secret key
    pub fn from_bytes(sk_bytes: &mut Vec<u8>) -> Result<SecretKey<'_>, Error> {
        // TODO: extend() could potentially re-allocate memory to a new location and copy the sk_bytes.
        // This would leave the original memory containing the secret key without being wiped.
        sk_bytes.extend([0u8; 4]); // default to period = 0
        let sum_6_kes = Sum6Kes::from_bytes(sk_bytes.as_mut_slice())?;
        Ok(SecretKey(sum_6_kes))
    }

    /// Get the internal representation of the KES secret key at the current period
    /// This value will include the period as the last 4 bytes in big-endian format
    ///
    /// # Safety
    /// This function is marked unsafe because we wished to highlight the
    /// importance of keeping the content of the secret key private.
    /// However there are reasons that may be valid to _leak_ the private
    /// key: to encrypt it and store securely.
    pub unsafe fn leak_into_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }

    /// Get the current period of the KES secret key
    pub fn get_period(&self) -> u32 {
        self.0.get_period()
    }

    /// Update the KES secret key to the next period
    pub fn update(&mut self) -> Result<(), Error> {
        Ok(self.0.update()?)
    }
}

// ------------------------------------------------------------------- PublicKey

/// KES public key
pub struct PublicKey(kes::PublicKey);

impl PublicKey {
    /// Size of a KES public key, in bytes;
    pub const SIZE: usize = 32;
}

impl AsRef<[u8]> for PublicKey {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl Deref for PublicKey {
    type Target = [u8; Self::SIZE];

    fn deref(&self) -> &Self::Target {
        self.0.as_bytes().try_into().unwrap_or_else(|e| {
            unreachable!(
                "Impossible! Failed to convert KES public key ({}) back to slice of known size: {e:?}",
                hex::encode(self.0),
            )
        })
    }
}

impl From<&SecretKey<'_>> for PublicKey {
    fn from(sk: &SecretKey<'_>) -> Self {
        PublicKey(sk.0.to_pk())
    }
}

impl From<&[u8; PublicKey::SIZE]> for PublicKey {
    fn from(bytes: &[u8; PublicKey::SIZE]) -> Self {
        PublicKey(kes::PublicKey::from_bytes(bytes).unwrap_or_else(|e| {
            unreachable!(
                "Impossible! Failed to create a KES public key from a slice ({}) of known size: {e:?}",
                hex::encode(bytes)
            )
        }))
    }
}

impl TryFrom<&[u8]> for PublicKey {
    type Error = TryFromSliceError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self::from(<&[u8; Self::SIZE]>::try_from(bytes)?))
    }
}

// ------------------------------------------------------------------- Signature

/// KES signature
pub struct Signature(Sum6KesSig);

impl Signature {
    /// Size of a KES signature, in bytes;
    pub const SIZE: usize = Sum6KesSig::SIZE;

    /// Verify the KES signature
    pub fn verify(&self, kes_period: u32, kes_pk: &PublicKey, msg: &[u8]) -> Result<(), Error> {
        Ok(self.0.verify(kes_period, &kes_pk.0, msg)?)
    }
}

impl From<&[u8; Self::SIZE]> for Signature {
    fn from(bytes: &[u8; Self::SIZE]) -> Self {
        Signature(Sum6KesSig::from_bytes(bytes).unwrap_or_else(|e| {
            unreachable!("Impossible! Failed to create a KES signature from a slice ({}) of known size: {e:?}", hex::encode(bytes))
        }))
    }
}

impl TryFrom<&[u8]> for Signature {
    type Error = TryFromSliceError;

    fn try_from(bytes: &[u8]) -> Result<Self, Self::Error> {
        Ok(Self::from(<&[u8; Self::SIZE]>::try_from(bytes)?))
    }
}

impl From<&Signature> for [u8; 448] {
    fn from(sig: &Signature) -> Self {
        sig.0.to_bytes()
    }
}

// ----------------------------------------------------------------------- Error

/// KES error
#[derive(Error, Debug)]
pub enum Error {
    #[error("KES error: {0}")]
    Kes(#[from] kes_summed_ed25519::errors::Error),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kes_key_evolution() {
        let mut kes_sk_bytes = hex::decode(
            "68b77b6e61925be0499d1445fd9210cec5bdfd5dd92662802eb2720ff70bc68fd89\
             64580ff18bd2b232eb716dfbbeef82e2844b466ddd5dacaad9f15d3c753b3483541\
             41e973d039b1147c48e71e5b7cadc6deb28c86e4ae4fc26e8bbe1695c3374d4eb10\
             94a7a698722894301546466c750947778b18ac3270397efd2eced4d25ced55d2bd2\
             c09e7c0fa7b849d41787ca11defc91609d930a9870881a56a587bff20b2c5c59f63\
             ccb008be495917da3fcae536d05401b6771bb1f9356f031b3ddadbffbc426a9a23e\
             34274b187f7e93892e990644f6273772a02d3e38bee7459ed6a9bb5760fe012e47a\
             2e75880125e7fb072b2b7a626a5375e2039d8d748cb8ad4dd02697250d3155eee39\
             308ecc2925405a8c15e1cbe556cc4315d43ee5101003639bcb33bd6e27da3885888\
             d7cca20b05cadbaa53941ef5282cde8f377c3bd0bf732cfac6b5d4d5597a1f72d81\
             bc0d8af634a4c760b309fe8959bbde666ff10310377b313860bd52d56fd7cb14963\
             3beb1eb2e0076111df61e570a042f7cebae74a8de298a6f114938946230db42651e\
             a4eddf5df2d7d2f3016464073da8a9dc715817b43586a61874e576da7b47a2bb6c2\
             e19d4cbd5b1b39a24427e89b812cce6d30e0506e207f1eaab313c45a236068ea319\
             958474237a5ffe02736e1c51c02a05999816c9253a557f09375c83acf5d7250f3bb\
             c638e10c58fb274e2002eed841ecef6a9cbc57c3157a7c3cf47e66b1741e8173b66\
             76ac973bc9715027a3225087cabad45407b891416330485891dc9a3875488a26428\
             d20d581b629a8f4f42e3aa00cbcaae6c8e2b8f3fe033b874d1de6a3f8c321c92b77\
             643f00d28e",
        )
        .unwrap();

        let mut kes_sk = SecretKey::from_bytes(&mut kes_sk_bytes).unwrap();
        assert_eq!(
            hex::encode(PublicKey::from(&kes_sk)),
            "2e5823037de29647e495b97d9dd7bf739f7ebc11d3701c8d0720f55618e1b292"
        );

        assert_eq!(kes_sk.get_period(), 0);
        insta::assert_snapshot!(hex::encode(unsafe { kes_sk.leak_into_bytes() }));

        kes_sk.update().unwrap();
        assert_eq!(kes_sk.get_period(), 1);
        insta::assert_snapshot!(hex::encode(unsafe { kes_sk.leak_into_bytes() }));

        kes_sk.update().unwrap();
        assert_eq!(kes_sk.get_period(), 2);
        insta::assert_snapshot!(hex::encode(unsafe { kes_sk.leak_into_bytes() }));
    }

    #[test]
    fn kes_signature_verify() {
        let kes_pk_bytes =
            hex::decode("2e5823037de29647e495b97d9dd7bf739f7ebc11d3701c8d0720f55618e1b292")
                .unwrap();
        let kes_pk = PublicKey::try_from(&kes_pk_bytes[..]).unwrap();
        let kes_signature_bytes = hex::decode(
            "20f1c8f9ae672e6ec75b0aa63a85e7ab7865b95f6b2907a26b54c14f49184ab52cf\
             98ef441bb71de50380325b34f16d84fc78d137467a1b49846747cf8ee4701c56f08\
             f198b94c468d46b67b271f5bc30ab2ad14b1bdbf2be0695a00fe4b02b3060fa5212\
             8f4cce9c5759df0ba8d71fe99456bd2e333671e45110908d03a2ec3b38599d26adf\
             182ba63f79900fdb2732947cf8e940a4cf1e8db9b4cf4c001dbd37c60d0e38851de\
             4910807896153be455e13161342d4c6f7bb3e4d2d35dbbbba0ebcd161be2f1ec030\
             d2f5a6059ac89dfa70dc6b3d0bc2da179c62ae95c4f9c7ad9c0387b35bf2b45b325\
             d1e0a18c0c783a0779003bf23e7a6b00cc126c5e3d51a57d41ff1707a76fb2c306a\
             67c21473b41f1d9a7f64a670ec172a2421da03d796fa97086de8812304f4f96bd45\
             243d0a2ad6c48a69d9e2c0afbb1333acee607d18eb3a33818c3c9d5bb72cade8893\
             79008bf60d436298cb0cfc6159332cb1af1de4f1d64e79c399d058ac4993704eed6\
             7917093f89db6cde830383e69aa400ba3225087cabad45407b891416330485891dc\
             9a3875488a26428d20d581b629a8f4f42e3aa00cbcaae6c8e2b8f3fe033b874d1de\
             6a3f8c321c92b77643f00d28e",
        )
        .unwrap();
        let kes_signature = Signature::try_from(&kes_signature_bytes[..]).unwrap();
        let kes_period = 36u32;
        let kes_msg = hex::decode(
            "8a1a00a50f121a0802d24458203deea82abe788d260b8987a522aadec86c9f098e8\
             8a57d7cfcdb24f474a7afb65820cad3c900ca6baee9e65bf61073d900bfbca458ee\
             ca6d0b9f9931f5b1017a8cd65820576d49e98adfab65623dc16f9fff2edd210e8dd\
             1d4588bfaf8af250beda9d3c7825840d944b8c81000fc1182ec02194ca9eca510fd\
             84995d22bfe1842190b39d468e5ecbd863969e0c717b0071a371f748d44c895fa92\
             33094cefcd3107410baabb19a5850f2a29f985d37ca8eb671c2847fab9cc45c9373\
             8a430b4e43837e7f33028b190a7e55152b0e901548961a66d56eebe72d616f9e68f\
             d13e9955ccd8611c201a5b422ac8ef56af74cb657b5b868ce9d850f1945d1582063\
             9d4986d17de3cac8079a3b25d671f339467aa3a9948e29992dafebf90f719f84582\
             02e5823037de29647e495b97d9dd7bf739f7ebc11d3701c8d0720f55618e1b29217\
             1903e958401feeeabc7460b19370f4050e986b558b149fdc8724b4a4805af8fe45c\
             8e7a7c6753894ad7a1b9c313da269ddc5922e150da3b378977f1dfea79fc52fd2c1\
             2f08820901",
        )
        .unwrap();
        assert!(kes_signature.verify(kes_period, &kes_pk, &kes_msg).is_ok());
    }
}
