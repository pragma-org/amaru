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

//! In-memory forging credentials for tests.
//!
//! The cold key is chosen by the caller so each simulated pool has its own identity.
//! The VRF seed defaults to a fixed test key and can be replaced when two pools must
//! not share a leader schedule.

use std::sync::Mutex;

use amaru_kernel::{
    Ed25519Signature, HeaderBody, KesPeriod, KesSignature, OperationalCert, VerificationKey,
    ed25519::{self, Signer},
    to_cbor,
};
use amaru_ouroboros::{kes, vrf};
use amaru_ouroboros_traits::{ForgingCredentials, ForgingCredentialsError};

/// VRF seed used by [`TestCredentials::new`] and the forge-stage schedule fixtures.
pub const TEST_VRF_SEED: [u8; 32] = [7u8; 32];

/// Cold key used by forge-stage tests that do not care which pool they are.
pub const TEST_COLD_KEY: [u8; 32] = [9u8; 32];

pub fn test_vrf_key() -> vrf::SecretKey {
    vrf::SecretKey::from(&TEST_VRF_SEED)
}

/// Block-producer secrets for a single pool.
pub struct TestCredentials {
    cold: ed25519::SigningKey,
    vrf_seed: [u8; 32],
    kes: Mutex<kes::SecretKey>,
    ocert: OperationalCert,
    max_kes_evolutions: u64,
}

impl TestCredentials {
    /// Credentials for `cold`, using [`TEST_VRF_SEED`].
    pub fn new(cold: ed25519::SigningKey, ocert_start_period: KesPeriod, max_kes_evolutions: u64) -> Self {
        Self::with_vrf_seed(cold, TEST_VRF_SEED, ocert_start_period, max_kes_evolutions)
    }

    pub fn with_vrf_seed(
        cold: ed25519::SigningKey,
        vrf_seed: [u8; 32],
        ocert_start_period: KesPeriod,
        max_kes_evolutions: u64,
    ) -> Self {
        let mut kes = kes::SecretKey::for_tests();
        let hot = VerificationKey::from(*kes::PublicKey::from(&mut kes));
        let sequence_number = 0u64;
        let mut message = Vec::with_capacity(48);
        message.extend_from_slice(&hot[..]);
        message.extend_from_slice(&sequence_number.to_be_bytes());
        message.extend_from_slice(&u64::from(ocert_start_period).to_be_bytes());
        let ocert = OperationalCert {
            operational_cert_hot_verification_key: hot,
            operational_cert_sequence_number: sequence_number,
            operational_cert_kes_period: ocert_start_period,
            operational_cert_sigma: Ed25519Signature::from(cold.sign(&message).to_bytes()),
        };
        Self { cold, vrf_seed, kes: Mutex::new(kes), ocert, max_kes_evolutions }
    }

    /// [`TEST_COLD_KEY`] and [`TEST_VRF_SEED`].
    pub fn for_test_keys(ocert_start_period: KesPeriod, max_kes_evolutions: u64) -> Self {
        Self::new(ed25519::SigningKey::from_bytes(&TEST_COLD_KEY), ocert_start_period, max_kes_evolutions)
    }
}

impl ForgingCredentials for TestCredentials {
    fn issuer_verification_key(&self) -> VerificationKey {
        VerificationKey::from(self.cold.verifying_key().to_bytes())
    }

    fn vrf_verification_key(&self) -> VerificationKey {
        VerificationKey::from(*vrf::PublicKey::from(&vrf::SecretKey::from(&self.vrf_seed)))
    }

    fn vrf_secret_bytes(&self) -> [u8; 32] {
        self.vrf_seed
    }

    fn operational_cert(&self) -> OperationalCert {
        self.ocert.clone()
    }

    fn sign(&self, period: KesPeriod, header_body: &HeaderBody) -> Result<KesSignature, ForgingCredentialsError> {
        let evolution = period.evolutions_since(self.ocert.operational_cert_kes_period, self.max_kes_evolutions)?;
        let mut kes = self.kes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        kes.evolve_to(evolution).map_err(|e| ForgingCredentialsError::Kes(e.to_string()))?;
        Ok(KesSignature::from(<[u8; kes::Signature::SIZE]>::from(&kes.sign(&to_cbor(header_body)))))
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        Bytes, Hash, HeaderBody, KesSignature, ProtocolVersion, VrfCert, cardano::fixed_bytes::FixedBytes,
        size::BLOCK_BODY,
    };
    use amaru_ouroboros::praos::header::{AssertKesSignatureError, AssertOperationalCertificateError};

    use super::*;

    fn header_body(credentials: &TestCredentials) -> HeaderBody {
        HeaderBody {
            block_number: 1,
            slot: 1,
            prev_hash: None,
            issuer_verification_key: credentials.issuer_verification_key(),
            vrf_verification_key: credentials.vrf_verification_key(),
            vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::<80>::zeroes() },
            block_body_size: 0,
            block_body_hash: Hash::new([0u8; BLOCK_BODY]),
            operational_cert: credentials.operational_cert(),
            protocol_version: ProtocolVersion::new(11, 0),
        }
    }

    #[test]
    fn cold_key_is_the_issuer_and_signs_the_operational_certificate() {
        let cold = ed25519::SigningKey::from_bytes(&[3u8; 32]);
        let credentials = TestCredentials::new(cold.clone(), KesPeriod::from(4), 62);
        assert_eq!(credentials.issuer_verification_key().as_slice(), cold.verifying_key().as_bytes());
        assert_eq!(
            credentials.pool_id(),
            amaru_kernel::Hasher::<224>::hash(credentials.issuer_verification_key().as_slice())
        );
        AssertOperationalCertificateError::new(&credentials.operational_cert(), &cold.verifying_key(), None).unwrap();
    }

    #[test]
    fn distinct_cold_keys_are_distinct_issuers() {
        let first = TestCredentials::new(ed25519::SigningKey::from_bytes(&[1u8; 32]), KesPeriod::from(0), 62);
        let second = TestCredentials::new(ed25519::SigningKey::from_bytes(&[2u8; 32]), KesPeriod::from(0), 62);
        assert_ne!(first.issuer_verification_key(), second.issuer_verification_key());
        assert_ne!(first.pool_id(), second.pool_id());
        assert_ne!(first.operational_cert().operational_cert_sigma, second.operational_cert().operational_cert_sigma);
    }

    #[test]
    fn signature_verifies_at_the_certificate_period_and_after_evolution() {
        let start = KesPeriod::from(2);
        let max = 62;
        let credentials = TestCredentials::for_test_keys(start, max);
        let hot =
            kes::PublicKey::try_from(credentials.operational_cert().operational_cert_hot_verification_key.as_slice())
                .expect("hot key");

        let at_start = header_body(&credentials);
        let signature = credentials.sign(start, &at_start).unwrap();
        let kes_signature = kes::Signature::try_from(signature.as_slice()).unwrap();
        AssertKesSignatureError::new(start, start, &at_start, &hot, &kes_signature, max).unwrap();

        let later = KesPeriod::from(5);
        let mut at_later = header_body(&credentials);
        at_later.block_number = 2;
        let signature = credentials.sign(later, &at_later).unwrap();
        let kes_signature = kes::Signature::try_from(signature.as_slice()).unwrap();
        AssertKesSignatureError::new(later, start, &at_later, &hot, &kes_signature, max).unwrap();
    }

    #[test]
    fn signing_before_the_certificate_or_past_its_window_fails() {
        let start = KesPeriod::from(3);
        let credentials = TestCredentials::for_test_keys(start, 4);
        let body = header_body(&credentials);
        assert!(matches!(credentials.sign(KesPeriod::from(2), &body), Err(ForgingCredentialsError::Period(_))));
        assert!(matches!(credentials.sign(KesPeriod::from(7), &body), Err(ForgingCredentialsError::Period(_))));
    }

    #[test]
    fn vrf_seed_selects_the_verification_key() {
        let seed = [4u8; 32];
        let credentials = TestCredentials::with_vrf_seed(
            ed25519::SigningKey::from_bytes(&TEST_COLD_KEY),
            seed,
            KesPeriod::from(0),
            62,
        );
        assert_eq!(credentials.vrf_secret_bytes(), seed);
        let expected = VerificationKey::from(*vrf::PublicKey::from(&vrf::SecretKey::from(&seed)));
        assert_eq!(credentials.vrf_verification_key(), expected);
    }

    #[test]
    fn kes_signature_type_matches_the_fixed_width() {
        let credentials = TestCredentials::for_test_keys(KesPeriod::from(0), 62);
        let signature: KesSignature = credentials.sign(KesPeriod::from(0), &header_body(&credentials)).unwrap();
        assert_eq!(signature.as_slice().len(), kes::Signature::SIZE);
    }
}
