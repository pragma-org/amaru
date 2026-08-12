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

use std::{
    array::TryFromSliceError,
    ops::{Deref, Neg},
    str::FromStr,
    sync::{Arc, LazyLock},
};

use amaru_kernel::{
    ConsensusParameters, Hash, Hasher, Header, HeaderBody, HeaderHash, IsHeader, Nonce, OperationalCert, Slot, VrfCert,
    ed25519,
    maths::{ExpOrdering, FixedDecimal},
    to_cbor,
};
use amaru_ouroboros_traits::{PoolSummary, has_stake_distribution::GetPoolError};
use thiserror::Error;

use crate::{kes, vrf};

/// The certified natural max value represents 2^256 in praos consensus
///
/// TODO: Ideally, we should replace the use of dashu  with num-bigint
/// since it has become our weapon of choice within Amaru. Mixing maths libraries is
/// a recipe for mistakes.
#[expect(clippy::expect_used)]
static CERTIFIED_NATURAL_MAX: LazyLock<FixedDecimal> = LazyLock::new(|| {
    FixedDecimal::from_str("115792089237316195423570985008687907853269984665640564039457584007913129639936")
        .expect("Infallible")
});

// ------------------------------------------------------------------ assert_all

#[derive(Error, Debug, serde::Serialize, serde::Deserialize)]
pub enum AssertHeaderError {
    #[error("{0}")]
    KnownLeaderVrf(#[from] AssertKnownLeaderVrfError),
    #[error("{0}")]
    VrfProof(#[from] AssertVrfProofError),
    #[error("{0}")]
    LeaderStake(#[from] AssertLeaderStakeError),
    #[error("{0}")]
    KesSignature(#[from] AssertKesSignatureError),
    #[error("{0}")]
    OperationalCertificate(#[from] AssertOperationalCertificateError),
    #[error("could not convert slice to array")]
    TryFromSliceError,
    #[error("cannot convert bytes into valid Ed25519 public key")]
    InvalidEd25519PublicKey,
    #[error("{0}")]
    PoolError(GetPoolError),
}

impl From<TryFromSliceError> for AssertHeaderError {
    fn from(_: TryFromSliceError) -> Self {
        Self::TryFromSliceError
    }
}

impl PartialEq for AssertHeaderError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::KnownLeaderVrf(l0), Self::KnownLeaderVrf(r0)) => l0 == r0,
            (Self::VrfProof(l0), Self::VrfProof(r0)) => l0 == r0,
            (Self::LeaderStake(l0), Self::LeaderStake(r0)) => l0 == r0,
            (Self::KesSignature(l0), Self::KesSignature(r0)) => l0 == r0,
            (Self::OperationalCertificate(l0), Self::OperationalCertificate(r0)) => l0 == r0,
            (Self::InvalidEd25519PublicKey, Self::InvalidEd25519PublicKey) => true,
            (Self::TryFromSliceError, Self::TryFromSliceError) => true,
            _ => false,
        }
    }
}

pub type Assertion<'a> = Box<dyn Fn() -> Result<(), AssertHeaderError> + Send + Sync + 'a>;

#[expect(clippy::result_large_err)]
pub fn assert_all<'a>(
    consensus_parameters: Arc<ConsensusParameters>,
    header: &'a Header,
    last_opcert_sequence_number: Option<u64>,
    pool_summary: &'a PoolSummary,
    epoch_nonce: &'a Nonce,
) -> Result<Vec<Assertion<'a>>, AssertHeaderError> {
    // Grab all the values we need to validate the block
    let absolute_slot = header.slot();
    let issuer = ed25519::VerifyingKey::try_from(&header.body().issuer_verification_key[..])
        .map_err(|_| AssertHeaderError::InvalidEd25519PublicKey)?;

    // TODO: Pallas should hold sized slices
    let declared_vrf_key: &'a [u8; vrf::PublicKey::SIZE] = header.body().vrf_verification_key[..].try_into()?;

    let (registered_vrf_key, leader_relative_stake): (Hash<{ vrf::PublicKey::HASH_SIZE }>, FixedDecimal) = {
        let leader_relative_stake = if pool_summary.active_stake == 0 {
            FixedDecimal::ZERO
        } else {
            &FixedDecimal::from(pool_summary.stake) / &FixedDecimal::from(pool_summary.active_stake)
        };
        (pool_summary.vrf, leader_relative_stake)
    };

    let active_slot_coeff = consensus_parameters.active_slot_coeff();
    let slot_to_kes_period = consensus_parameters.slot_to_kes_period(absolute_slot);
    let max_kes_evolutions = consensus_parameters.max_kes_evolutions();

    Ok(vec![
        Box::new(move || {
            AssertKnownLeaderVrfError::new(registered_vrf_key, &vrf::PublicKey::from(declared_vrf_key))?;
            Ok(())
        }),
        Box::new(move || {
            AssertVrfProofError::new(
                absolute_slot,
                epoch_nonce,
                &vrf::PublicKey::from(declared_vrf_key),
                &header.body().vrf_result,
            )?;
            Ok(())
        }),
        Box::new(move || {
            AssertLeaderStakeError::new(
                &active_slot_coeff,
                &leader_relative_stake,
                &FixedDecimal::from(vrf::Derivation::Leader.derive_tagged_vrf_output(header.vrf_output()).as_slice()),
            )?;
            Ok(())
        }),
        Box::new(move || {
            AssertOperationalCertificateError::new(
                &header.body().operational_cert,
                &issuer,
                last_opcert_sequence_number,
            )?;
            Ok(())
        }),
        Box::new(move || {
            let opcert = &header.body().operational_cert;
            AssertKesSignatureError::new(
                slot_to_kes_period,
                opcert.operational_cert_kes_period,
                header.body(),
                &opcert.operational_cert_hot_verification_key[..].try_into()?, // TODO: Pallas should hold sized slices
                &header.signature()[..].try_into()?,                           // TODO: Pallas should hold sized slices
                max_kes_evolutions,
            )?;
            Ok(())
        }),
    ])
}

// ----------------------------------------------------- assert_known_leader_vrf

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[error(
    "declared leader's VRF credentials differs from those registered in the ledger (registered={} vs declared={})",
    hex::encode(&registered_vrf[0..7]),
    hex::encode(&declared_vrf[0..7]),
)]
pub struct AssertKnownLeaderVrfError {
    registered_vrf: Hash<{ vrf::PublicKey::HASH_SIZE }>,
    declared_vrf: Hash<{ vrf::PublicKey::HASH_SIZE }>,
}

impl AssertKnownLeaderVrfError {
    /// Asserts that the declared VRF credentials advertised in a block do indeed match those
    /// registered for the corresponding leader.
    pub fn new(
        registered_vrf_hash: Hash<{ vrf::PublicKey::HASH_SIZE }>,
        declared_vrf: &vrf::PublicKey,
    ) -> Result<(), Self> {
        let declared_vrf_hash = Hasher::<{ 8 * vrf::PublicKey::HASH_SIZE }>::hash(declared_vrf.as_ref());
        if declared_vrf_hash != registered_vrf_hash {
            return Err(Self { registered_vrf: registered_vrf_hash, declared_vrf: declared_vrf_hash });
        }
        Ok(())
    }
}

// ------------------------------------------------------------ assert_vrf_proof

#[derive(Error, Debug, serde::Serialize, serde::Deserialize)]
pub enum AssertVrfProofError {
    #[error("Malformed VRF proof: {0}")]
    MalformedProof(#[from] vrf::ProofFromBytesError),

    #[error("Invalid VRF proof: {0}")]
    InvalidProof(vrf::ProofVerifyError, Slot, HeaderHash, Vec<u8>),

    #[error("could not convert slice to array")]
    TryFromSliceError,

    #[error(
        "Mismatch between the declared VRF proof hash in block ({}) and the computed one ({}).",
        hex::encode(&.declared[0..7]),
        hex::encode(&.computed[0..7]),
    )]
    ProofMismatch {
        #[serde(with = "crate::serde_util::bytes")]
        declared: Box<[u8; vrf::Proof::HASH_SIZE]>,
        #[serde(with = "crate::serde_util::bytes")]
        computed: Box<Hash<{ vrf::Proof::HASH_SIZE }>>,
    },
}

impl From<TryFromSliceError> for AssertVrfProofError {
    fn from(_: TryFromSliceError) -> Self {
        Self::TryFromSliceError
    }
}

impl PartialEq for AssertVrfProofError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MalformedProof(l0), Self::MalformedProof(r0)) => l0 == r0,
            (Self::InvalidProof(l0, l1, l2, l3), Self::InvalidProof(r0, r1, r2, r3)) => {
                l0 == r0 && l1 == r1 && l2 == r2 && l3 == r3
            }
            (Self::TryFromSliceError, Self::TryFromSliceError) => true,
            (
                Self::ProofMismatch { declared: l_declared, computed: l_computed },
                Self::ProofMismatch { declared: r_declared, computed: r_computed },
            ) => l_declared == r_declared && l_computed == r_computed,
            _ => false,
        }
    }
}

impl AssertVrfProofError {
    /// Assert that the VRF output from the block and its corresponding hash.
    pub fn new(
        absolute_slot: Slot,
        epoch_nonce: &Nonce,
        leader_public_key: &vrf::PublicKey,
        vrf_cert: &VrfCert,
    ) -> Result<(), Self> {
        let input = &vrf::Input::new(absolute_slot, epoch_nonce);

        // TODO: Should have fixed size slices here.
        let block_output: [u8; vrf::Proof::HASH_SIZE] = {
            let bytes: &[u8] = vrf_cert.output.as_ref();
            bytes.try_into()
        }?;

        // TODO: Should have fixed size slices here.
        let block_proof: [u8; vrf::Proof::SIZE] = {
            let bytes: &[u8] = vrf_cert.proof.as_ref();
            bytes.try_into()
        }?;

        // Verify the VRF proof
        let vrf_proof = vrf::Proof::try_from(&block_proof)?;
        let computed_output = vrf_proof
            .verify(leader_public_key, input)
            .map_err(|e| Self::InvalidProof(e, absolute_slot, *epoch_nonce, leader_public_key.as_ref().to_vec()))?;
        if computed_output.as_slice() != block_output {
            return Err(Self::ProofMismatch { declared: Box::new(block_output), computed: Box::new(computed_output) });
        }

        Ok(())
    }
}

// ------------------------------------------------------------ assert_leader_stake

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AssertLeaderStakeError {
    #[error("Insufficient leader stake.")]
    InsufficientLeaderStake,
}

impl AssertLeaderStakeError {
    /// Asserts that the leader had enough stake to produce a block.
    pub fn new(
        active_slot_coeff: &FixedDecimal,
        leader_relative_stake: &FixedDecimal,
        certified_leader_vrf: &FixedDecimal,
    ) -> Result<(), Self> {
        if active_slot_coeff >= FixedDecimal::one() {
            return Ok(());
        }
        let denominator = CERTIFIED_NATURAL_MAX.deref() - certified_leader_vrf;
        let recip_q = CERTIFIED_NATURAL_MAX.deref() / &denominator;
        let c = (FixedDecimal::one() - active_slot_coeff).ln();
        let x = (leader_relative_stake * &c).neg();
        let ordering = x.exp_cmp(1000, 3, &recip_q);
        match ordering.estimation {
            ExpOrdering::LT => Ok(()),
            ExpOrdering::GT | ExpOrdering::UNKNOWN => Err(Self::InsufficientLeaderStake),
        }
    }
}

// -------------------------------------------------------- assert_kes_signature

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AssertKesSignatureError {
    #[error(
        "Operational Certificate KES period ({opcert_kes_period}) is greater than the block slot KES period ({slot_kes_period})."
    )]
    OpCertKesPeriodTooLarge { opcert_kes_period: u64, slot_kes_period: u64 },

    #[error("Operational Certificate KES period ({opcert_kes_period}) is too old.")]
    OpCertKesPeriodTooOld { opcert_kes_period: u64, slot_kes_period: u64, max_kes_evolutions: u64 },

    #[error("Invalid KES signature from leader: {reason}")]
    InvalidKesSignature { period: u32, reason: String },
}

impl AssertKesSignatureError {
    /// Asserts the KES signature is valid. Also controls the validity of the KES period.
    pub fn new(
        slot_kes_period: u64,
        opcert_kes_period: u64,
        header_body: &HeaderBody,
        public_key: &kes::PublicKey,
        signature: &kes::Signature,
        max_kes_evolutions: u64,
    ) -> Result<(), Self> {
        if opcert_kes_period > slot_kes_period {
            return Err(Self::OpCertKesPeriodTooLarge { opcert_kes_period, slot_kes_period });
        }

        if slot_kes_period >= opcert_kes_period + max_kes_evolutions {
            return Err(Self::OpCertKesPeriodTooOld { opcert_kes_period, slot_kes_period, max_kes_evolutions });
        }

        let kes_period = (slot_kes_period - opcert_kes_period) as u32;

        signature
            .verify(kes_period, public_key, &to_cbor(header_body))
            .map_err(|error| Self::InvalidKesSignature { period: kes_period, reason: error.to_string() })
    }
}

// ---------------------------------------------- assert_operational_certificate

#[derive(Error, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AssertOperationalCertificateError {
    #[error("Malformed operational certificate signature: {reason}")]
    MalformedSignature { reason: String },

    #[error(
        "Operational certificate sequence number ({}) is too far ahead of the latest known sequence number ({}).",
        declared_sequence_number,
        latest_sequence_number
    )]
    SequenceNumberTooFarAhead { declared_sequence_number: u64, latest_sequence_number: u64 },

    #[error(
        "Operational certificate sequence number ({}) is less than the latest known sequence number ({}).",
        declared_sequence_number,
        latest_sequence_number
    )]
    SequenceNumberTooSmall { declared_sequence_number: u64, latest_sequence_number: u64 },

    #[error(
        "Invalid operational certificate signature from issuer ({})",
        hex::encode(&.issuer.as_ref()[0..7]),
    )]
    InvalidSignature {
        #[serde(with = "crate::serde_util::bytes")]
        issuer: ed25519::VerifyingKey,
    },
}

impl AssertOperationalCertificateError {
    #[expect(clippy::result_large_err)]
    pub fn new(
        certificate: &OperationalCert,
        issuer: &ed25519::VerifyingKey,
        latest_sequence_number: Option<u64>,
    ) -> Result<(), Self> {
        // Verify the Operational Certificate signature
        let signature = ed25519::Signature::try_from(certificate.operational_cert_sigma.as_slice())
            .map_err(|error| Self::MalformedSignature { reason: error.to_string() })?;

        let declared_sequence_number = certificate.operational_cert_sequence_number;

        // A registered pool with no recorded opcert behaves as if its counter were 0, so its
        // first certificate may carry sequence number 0 or 1.
        let latest_sequence_number = latest_sequence_number.unwrap_or(0);
        if declared_sequence_number < latest_sequence_number {
            return Err(Self::SequenceNumberTooSmall { declared_sequence_number, latest_sequence_number });
        }
        if declared_sequence_number - latest_sequence_number > 1 {
            return Err(Self::SequenceNumberTooFarAhead { declared_sequence_number, latest_sequence_number });
        }

        // The opcert message is a concatenation of the KES verification key, the sequence number, and the kes period
        let mut message = Vec::new();
        message.extend_from_slice(&certificate.operational_cert_hot_verification_key[..]);
        message.extend_from_slice(&certificate.operational_cert_sequence_number.to_be_bytes());
        message.extend_from_slice(&certificate.operational_cert_kes_period.to_be_bytes());

        issuer.verify_strict(&message, &signature).map_err(|_| Self::InvalidSignature { issuer: issuer.to_owned() })
    }
}

#[cfg(test)]
mod tests {
    use serde_json;

    use super::*;

    #[test]
    fn test_assert_header_error_serialization_roundtrip() {
        let errors = vec![AssertHeaderError::TryFromSliceError];

        for error in errors {
            // Test JSON serialization
            let json_serialized = serde_json::to_string(&error).unwrap();
            let json_deserialized: AssertHeaderError = serde_json::from_str(&json_serialized).unwrap();
            assert_eq!(error, json_deserialized);
        }
    }

    #[test]
    fn test_assert_known_leader_vrf_error_serialization_roundtrip() {
        let error = AssertKnownLeaderVrfError { registered_vrf: Hash::new([1; 32]), declared_vrf: Hash::new([2; 32]) };

        // Test JSON serialization
        let json_serialized = serde_json::to_string(&error).unwrap();
        let json_deserialized: AssertKnownLeaderVrfError = serde_json::from_str(&json_serialized).unwrap();
        assert_eq!(error, json_deserialized);
    }

    #[test]
    fn test_assert_vrf_proof_error_serialization_roundtrip() {
        let errors = vec![
            AssertVrfProofError::TryFromSliceError,
            AssertVrfProofError::ProofMismatch {
                declared: Box::new([1; 64]), // Proof::HASH_SIZE is 64
                computed: Box::new(Hash::new([2; 64])),
            },
        ];

        for error in errors {
            // Test JSON serialization
            let json_serialized = serde_json::to_string(&error).unwrap();
            let json_deserialized: AssertVrfProofError = serde_json::from_str(&json_serialized).unwrap();
            assert_eq!(error, json_deserialized);
        }
    }

    #[test]
    fn test_assert_leader_stake_error_serialization_roundtrip() {
        let error = AssertLeaderStakeError::InsufficientLeaderStake;

        // Test JSON serialization
        let json_serialized = serde_json::to_string(&error).unwrap();
        let json_deserialized: AssertLeaderStakeError = serde_json::from_str(&json_serialized).unwrap();
        assert_eq!(error, json_deserialized);
    }

    #[test]
    fn test_assert_kes_signature_error_serialization_roundtrip() {
        let errors = vec![
            AssertKesSignatureError::OpCertKesPeriodTooLarge { opcert_kes_period: 100, slot_kes_period: 50 },
            AssertKesSignatureError::OpCertKesPeriodTooOld {
                opcert_kes_period: 50,
                slot_kes_period: 200,
                max_kes_evolutions: 100,
            },
            AssertKesSignatureError::InvalidKesSignature { period: 42, reason: "Invalid signature".to_string() },
        ];

        for error in errors {
            // Test JSON serialization
            let json_serialized = serde_json::to_string(&error).unwrap();
            let json_deserialized: AssertKesSignatureError = serde_json::from_str(&json_serialized).unwrap();
            assert_eq!(error, json_deserialized);
        }
    }

    #[test]
    fn test_assert_operational_certificate_error_serialization_roundtrip() {
        let errors = vec![
            AssertOperationalCertificateError::MalformedSignature { reason: "Invalid format".to_string() },
            AssertOperationalCertificateError::SequenceNumberTooFarAhead {
                declared_sequence_number: 100,
                latest_sequence_number: 50,
            },
            AssertOperationalCertificateError::SequenceNumberTooSmall {
                declared_sequence_number: 25,
                latest_sequence_number: 50,
            },
            AssertOperationalCertificateError::InvalidSignature {
                issuer: ed25519::VerifyingKey::try_from([1; 32].as_slice()).unwrap(),
            },
        ];

        for error in errors {
            // Test JSON serialization
            let json_serialized = serde_json::to_string(&error).unwrap();
            let json_deserialized: AssertOperationalCertificateError = serde_json::from_str(&json_serialized).unwrap();
            assert_eq!(error, json_deserialized);
        }
    }

    #[test]
    fn test_serialization_edge_cases() {
        // Test with empty strings and zero values
        let error = AssertKesSignatureError::InvalidKesSignature { period: 0, reason: "".to_string() };

        let json_serialized = serde_json::to_string(&error).unwrap();
        let json_deserialized: AssertKesSignatureError = serde_json::from_str(&json_serialized).unwrap();
        assert_eq!(error, json_deserialized);

        // Test with maximum values
        let error = AssertKesSignatureError::OpCertKesPeriodTooLarge {
            opcert_kes_period: u64::MAX,
            slot_kes_period: u64::MAX - 1,
        };

        let json_serialized = serde_json::to_string(&error).unwrap();
        let json_deserialized: AssertKesSignatureError = serde_json::from_str(&json_serialized).unwrap();
        assert_eq!(error, json_deserialized);
    }
}
