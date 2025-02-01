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

use crate::{
    kes::{KesPublicKey, KesSignature},
    ledger::{issuer_vkey_to_pool_id, LedgerState, PoolId},
    validator::{ValidationError, Validator},
    vrf::{VrfProof, VrfProofBytes, VrfProofHashBytes, VrfPublicKey, VrfPublicKeyBytes},
};
use pallas_crypto::{
    hash::{Hash, Hasher},
    key::ed25519::{PublicKey, Signature},
};
use pallas_math::math::{ExpOrdering, FixedDecimal, FixedPrecision};
use pallas_primitives::{
    babbage,
    babbage::{derive_tagged_vrf_output, VrfDerivation},
};
use rayon::prelude::*;
use std::{ops::Deref, sync::LazyLock};
use tracing::{span, trace};

/// The certified natural max value represents 2^256 in praos consensus
static CERTIFIED_NATURAL_MAX: LazyLock<FixedDecimal> = LazyLock::new(|| {
    FixedDecimal::from_str(
        "1157920892373161954235709850086879078532699846656405640394575840079131296399360000000000000000000000000000000000",
        34,
    )
        .expect("Infallible")
});

/// Validator for a block using praos consensus.
pub struct BlockValidator<'b> {
    header: &'b babbage::MintedHeader<'b>,
    ledger_state: &'b dyn LedgerState,
    epoch_nonce: &'b Hash<32>,
    active_slots_coeff: &'b FixedDecimal,
}

impl<'b> BlockValidator<'b> {
    pub fn new(
        header: &'b babbage::MintedHeader,
        ledger_state: &'b dyn LedgerState,
        epoch_nonce: &'b Hash<32>,
        active_slots_coeff: &'b FixedDecimal,
    ) -> Self {
        Self {
            header,
            ledger_state,
            epoch_nonce,
            active_slots_coeff,
        }
    }

    fn validate_babbage_compatible(&self) -> Result<(), ValidationError> {
        let span = span!(tracing::Level::TRACE, "validate_babbage_compatible");
        let _enter = span.enter();

        // Grab all the values we need to validate the block
        let absolute_slot = self.header.header_body.slot;
        let issuer_vkey = &self.header.header_body.issuer_vkey;
        let pool_id: PoolId = issuer_vkey_to_pool_id(issuer_vkey);
        let vrf_vkey: VrfPublicKeyBytes =
            (&self.header.header_body.vrf_vkey)
                .try_into()
                .map_err(|error| {
                    ValidationError::GenericValidationError(format!("vrf_vkey: {}", error))
                })?;

        let leader_vrf_output = &self.header.header_body.leader_vrf_output();
        let sigma: FixedDecimal = self.ledger_state.pool_id_to_sigma(&pool_id).map(|sigma| {
            FixedDecimal::from(sigma.numerator) / FixedDecimal::from(sigma.denominator)
        })?;

        let block_vrf_proof_hash: VrfProofHashBytes = (&self.header.header_body.vrf_result.0)
            .try_into()
            .map_err(|error| {
                ValidationError::GenericValidationError(format!("block_vrf_proof_hash: {}", error))
            })?;
        let block_vrf_proof: VrfProofBytes = (&self.header.header_body.vrf_result.1)
            .try_into()
            .map_err(|error| {
                ValidationError::GenericValidationError(format!("block_vrf_proof: {}", error))
            })?;
        let kes_signature = self.header.body_signature.as_slice();

        trace!("pool_id: {}", pool_id);
        trace!("block vrf_vkey: {}", hex::encode(vrf_vkey));
        trace!("sigma: {}", sigma);
        trace!("absolute_slot: {}", absolute_slot);
        trace!("leader_vrf_output: {}", hex::encode(leader_vrf_output));
        trace!(
            "block_vrf_proof_hash: {}",
            hex::encode(block_vrf_proof_hash.as_slice())
        );
        trace!(
            "block_vrf_proof: {}",
            hex::encode(block_vrf_proof.as_slice())
        );
        trace!("kes_signature: {}", hex::encode(kes_signature));

        let validation_checks: Vec<Box<dyn Fn() -> Result<(), ValidationError> + Send + Sync>> = vec![
            Box::new(|| self.validate_ledger_matches_block_vrf_key_hash(&pool_id, &vrf_vkey)),
            Box::new(|| {
                self.validate_block_vrf(
                    absolute_slot,
                    &vrf_vkey,
                    leader_vrf_output,
                    &sigma,
                    block_vrf_proof_hash,
                    &block_vrf_proof,
                )
            }),
            Box::new(|| self.validate_operational_certificate(issuer_vkey.as_slice(), &pool_id)),
            Box::new(|| self.validate_kes_signature(absolute_slot, kes_signature)),
        ];

        validation_checks
            .into_par_iter()
            .try_for_each(|check| check())?;

        Ok(())
    }

    fn validate_kes_signature(
        &self,
        absolute_slot: u64,
        kes_signature: &[u8],
    ) -> Result<(), ValidationError> {
        // Verify the KES signature
        let kes_pk = KesPublicKey::from_bytes(
            &self
                .header
                .header_body
                .operational_cert
                .operational_cert_hot_vkey,
        )
        .map_err(|error| ValidationError::GenericValidationError(format!("kes_pk: {}", error)))?;

        // calculate the right KES period to verify the signature
        let slot_kes_period = self.ledger_state.slot_to_kes_period(absolute_slot);
        let opcert_kes_period = self
            .header
            .header_body
            .operational_cert
            .operational_cert_kes_period;

        if opcert_kes_period > slot_kes_period {
            return Err(ValidationError::OpCertKesPeriodTooLarge {
                opcert_kes_period,
                slot_kes_period,
            });
        }
        if slot_kes_period >= opcert_kes_period + self.ledger_state.max_kes_evolutions() {
            return Err(ValidationError::KesVerificationError(
                "Operational Certificate KES period is too old!".to_string(),
            ));
        }

        let kes_period = (slot_kes_period - opcert_kes_period) as u32;
        trace!("kes_period: {}", kes_period);

        // The header_body_cbor was signed by the KES private key. Verify this with the KES public key
        let header_body_cbor = self.header.header_body.raw_cbor();
        let kes_signature = KesSignature::from_bytes(kes_signature).map_err(|error| {
            ValidationError::GenericValidationError(format!("kes_signature: {}", error))
        })?;

        kes_signature
            .verify(kes_period, &kes_pk, header_body_cbor)
            .map_err(|error| {
                ValidationError::KesVerificationError(format!(
                    "KES signature verification failed: {}",
                    error
                ))
            })
    }

    fn validate_operational_certificate(
        &self,
        issuer_vkey: &[u8],
        pool_id: &PoolId,
    ) -> Result<(), ValidationError> {
        // Verify the Operational Certificate signature
        let opcert_signature = Signature::try_from(
            self.header
                .header_body
                .operational_cert
                .operational_cert_sigma
                .as_slice(),
        )
        .map_err(|error| {
            ValidationError::GenericValidationError(format!("opcert_signature: {}", error))
        })?;
        let cold_pk = PublicKey::try_from(issuer_vkey).map_err(|error| {
            ValidationError::GenericValidationError(format!("cold_pk: {}", error))
        })?;

        let opcert_sequence_number = self
            .header
            .header_body
            .operational_cert
            .operational_cert_sequence_number;

        // Check the sequence number of the operational certificate. It should either be the same
        // as the latest known sequence number for the issuer_vkey or one greater.
        match self.ledger_state.latest_opcert_sequence_number(pool_id) {
            Some(latest_opcert_sequence_number) => {
                if opcert_sequence_number < latest_opcert_sequence_number {
                    return Err(ValidationError::InvalidOpcertSequenceNumber("Operational Certificate sequence number is less than the latest known sequence number!".to_string()));
                } else if (opcert_sequence_number - latest_opcert_sequence_number) > 1 {
                    return Err(ValidationError::InvalidOpcertSequenceNumber("Operational Certificate sequence number is too far ahead of the latest known sequence number!".to_string()));
                }
                trace!("Operational Certificate sequence number is ok.")
            }
            None => {
                trace!("No latest known opcert sequence number for the issuer_vkey");
            }
        }

        // The opcert message is a concatenation of the KES vkey, the sequence number, and the kes period
        let mut opcert_message = Vec::new();
        opcert_message.extend_from_slice(
            &self
                .header
                .header_body
                .operational_cert
                .operational_cert_hot_vkey,
        );
        opcert_message.extend_from_slice(
            &self
                .header
                .header_body
                .operational_cert
                .operational_cert_sequence_number
                .to_be_bytes(),
        );
        opcert_message.extend_from_slice(
            &self
                .header
                .header_body
                .operational_cert
                .operational_cert_kes_period
                .to_be_bytes(),
        );

        if cold_pk.verify(&opcert_message, &opcert_signature) {
            Ok(())
        } else {
            Err(ValidationError::InvalidOpcertSignature)
        }
    }

    fn validate_block_vrf(
        &self,
        absolute_slot: u64,
        vrf_vkey: &VrfPublicKeyBytes,
        leader_vrf_output: &Vec<u8>,
        sigma: &FixedDecimal,
        block_vrf_proof_hash: VrfProofHashBytes,
        block_vrf_proof: &VrfProofBytes,
    ) -> Result<(), ValidationError> {
        // Calculate the VRF input seed so we can verify the VRF output against it.
        let vrf_input_seed = self.mk_vrf_input(absolute_slot, self.epoch_nonce.as_ref());
        trace!("vrf_input_seed: {}", vrf_input_seed);

        // Verify the VRF proof
        let vrf_proof = VrfProof::from(block_vrf_proof);
        let vrf_vkey = VrfPublicKey::from(vrf_vkey);
        let proof_hash = vrf_proof.verify(&vrf_vkey, vrf_input_seed.as_ref())?;
        if proof_hash.as_slice() != block_vrf_proof_hash.as_slice() {
            Err(ValidationError::InvalidVrfProofHash(
                hex::encode(block_vrf_proof_hash.as_slice()),
                hex::encode(proof_hash.as_slice()),
            ))
        } else {
            // The proof was valid. Make sure that our leader_vrf_output matches what was in the block
            trace!("certified_proof_hash: {}", hex::encode(proof_hash));
            let calculated_leader_vrf_output =
                derive_tagged_vrf_output(proof_hash.as_slice(), VrfDerivation::Leader);
            if calculated_leader_vrf_output.as_slice() != leader_vrf_output.as_slice() {
                Err(ValidationError::InvalidVrfLeaderHash(
                    hex::encode(leader_vrf_output),
                    hex::encode(calculated_leader_vrf_output),
                ))
            } else {
                // The leader VRF output hash matches what was in the block
                // Now we need to check if the pool had enough sigma stake to produce this block
                self.validate_pool_meets_delegation_threshold(
                    sigma,
                    absolute_slot,
                    leader_vrf_output.as_slice(),
                )
            }
        }
    }

    /// Verify that the pool meets the delegation threshold
    fn validate_pool_meets_delegation_threshold(
        &self,
        sigma: &FixedDecimal,
        absolute_slot: u64,
        leader_vrf_output: &[u8],
    ) -> Result<(), ValidationError> {
        // special case for testing purposes
        if self.active_slots_coeff == &FixedDecimal::from(1u64) {
            return Ok(());
        }

        let certified_leader_vrf: FixedDecimal = leader_vrf_output.into();
        let denominator = CERTIFIED_NATURAL_MAX.deref() - &certified_leader_vrf;
        let recip_q = CERTIFIED_NATURAL_MAX.deref() / &denominator;
        let c = (FixedDecimal::from(1u64) - self.active_slots_coeff.clone()).ln();
        let x = -(sigma * &c);

        trace!("leader_vrf_output: {}", hex::encode(leader_vrf_output));
        trace!("certified_leader_vrf: {}", certified_leader_vrf);
        trace!("denominator: {}", denominator);
        trace!("recip_q: {}", recip_q);
        trace!("active_slots_coeff: {}", self.active_slots_coeff);
        trace!("x: {}", x);

        let ordering = x.exp_cmp(1000, 3, &recip_q);
        match ordering.estimation {
            ExpOrdering::LT => {
                trace!(
                    "Slot: {} - IS Leader: {} < {}",
                    absolute_slot,
                    recip_q,
                    ordering.approx
                );
                Ok(())
            }
            _ => {
                trace!(
                    "Slot: {} - NOT Leader: {} >= {}",
                    absolute_slot,
                    recip_q,
                    ordering.approx
                );
                Err(ValidationError::InsufficientPoolStake)
            }
        }
    }

    /// Validate that the VRF key hash in the block matches the VRF key hash in the ledger
    fn validate_ledger_matches_block_vrf_key_hash(
        &self,
        pool_id: &PoolId,
        vrf_vkey: &VrfPublicKeyBytes,
    ) -> Result<(), ValidationError> {
        let vrf_vkey_hash: Hash<32> = Hasher::<256>::hash(vrf_vkey);
        trace!("block vrf_vkey_hash: {}", hex::encode(vrf_vkey_hash));
        let ledger_vrf_vkey_hash = self.ledger_state.vrf_vkey_hash(pool_id)?;
        if vrf_vkey_hash != ledger_vrf_vkey_hash {
            return Err(ValidationError::InvalidVrfKeyForPool {
                key_hash_from_ledger: hex::encode(ledger_vrf_vkey_hash),
                key_hash_from_block: hex::encode(vrf_vkey),
            });
        }
        Ok(())
    }

    fn mk_vrf_input(&self, absolute_slot: u64, eta0: &[u8]) -> Hash<32> {
        trace!("mk_vrf_input() absolute_slot {}", absolute_slot);
        let mut hasher = Hasher::<256>::new();
        hasher.input(&absolute_slot.to_be_bytes());
        hasher.input(eta0);
        hasher.finalize()
    }
}

impl Validator for BlockValidator<'_> {
    fn validate(&self) -> Result<(), ValidationError> {
        self.validate_babbage_compatible()
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        consensus::BlockValidator,
        ledger::{MockLedgerState, PoolId, PoolSigma},
        validator::Validator,
    };
    use ctor::ctor;
    use mockall::predicate::eq;
    use pallas_crypto::hash::Hash;
    use pallas_math::math::FixedDecimal;
    use pallas_traverse::MultiEraHeader;

    #[ctor]
    fn init() {
        // set rust log level to TRACE
        // std::env::set_var("RUST_LOG", "trace");
        // initialize tracing crate
        // tracing_subscriber::fmt::init();
    }

    #[test]
    fn test_validate_conway_block() {
        let test_block = include_bytes!("../../tests/data/mainnet_blockheader_10817298.cbor");
        let test_block_hex = hex::encode(test_block);
        insta::assert_snapshot!(test_block_hex);
        let test_vector = vec![
            (
                "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
                "c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b",
                "c7937fc47fecbe687891b3decd71e904d1e129598aa3852481d295eea3ea3ada",
                25626202470912_u64,
                22586623335121436_u64,
                true,
            ),
            (
                "00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114",
                "c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b",
                "c7937fc47fecbe687891b3decd71e904d1e129598aa3852481d295eea3ea3ada",
                6026202470912_u64,
                22586623335121436_u64,
                false,
            ),
        ];
        insta::assert_yaml_snapshot!(test_vector);

        for (pool_id_str, vrf_vkey_hash_str, epoch_nonce_str, numerator, denominator, expected) in
            test_vector
        {
            let pool_id: PoolId = pool_id_str.parse().unwrap();
            let vrf_vkey_hash: Hash<32> = vrf_vkey_hash_str.parse().unwrap();
            let epoch_nonce: Hash<32> = epoch_nonce_str.parse().unwrap();

            let active_slots_coeff: FixedDecimal =
                FixedDecimal::from(5u64) / FixedDecimal::from(100u64);
            let conway_block_tag: u8 = 6;
            let multi_era_header =
                MultiEraHeader::decode(conway_block_tag, None, test_block).unwrap();
            let babbage_header = multi_era_header.as_babbage().expect("Infallible");
            assert_eq!(babbage_header.header_body.slot, 134402628u64);

            let mut ledger_state = MockLedgerState::new();
            ledger_state
                .expect_pool_id_to_sigma()
                .with(eq(pool_id))
                .returning(move |_| {
                    Ok(PoolSigma {
                        numerator,
                        denominator,
                    })
                });
            ledger_state
                .expect_vrf_vkey_hash()
                .with(eq(pool_id))
                .returning(move |_| Ok(vrf_vkey_hash));
            ledger_state.expect_slot_to_kes_period().returning(|slot| {
                // hardcode some values from shelley-genesis.json for the mock implementation
                let slots_per_kes_period: u64 = 129600; // from shelley-genesis.json (1.5 days in seconds)
                slot / slots_per_kes_period
            });
            ledger_state.expect_max_kes_evolutions().returning(|| 62);
            ledger_state
                .expect_latest_opcert_sequence_number()
                .returning(|_| None);

            let block_validator = BlockValidator::new(
                babbage_header,
                &ledger_state,
                &epoch_nonce,
                &active_slots_coeff,
            );
            assert_eq!(block_validator.validate().is_ok(), expected);
        }
    }
}
