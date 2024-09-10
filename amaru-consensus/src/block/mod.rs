use amaru_common::ledger::{PoolInfo, SlotCalculator};
use amaru_common::pool::{issuer_vkey_to_pool_id, PoolId};
use amaru_common::validator::Validator;
use malachite::platform::Limb;
use malachite::{Integer, Natural, Rational};
use malachite_base::num::arithmetic::traits::Ceiling;
use pallas_crypto::hash::{Hash, Hasher};
use pallas_crypto::vrf::{
    VrfProof, VrfPublicKey, VRF_PROOF_HASH_SIZE, VRF_PROOF_SIZE, VRF_PUBLIC_KEY_SIZE,
};
use pallas_math::math::{ExpOrdering, FixedDecimal, FixedPrecision};
use pallas_traverse::MultiEraHeader;
use std::ops::Deref;
use std::sync::LazyLock;
use tracing::{error, span, trace, warn};

// The certified natural max value represents 2^256 in babbage and beyond
static CERTIFIED_NATURAL_MAX: LazyLock<FixedDecimal> = LazyLock::new(|| {
    FixedDecimal::from_str(
        "1157920892373161954235709850086879078532699846656405640394575840079131296399360000000000000000000000000000000000",
        34,
    )
        .expect("Infallible")
});

struct BlockValidator<'b> {
    multi_era_header: &'b MultiEraHeader<'b>,
    pool_info: &'b dyn PoolInfo,
    slot_calculator: &'b dyn SlotCalculator,
    epoch_nonce: &'b Hash<32>,
    decentralization: &'b Rational,
    // c is the ln(1-active_slots_coeff). Usually ln(1-0.05)
    c: &'b FixedDecimal,
}

impl<'b> BlockValidator<'b> {
    // TODO: This function is currently only used in tests. Remove warning suppression in the future
    #[allow(unused)]
    fn new(
        multi_era_header: &'b MultiEraHeader<'b>,
        pool_info: &'b dyn PoolInfo,
        slot_calculator: &'b dyn SlotCalculator,
        epoch_nonce: &'b Hash<32>,
        decentralization: &'b Rational,
        c: &'b FixedDecimal,
    ) -> Self {
        Self {
            multi_era_header,
            pool_info,
            slot_calculator,
            epoch_nonce,
            decentralization,
            c,
        }
    }

    fn validate_epoch_boundary(&self) -> bool {
        // we ignore epoch boundaries and assume they are valid
        true
    }

    fn validate_byron(&self) -> bool {
        // we ignore byron headers and assume they are valid
        true
    }

    fn validate_shelley_compatible(&self) -> bool {
        // we ignore shelley compatible headers and assume they are valid
        true

        // TODO: In the future, we will need to validate this data if we build an archive amaru node.
        // let issuer_vkey = self.multi_era_header.issuer_vkey().expect("Infallible");
        // let pool_id: PoolId = issuer_vkey_to_pool_id(issuer_vkey);
        // let sigma: Sigma = match self.pool_id_to_sigma.sigma(&pool_id) {
        //     Ok(sigma) => sigma,
        //     Err(error) => {
        //         warn!("{:?} - {:?}", error, pool_id);
        //         return false;
        //     }
        // };
        // let absolute_slot = self.multi_era_header.slot();
        // let leader_vrf_output = self.multi_era_header.leader_vrf_output().expect("Infallible");
        // let vrf_vkey = self.multi_era_header.vrf_vkey().expect("Infallible");
        // if self.is_overlay_slot(absolute_slot) {
        //     // This is an overlay block produced by the BFT node
        //     todo!("Overlay block validation");
        // } else {
        //     // This is a block produced by the stake pool
        //     // We must calculate the seed ourselves to verify the VRF output
        //     todo!("Stake pool block validation");
        // }
    }

    fn validate_babbage_compatible(&self) -> bool {
        let span = span!(tracing::Level::TRACE, "validate_babbage_compatible");
        let _enter = span.enter();

        // Grab all the values we need to validate the block
        let issuer_vkey = self.multi_era_header.issuer_vkey().expect("Infallible");
        let pool_id: PoolId = issuer_vkey_to_pool_id(issuer_vkey);
        let vrf_vkey: &[u8; VRF_PUBLIC_KEY_SIZE] = self
            .multi_era_header
            .vrf_vkey()
            .expect("Infallible")
            .try_into()
            .expect("Infallible");
        if !self.ledger_matches_block_vrf_key_hash(&pool_id, vrf_vkey) {
            // Fail fast if the vrf key hash in the block does not match the ledger
            return false;
        }
        let sigma: FixedDecimal = match self.pool_info.sigma(&pool_id) {
            Ok(sigma) => {
                FixedDecimal::from(sigma.numerator) / FixedDecimal::from(sigma.denominator)
            }
            Err(error) => {
                warn!("{:?} - {:?}", error, pool_id);
                return false;
            }
        };
        let absolute_slot = self.multi_era_header.slot();
        let leader_vrf_output = self
            .multi_era_header
            .leader_vrf_output()
            .expect("Infallible");
        let (block_vrf_proof_hash, block_vrf_proof, kes_signature) = {
            let babbage_header = self.multi_era_header.as_babbage().expect("Infallible");
            (
                &babbage_header.header_body.vrf_result.0,
                &babbage_header.header_body.vrf_result.1,
                &babbage_header.body_signature,
            )
        };
        let block_vrf_proof_hash: [u8; VRF_PROOF_HASH_SIZE] = block_vrf_proof_hash
            .as_slice()
            .try_into()
            .expect("Infallible");
        let block_vrf_proof: [u8; VRF_PROOF_SIZE] =
            block_vrf_proof.as_slice().try_into().expect("Infallible");

        trace!("pool_id: {}", pool_id);
        trace!("block vrf_vkey: {}", hex::encode(vrf_vkey));
        trace!("sigma: {}", sigma);
        trace!("absolute_slot: {}", absolute_slot);
        trace!("leader_vrf_output: {}", hex::encode(&leader_vrf_output));
        trace!(
            "block_vrf_proof_hash: {}",
            hex::encode(block_vrf_proof_hash.as_slice())
        );
        trace!(
            "block_vrf_proof: {}",
            hex::encode(block_vrf_proof.as_slice())
        );
        trace!("kes_signature: {}", hex::encode(kes_signature.as_slice()));

        // Calculate the VRF input seed so we can verify the VRF output against it.
        let vrf_input_seed = self.mk_vrf_input(absolute_slot, self.epoch_nonce.as_ref());
        trace!("vrf_input_seed: {}", vrf_input_seed);

        // Verify the VRF proof
        let vrf_proof = VrfProof::from(&block_vrf_proof);
        let vrf_vkey = VrfPublicKey::from(vrf_vkey);
        match vrf_proof.verify(&vrf_vkey, vrf_input_seed.as_ref()) {
            Ok(proof_hash) => {
                if proof_hash.as_slice() != block_vrf_proof_hash.as_slice() {
                    error!("VRF proof hash mismatch");
                    false
                } else {
                    // The proof was valid. Make sure that our leader_vrf_output matches what was in the block
                    trace!("certified_proof_hash: {}", hex::encode(proof_hash));
                    let mut hasher = Hasher::<256>::new();
                    hasher.input(&[0x4C_u8]); /* "L" */
                    hasher.input(proof_hash.as_slice());
                    let calculated_leader_vrf_output = hasher.finalize();
                    if calculated_leader_vrf_output.as_slice() != leader_vrf_output.as_slice() {
                        error!(
                            "Leader VRF output hash mismatch. was: {}, expected: {}",
                            hex::encode(calculated_leader_vrf_output),
                            hex::encode(&leader_vrf_output)
                        );
                        false
                    } else {
                        // The leader VRF output hash matches what was in the block
                        // Now we need to check if the pool had enough sigma stake to produce this block
                        if self.pool_meets_delegation_threshold(
                            &sigma,
                            absolute_slot,
                            leader_vrf_output,
                        ) {
                            // TODO: Validate the KES signature
                            true
                        } else {
                            false
                        }
                    }
                }
            }
            Err(error) => {
                error!("Could not verify block vrf: {}", error);
                false
            }
        }
    }

    /// Verify that the pool meets the delegation threshold
    fn pool_meets_delegation_threshold(
        &self,
        sigma: &FixedDecimal,
        absolute_slot: u64,
        leader_vrf_output: Vec<u8>,
    ) -> bool {
        let limbs = leader_vrf_output
            .chunks(size_of::<u64>())
            .map(|chunk| Limb::from_be_bytes(chunk.try_into().expect("Infallible")))
            .collect();
        let certified_leader_vrf = FixedDecimal::from(Natural::from_owned_limbs_desc(limbs));
        let denominator = CERTIFIED_NATURAL_MAX.deref() - &certified_leader_vrf;
        let recip_q = CERTIFIED_NATURAL_MAX.deref() / &denominator;
        let x = -(sigma * self.c);

        trace!("certified_leader_vrf: {}", certified_leader_vrf);
        trace!("denominator: {}", denominator);
        trace!("recip_q: {}", recip_q);
        trace!("c: {}", self.c);
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
                true
            }
            _ => {
                trace!(
                    "Slot: {} - NOT Leader: {} >= {}",
                    absolute_slot,
                    recip_q,
                    ordering.approx
                );
                false
            }
        }
    }

    /// Validate that the VRF key hash in the block matches the VRF key hash in the ledger
    fn ledger_matches_block_vrf_key_hash(&self, pool_id: &PoolId, vrf_vkey: &[u8]) -> bool {
        let vrf_vkey_hash = Hasher::<256>::hash(vrf_vkey);
        trace!("block vrf_vkey_hash: {}", hex::encode(vrf_vkey_hash));
        let ledger_vrf_vkey_hash = match self.pool_info.vrf_vkey_hash(pool_id) {
            Ok(ledger_vrf_vkey_hash) => ledger_vrf_vkey_hash,
            Err(error) => {
                warn!("{:?} - {:?}", error, pool_id);
                return false;
            }
        };
        if vrf_vkey_hash.as_slice() != ledger_vrf_vkey_hash {
            error!(
                "VRF vkey hash in block ({}) does not match registered ledger vrf vkey hash ({})",
                hex::encode(vrf_vkey_hash),
                hex::encode(ledger_vrf_vkey_hash)
            );
            return false;
        }
        true
    }

    fn mk_vrf_input(&self, absolute_slot: u64, eta0: &[u8]) -> Hash<32> {
        trace!("mk_vrf_input() absolute_slot {}", absolute_slot);
        let mut hasher = Hasher::<256>::new();
        hasher.input(&absolute_slot.to_be_bytes());
        hasher.input(eta0);
        hasher.finalize()
    }

    #[allow(unused)]
    fn is_overlay_slot(&self, absolute_slot: u64) -> bool {
        let diff_slot: Rational =
            Rational::from(absolute_slot - self.slot_calculator.first_slot_in_epoch(absolute_slot));
        let diff_slot_inc: Rational = &diff_slot + Rational::from(1);
        let left: Integer = (self.decentralization * &diff_slot).ceiling();
        let right: Integer = (self.decentralization * &diff_slot_inc).ceiling();

        trace!("is_overlay_slot: d: {:?}", self.decentralization);
        trace!("is_overlay_slot: diff_slot: {:?}", &diff_slot);
        trace!("is_overlay_slot: diff_slot_inc: {:?}", &diff_slot_inc);
        trace!("is_overlay_slot: left: {:?}", &left);
        trace!("is_overlay_slot: right: {:?}", &right);
        trace!(
            "is_overlay_slot: result: {:?} - {:?}",
            absolute_slot,
            left < right
        );

        left < right
    }
}

impl Validator for BlockValidator<'_> {
    fn validate(&self) -> bool {
        match self.multi_era_header {
            MultiEraHeader::EpochBoundary(_) => self.validate_epoch_boundary(),
            MultiEraHeader::Byron(_) => self.validate_byron(),
            MultiEraHeader::ShelleyCompatible(_) => self.validate_shelley_compatible(),
            MultiEraHeader::BabbageCompatible(_) => self.validate_babbage_compatible(),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::block::BlockValidator;
    use amaru_common::ledger::{MockPoolInfo, MockSlotCalculator, Sigma};
    use amaru_common::pool::PoolId;
    use amaru_common::validator::Validator;
    use ctor::ctor;
    use malachite::Rational;
    use mockall::predicate::eq;
    use pallas_crypto::hash::Hash;
    use pallas_math::math::{FixedDecimal, FixedPrecision};
    use pallas_traverse::MultiEraHeader;
    use std::fs::File;
    use std::io::Read;

    #[ctor]
    fn init() {
        // set rust log level to TRACE
        // std::env::set_var("RUST_LOG", "amaru_consensus=trace");

        // initialize tracing crate
        tracing_subscriber::fmt::init();
    }

    #[test]
    fn test_validate_conway_block() {
        let mut pool_info = MockPoolInfo::new();
        // bcsh pool in epoch 508 made block 10817298
        let bcsh_pool_id = PoolId::from(
            hex::decode("00beef0a9be2f6d897ed24a613cf547bb20cd282a04edfc53d477114")
                .unwrap()
                .as_slice(),
        );
        pool_info
            .expect_sigma()
            .with(eq(bcsh_pool_id))
            .returning(|_| {
                Ok(Sigma {
                    numerator: 25626202470912,
                    denominator: 22586623335121436,
                })
            });
        pool_info
            .expect_vrf_vkey_hash()
            .with(eq(bcsh_pool_id))
            .returning(|_| {
                Ok(<[u8; 32]>::try_from(
                    hex::decode("c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b")
                        .unwrap(),
                )
                .unwrap())
            });

        let mut slot_calculator = MockSlotCalculator::new();
        slot_calculator
            .expect_first_slot_in_epoch()
            .with(eq(134402628))
            .returning(|_| 134092800);

        let epoch_nonce: Hash<32> =
            "c7937fc47fecbe687891b3decd71e904d1e129598aa3852481d295eea3ea3ada"
                .parse()
                .unwrap();
        let decentralization: Rational = Rational::from(0u64);
        let active_slots_coeff: FixedDecimal =
            FixedDecimal::from(5u64) / FixedDecimal::from(100u64);

        let c = (FixedDecimal::from(1u64) - active_slots_coeff).ln();

        let conway_block_tag: u8 = 6;
        let mut cbor = Vec::new();
        File::open("tests/data/mainnet_blockheader_10817298.cbor")
            .unwrap()
            .read_to_end(&mut cbor)
            .unwrap();

        let multi_era_header = MultiEraHeader::decode(conway_block_tag, None, &cbor).unwrap();
        assert_eq!(multi_era_header.slot(), 134402628u64);

        let block_validator = BlockValidator::new(
            &multi_era_header,
            &pool_info,
            &slot_calculator,
            &epoch_nonce,
            &decentralization,
            &c,
        );
        assert_eq!(block_validator.validate(), true);

        // lower our stake and expect that we are not a leader
        let mut pool_info = MockPoolInfo::new();
        pool_info
            .expect_sigma()
            .with(eq(bcsh_pool_id))
            .returning(|_| {
                Ok(Sigma {
                    numerator: 6026202470912,
                    denominator: 22586623335121436,
                })
            });
        pool_info
            .expect_vrf_vkey_hash()
            .with(eq(bcsh_pool_id))
            .returning(|_| {
                Ok(<[u8; 32]>::try_from(
                    hex::decode("c0d1f9b040d2f6fd7fc8775d24753d6db4b697429f11404a6178a0a4a005867b")
                        .unwrap(),
                )
                .unwrap())
            });

        let block_validator = BlockValidator::new(
            &multi_era_header,
            &pool_info,
            &slot_calculator,
            &epoch_nonce,
            &decentralization,
            &c,
        );
        assert_eq!(block_validator.validate(), false);
    }
}
