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
    Ed25519Signature, KesPeriod, KesSignature, OperationalCert, VerificationKey,
    ed25519::{self, Signer},
};
use amaru_ouroboros::{kes, vrf};
use amaru_ouroboros_traits::{ForgingCredentials, ForgingCredentialsError, IssuerFields};

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

    pub fn vrf_verification_key(&self) -> VerificationKey {
        VerificationKey::from(*vrf::PublicKey::from(&vrf::SecretKey::from(&self.vrf_seed)))
    }

    pub fn operational_cert(&self) -> &OperationalCert {
        &self.ocert
    }
}

impl ForgingCredentials for TestCredentials {
    fn issuer_verification_key(&self) -> VerificationKey {
        VerificationKey::from(self.cold.verifying_key().to_bytes())
    }

    fn vrf_secret_bytes(&self) -> [u8; 32] {
        self.vrf_seed
    }

    fn issuer_fields(&self) -> IssuerFields {
        IssuerFields {
            issuer_verification_key: self.issuer_verification_key(),
            vrf_verification_key: self.vrf_verification_key(),
            operational_cert: self.operational_cert().clone(),
        }
    }

    fn sign(&self, period: KesPeriod, message: &[u8]) -> Result<KesSignature, ForgingCredentialsError> {
        let evolution = period.evolutions_since(self.ocert.operational_cert_kes_period, self.max_kes_evolutions)?;
        let mut kes = self.kes.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        kes.evolve_to(evolution).map_err(|error| {
            if matches!(error, kes::KesError::CannotEvolveBackwards { .. }) {
                ForgingCredentialsError::EvolvedPast(error.to_string())
            } else {
                ForgingCredentialsError::Kes(error.to_string())
            }
        })?;
        Ok(KesSignature::from(<[u8; kes::Signature::SIZE]>::from(&kes.sign(message))))
    }
}

#[cfg(test)]
mod tests {
    use amaru_kernel::{
        Bytes, Hash, Header, ProtocolVersion, Slot, VrfCert, cardano::fixed_bytes::FixedBytes, size::BLOCK_BODY,
    };
    use amaru_ouroboros::praos::header::{AssertKesSignatureError, AssertOperationalCertificateError};
    use amaru_ouroboros_traits::{HeaderDraft, kes_message};

    use super::*;

    fn draft(block_number: u64) -> HeaderDraft {
        HeaderDraft {
            block_number,
            slot: Slot::from(1),
            prev_hash: None,
            vrf_result: VrfCert { output: Bytes::default(), proof: FixedBytes::<80>::zeroes() },
            block_body_size: 0,
            block_body_hash: Hash::new([0u8; BLOCK_BODY]),
            protocol_version: ProtocolVersion::new(11, 0),
        }
    }

    fn verify(credentials: &TestCredentials, period: KesPeriod, header: &Header, max: u64) {
        let ocert = &header.body().operational_cert;
        let hot = kes::PublicKey::from(ocert.operational_cert_hot_verification_key.as_array());
        let signature = kes::Signature::from(header.signature().as_array());
        assert_eq!(ocert, credentials.operational_cert());
        AssertKesSignatureError::new(period, ocert.operational_cert_kes_period, header.body(), &hot, &signature, max)
            .unwrap();
    }

    fn signed(
        credentials: &TestCredentials,
        period: KesPeriod,
        block_number: u64,
    ) -> Result<Header, ForgingCredentialsError> {
        let body = draft(block_number).body(&credentials.issuer_fields());
        let signature = credentials.sign(period, &kes_message(&body))?;
        Ok(Header::new(body, signature))
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
        AssertOperationalCertificateError::new(credentials.operational_cert(), &cold.verifying_key(), None).unwrap();
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

        let at_start = signed(&credentials, start, 1).unwrap();
        verify(&credentials, start, &at_start, max);
        assert_eq!(at_start.body().issuer_verification_key, credentials.issuer_verification_key());
        assert_eq!(at_start.body().vrf_verification_key, credentials.vrf_verification_key());

        let later = KesPeriod::from(5);
        let at_later = signed(&credentials, later, 2).unwrap();
        verify(&credentials, later, &at_later, max);
    }

    #[test]
    fn signing_before_the_certificate_or_past_its_window_fails() {
        let start = KesPeriod::from(3);
        let credentials = TestCredentials::for_test_keys(start, 4);
        assert!(matches!(credentials.sign(KesPeriod::from(2), &[]), Err(ForgingCredentialsError::Period(_))));
        assert!(matches!(credentials.sign(KesPeriod::from(7), &[]), Err(ForgingCredentialsError::Period(_))));
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
    fn signing_backwards_fails_after_the_key_evolved() {
        let credentials = TestCredentials::for_test_keys(KesPeriod::from(0), 62);
        credentials.sign(KesPeriod::from(3), &[]).unwrap();
        assert!(matches!(credentials.sign(KesPeriod::from(2), &[]), Err(ForgingCredentialsError::EvolvedPast(_))));
    }
}
